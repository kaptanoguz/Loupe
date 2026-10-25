//! S3 uyumlu hedefler: AWS S3, Backblaze B2, Cloudflare R2, Wasabi.
//!
//! İmzalama `rusty-s3` (SigV4, ön imzalı URL, G/Ç yok), taşıma `reqwest`.
//! Akış en fazla bir parça (varsayılan 8 MiB) tamponlanır:
//! - akış ilk parça dolmadan biterse tekli `PUT` (`Content-MD5` imzalı);
//! - yoksa multipart: her parça `Content-MD5` ile; bir hata olursa
//!   `AbortMultipartUpload`, yarım nesne kalmaz.
//!
//! Beklenen ETag yerelde hesaplanır (tekli: MD5; multipart: parça MD5'lerinin
//! MD5'i + `-N`) ve `HEAD` ile karşılaştırılır. SSE-KMS gibi ETag'in MD5
//! olmadığı durumlarda motor geri okuma doğrulamasına düşer.

use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bytes::{Bytes, BytesMut};
use futures_util::{StreamExt, TryStreamExt};
use md5::{Digest, Md5};
use reqwest::{Client, Response, StatusCode};
use rusty_s3::actions::CreateMultipartUpload;
use rusty_s3::{Bucket, S3Action, UrlStyle};

use super::{
    ByteStream, ChecksumAlgo, CloudProvider, Credentials, ObjectMeta, RemoteChecksum, UploadReceipt, check_key, hex,
};
use crate::CloudError;

pub const DEFAULT_PART_SIZE: usize = 8 * 1024 * 1024;
/// S3'ün son parça dışındaki en küçük parça boyu.
pub const MIN_PART_SIZE: usize = 5 * 1024 * 1024;
const SIGN_TTL: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3Config {
    /// `https://s3.eu-central-003.backblazeb2.com`, `https://<hesap>.r2.cloudflarestorage.com`…
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub path_style: bool,
    pub part_size: usize,
}

#[derive(Debug)]
pub struct S3Provider {
    bucket: Bucket,
    creds: rusty_s3::Credentials,
    http: Client,
    part_size: usize,
}

async fn check(res: Response) -> Result<Response, CloudError> {
    if res.status().is_success() {
        return Ok(res);
    }
    let status = res.status().as_u16();
    let retry_after = res
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs);
    let body = res.text().await.unwrap_or_default();
    // S3 yavaşlama isteğini 503 SlowDown ile de verir.
    let status = if body.contains("<Code>SlowDown</Code>") { 503 } else { status };
    Err(CloudError::Http { status, retry_after, body: body.chars().take(512).collect() })
}

fn etag_of(res: &Response) -> Option<String> {
    res.headers().get("etag").and_then(|v| v.to_str().ok()).map(|v| v.trim_matches('"').to_ascii_lowercase())
}

/// MD5 tabanlı ETag mi (32 hex, isteğe bağlı `-N`)?
fn md5_like(etag: &str) -> bool {
    let (h, n) = etag.split_once('-').unwrap_or((etag, "1"));
    h.len() == 32 && h.bytes().all(|b| b.is_ascii_hexdigit()) && n.parse::<u32>().is_ok()
}

impl S3Provider {
    pub fn new(cfg: S3Config, creds: Credentials, http: Client) -> Result<Self, CloudError> {
        let endpoint = url::Url::parse(&cfg.endpoint).map_err(|e| CloudError::Config(format!("S3 uç noktası: {e}")))?;
        let style = if cfg.path_style { UrlStyle::Path } else { UrlStyle::VirtualHost };
        let bucket = Bucket::new(endpoint, style, cfg.bucket, cfg.region)
            .map_err(|e| CloudError::Config(format!("S3 bucket: {e}")))?;
        Ok(Self {
            bucket,
            creds: rusty_s3::Credentials::new(creds.user, creds.secret.as_str()),
            http,
            part_size: cfg.part_size.max(MIN_PART_SIZE),
        })
    }

    /// Testler için: küçük parça boyu (S3 alt sınırı uygulanmaz).
    #[must_use]
    pub const fn with_part_size_unchecked(mut self, part_size: usize) -> Self {
        self.part_size = part_size;
        self
    }

    async fn put_single(&self, key: &str, body: Bytes) -> Result<UploadReceipt, CloudError> {
        let md5 = Md5::digest(&body);
        let content_md5 = STANDARD.encode(md5);
        let mut action = self.bucket.put_object(Some(&self.creds), key);
        action.headers_mut().insert("content-md5", content_md5.clone());
        let url = action.sign(SIGN_TTL);
        let size = body.len() as u64;
        let res = check(self.http.put(url).header("content-md5", content_md5).body(body).send().await?).await?;
        let expected = hex(&md5);
        let got = etag_of(&res);
        if got.as_deref().is_some_and(|g| md5_like(g) && g != expected) {
            return Err(CloudError::RemoteMismatch(format!("PUT ETag {got:?} ≠ MD5 {expected}")));
        }
        Ok(UploadReceipt { size, expected: Some(RemoteChecksum { algo: ChecksumAlgo::Etag, value: expected }) })
    }

    async fn put_part(
        &self,
        key: &str,
        upload_id: &str,
        n: u16,
        body: Bytes,
    ) -> Result<(String, [u8; 16]), CloudError> {
        let md5: [u8; 16] = Md5::digest(&body).into();
        let content_md5 = STANDARD.encode(md5);
        let mut action = self.bucket.upload_part(Some(&self.creds), key, n, upload_id);
        action.headers_mut().insert("content-md5", content_md5.clone());
        let url = action.sign(SIGN_TTL);
        let res = check(self.http.put(url).header("content-md5", content_md5).body(body).send().await?).await?;
        let etag = etag_of(&res).ok_or_else(|| CloudError::RemoteMismatch(format!("parça {n} ETag'siz")))?;
        Ok((etag, md5))
    }

    async fn multipart(&self, key: &str, first: Bytes, body: &mut ByteStream) -> Result<UploadReceipt, CloudError> {
        let url = CreateMultipartUpload::new(&self.bucket, Some(&self.creds), key).sign(SIGN_TTL);
        let res = check(self.http.post(url).send().await?).await?;
        let xml = res.bytes().await?;
        let created = CreateMultipartUpload::parse_response(&xml)
            .map_err(|e| CloudError::RemoteMismatch(format!("CreateMultipartUpload yanıtı: {e}")))?;
        let upload_id = created.upload_id().to_owned();

        let result = self.send_parts(key, &upload_id, first, body).await;
        if result.is_err() {
            let url = self.bucket.abort_multipart_upload(Some(&self.creds), key, &upload_id).sign(SIGN_TTL);
            let _aborted = self.http.delete(url).send().await;
        }
        result
    }

    async fn send_parts(
        &self,
        key: &str,
        upload_id: &str,
        first: Bytes,
        body: &mut ByteStream,
    ) -> Result<UploadReceipt, CloudError> {
        let mut etags: Vec<String> = Vec::new();
        let mut md5s: Vec<u8> = Vec::new();
        let mut size = first.len() as u64;
        let mut n: u16 = 1;
        let (etag, md5) = self.put_part(key, upload_id, n, first).await?;
        etags.push(etag);
        md5s.extend_from_slice(&md5);
        let mut buf = BytesMut::with_capacity(self.part_size);
        loop {
            let next = body.next().await.transpose()?;
            if let Some(chunk) = &next {
                buf.extend_from_slice(chunk);
                size += chunk.len() as u64;
            }
            let last = next.is_none();
            while buf.len() >= self.part_size || (last && !buf.is_empty()) {
                let take = buf.len().min(self.part_size);
                let part = buf.split_to(take).freeze();
                n = n.checked_add(1).ok_or_else(|| CloudError::Config("10 000 parçadan fazla".into()))?;
                let (etag, md5) = self.put_part(key, upload_id, n, part).await?;
                etags.push(etag);
                md5s.extend_from_slice(&md5);
            }
            if last {
                break;
            }
        }
        let action =
            self.bucket.complete_multipart_upload(Some(&self.creds), key, upload_id, etags.iter().map(String::as_str));
        let url = action.sign(SIGN_TTL);
        let xml = action.body();
        let res = check(self.http.post(url).body(xml).send().await?).await?;
        // CompleteMultipartUpload 200 içinde de hata dönebilir.
        let text = res.text().await?;
        if text.contains("<Error>") {
            return Err(CloudError::Http { status: 500, retry_after: None, body: text.chars().take(512).collect() });
        }
        let expected = format!("{}-{}", hex(&Md5::digest(&md5s)), etags.len());
        Ok(UploadReceipt { size, expected: Some(RemoteChecksum { algo: ChecksumAlgo::Etag, value: expected }) })
    }
}

#[async_trait]
impl CloudProvider for S3Provider {
    fn name(&self) -> &str {
        "s3"
    }

    async fn upload_stream(&self, key: &str, mut body: ByteStream) -> Result<UploadReceipt, CloudError> {
        check_key(key)?;
        let mut buf = BytesMut::with_capacity(self.part_size);
        while buf.len() < self.part_size {
            match body.next().await.transpose()? {
                Some(chunk) => buf.extend_from_slice(&chunk),
                None => return self.put_single(key, buf.freeze()).await,
            }
        }
        let first = buf.split_to(self.part_size).freeze();
        let rest = buf.freeze();
        let mut body: ByteStream = if rest.is_empty() {
            body
        } else {
            Box::pin(futures_util::stream::once(async move { Ok(rest) }).chain(body))
        };
        self.multipart(key, first, &mut body).await
    }

    async fn download_stream(&self, key: &str) -> Result<ByteStream, CloudError> {
        check_key(key)?;
        let url = self.bucket.get_object(Some(&self.creds), key).sign(SIGN_TTL);
        let res = check(self.http.get(url).send().await?).await?;
        Ok(Box::pin(res.bytes_stream().map_err(CloudError::from)))
    }

    async fn head_object(&self, key: &str) -> Result<Option<ObjectMeta>, CloudError> {
        check_key(key)?;
        let url = self.bucket.head_object(Some(&self.creds), key).sign(SIGN_TTL);
        let res = self.http.head(url).send().await?;
        if res.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let res = check(res).await?;
        let size = res
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| CloudError::RemoteMismatch("HEAD Content-Length yok".into()))?;
        let checksum =
            etag_of(&res).filter(|e| md5_like(e)).map(|value| RemoteChecksum { algo: ChecksumAlgo::Etag, value });
        Ok(Some(ObjectMeta { size, checksum }))
    }

    async fn delete_object(&self, key: &str) -> Result<(), CloudError> {
        check_key(key)?;
        let url = self.bucket.delete_object(Some(&self.creds), key).sign(SIGN_TTL);
        let res = self.http.delete(url).send().await?;
        if res.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        check(res).await.map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_etags_are_recognised() {
        assert!(md5_like("9e107d9d372bb6826bd81d3542a419d6"));
        assert!(md5_like("9e107d9d372bb6826bd81d3542a419d6-3"));
        assert!(!md5_like("9e107d9d372bb6826bd81d3542a419d"));
        assert!(!md5_like("not-an-etag"));
    }
}

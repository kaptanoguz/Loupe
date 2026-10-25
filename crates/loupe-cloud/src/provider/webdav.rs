//! WebDAV hedefi (Nextcloud, ownCloud, Synology/QNAP WebDAV sunucusu…).
//!
//! Üst koleksiyonlar `MKCOL` ile sırayla oluşturulur (var olan 405 döner),
//! nesne tek `PUT` ile akıtılır. WebDAV standart bir içerik checksum'ı
//! vermediği için [`UploadReceipt::expected`] boştur; motor nesneyi geri okuyup
//! doğrular.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use futures_util::TryStreamExt;
use reqwest::{Client, Method, RequestBuilder, StatusCode};
use url::Url;

use super::{ByteStream, CloudProvider, Credentials, ObjectMeta, UploadReceipt, check_key};
use crate::CloudError;

#[derive(Debug)]
pub struct WebDavProvider {
    base: Url,
    auth: Option<Credentials>,
    http: Client,
}

async fn check(res: reqwest::Response) -> Result<reqwest::Response, CloudError> {
    if res.status().is_success() {
        return Ok(res);
    }
    let status = res.status().as_u16();
    let body = res.text().await.unwrap_or_default();
    Err(CloudError::Http { status, retry_after: None, body: body.chars().take(512).collect() })
}

impl WebDavProvider {
    pub fn new(base: &str, auth: Option<Credentials>, http: Client) -> Result<Self, CloudError> {
        let mut base = Url::parse(base).map_err(|e| CloudError::Config(format!("WebDAV adresi: {e}")))?;
        if !matches!(base.scheme(), "https" | "http") {
            return Err(CloudError::Config("WebDAV adresi http(s) olmalı".into()));
        }
        if !base.path().ends_with('/') {
            let p = format!("{}/", base.path());
            base.set_path(&p);
        }
        Ok(Self { base, auth, http })
    }

    fn url(&self, key: &str) -> Result<Url, CloudError> {
        check_key(key)?;
        let mut u = self.base.clone();
        {
            let mut segs = u.path_segments_mut().map_err(|()| CloudError::Config("WebDAV adresi kök olamaz".into()))?;
            segs.pop_if_empty();
            segs.extend(key.split('/'));
        }
        Ok(u)
    }

    fn req(&self, method: Method, url: Url) -> RequestBuilder {
        let r = self.http.request(method, url);
        match &self.auth {
            Some(c) => r.basic_auth(&c.user, Some(c.secret.as_str())),
            None => r,
        }
    }

    async fn mkcols(&self, key: &str) -> Result<(), CloudError> {
        let parts: Vec<&str> = key.split('/').collect();
        let mkcol = Method::from_bytes(b"MKCOL").map_err(|e| CloudError::Config(e.to_string()))?;
        for i in 1..parts.len() {
            let mut u = self.url(&parts[..i].join("/"))?;
            u.set_path(&format!("{}/", u.path()));
            let res = self.req(mkcol.clone(), u).send().await?;
            match res.status() {
                s if s.is_success() => {}
                // Zaten var.
                StatusCode::METHOD_NOT_ALLOWED => {}
                _ => {
                    check(res).await?;
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl CloudProvider for WebDavProvider {
    fn name(&self) -> &str {
        "webdav"
    }

    async fn upload_stream(&self, key: &str, body: ByteStream) -> Result<UploadReceipt, CloudError> {
        let url = self.url(key)?;
        self.mkcols(key).await?;
        let counted = Arc::new(AtomicU64::new(0));
        let c = counted.clone();
        let body = body.inspect_ok(move |b| {
            c.fetch_add(b.len() as u64, Ordering::Relaxed);
        });
        check(self.req(Method::PUT, url).body(reqwest::Body::wrap_stream(body)).send().await?).await?;
        Ok(UploadReceipt { size: counted.load(Ordering::Relaxed), expected: None })
    }

    async fn download_stream(&self, key: &str) -> Result<ByteStream, CloudError> {
        let res = check(self.req(Method::GET, self.url(key)?).send().await?).await?;
        Ok(Box::pin(res.bytes_stream().map_err(CloudError::from)))
    }

    async fn head_object(&self, key: &str) -> Result<Option<ObjectMeta>, CloudError> {
        let res = self.req(Method::HEAD, self.url(key)?).send().await?;
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
        Ok(Some(ObjectMeta { size, checksum: None }))
    }

    async fn delete_object(&self, key: &str) -> Result<(), CloudError> {
        let res = self.req(Method::DELETE, self.url(key)?).send().await?;
        if res.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        check(res).await.map(drop)
    }
}

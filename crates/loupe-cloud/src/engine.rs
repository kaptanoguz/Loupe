//! Yükleme motoru (RFC-001B §3.3).
//!
//! ```text
//! yerel dosya ─▶ BLAKE3 (okurken) ─▶ age (cihaz + kurtarma) ─▶ 1 MiB parçalar
//!   ─▶ token bucket ─▶ CloudProvider::upload_stream ─▶ HEAD + checksum ─▶ sync_status = synced
//! ```
//!
//! - Okuma ve şifreleme ayrı bir iş parçacığında, yükleme async tarafta; aradaki
//!   kanal 4 parçalıktır (geri basınç: ağ yavaşsa disk okuması da yavaşlar).
//!   Diskte şifreli geçici kopya oluşmaz.
//! - Dosya sonunda düz metin BLAKE3'ü katalogdakiyle karşılaştırılır. Uyuşmazsa
//!   akışa hata yazılır, sağlayıcı yüklemeyi iptal eder: **bozuk dosya buluta
//!   çıkmaz** ve sağlam bir uzak kopyanın üzerine yazılmaz.
//! - Doğrulama: sağlayıcının bildirdiği checksum, yükleme sırasında akıştan
//!   hesaplananla aynı olmalı. Sağlayıcı checksum vermiyorsa (WebDAV) ya da
//!   [`VerifyMode::ReadBack`] seçildiyse nesne geri indirilir, cihaz anahtarıyla
//!   çözülür ve BLAKE3'ü katalogla karşılaştırılır.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use age::x25519::Identity;
use bytes::{Bytes, BytesMut};
use futures_util::{StreamExt, TryStreamExt};
use loupe_core::{Blake3Hash, LibraryId, MediaId, RelPath};
use loupe_db::Pool;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::io::{StreamReader, SyncIoBridge};

use crate::crypto::decrypt_from;
use crate::provider::{ChecksumAlgo, UploadReceipt};
use crate::queue::{self, Lease, SyncedObject, write_tx};
use crate::targets::Target;
use crate::{CloudError, CloudProvider, ErrorClass, Keyring, Limiter, TokenBucket, backoff};

/// Akış parçası (kanal ve hız sınırı birimi).
pub const CHUNK: usize = 1024 * 1024;
const READ_BUF: usize = 256 * 1024;
/// Kira süresi: çöken işçinin işi bu süre sonra yeniden alınır.
const LEASE_MS: i64 = 30 * 60 * 1000;

/// Kütüphane kökleri (disk takılı değilse `None`: iş `LOCAL_MISSING` ile bekler).
pub trait LibraryRoots: Send + Sync + std::fmt::Debug {
    fn root(&self, library: LibraryId) -> Option<PathBuf>;
}

impl LibraryRoots for HashMap<LibraryId, PathBuf> {
    fn root(&self, library: LibraryId) -> Option<PathBuf> {
        self.get(&library).cloned()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyMode {
    /// Sağlayıcı checksum'ı; yoksa geri okuma.
    Checksum,
    /// Her zaman geri indir, çöz, BLAKE3 karşılaştır.
    ReadBack,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadOutcome {
    Synced { remote_key: String, remote_size: u64, checksum_algo: &'static str },
    Retrying { class: ErrorClass, at_ms: i64 },
    Failed { class: ErrorClass },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadReport {
    pub media_id: MediaId,
    pub outcome: UploadOutcome,
    pub message: Option<String>,
}

#[derive(Debug)]
pub struct BackupEngine {
    pool: Arc<Pool>,
    provider: Arc<dyn CloudProvider>,
    target: Target,
    keyring: Option<Arc<Keyring>>,
    limiter: Limiter,
    roots: Arc<dyn LibraryRoots>,
    verify: VerifyMode,
    owner: String,
}

/// Kanala 1 MiB'lık parçalar yazan `Write`. Alıcı düşerse `BrokenPipe`.
struct ChannelWriter {
    tx: mpsc::Sender<Result<Bytes, CloudError>>,
    buf: BytesMut,
}

impl ChannelWriter {
    fn send(&self, b: Bytes) -> io::Result<()> {
        self.tx.blocking_send(Ok(b)).map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
    }

    fn finish(mut self) -> io::Result<()> {
        if !self.buf.is_empty() {
            let rest = self.buf.split().freeze();
            self.send(rest)?;
        }
        Ok(())
    }
}

impl Write for ChannelWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        while self.buf.len() >= CHUNK {
            let b = self.buf.split_to(CHUNK).freeze();
            self.send(b)?;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Okurken BLAKE3 hesaplayan yazıcı (geri okuma ve geri yükleme).
struct HashingWriter<W> {
    inner: W,
    hasher: blake3::Hasher,
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(data)?;
        self.hasher.update(&data[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Üretici: dosyayı okur, BLAKE3'ler, (gerekirse) şifreler, kanala yazar.
/// `Ok(None)`: alıcı (yükleme) önce bitti, iş iptal.
fn produce(
    path: &Path,
    keyring: Option<&Keyring>,
    expected: Blake3Hash,
    tx: mpsc::Sender<Result<Bytes, CloudError>>,
) -> Result<Option<Blake3Hash>, CloudError> {
    let cancelled = |e: &io::Error| e.kind() == io::ErrorKind::BrokenPipe;
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(CloudError::LocalMissing(path.to_path_buf())),
        Err(e) => return Err(CloudError::Io { context: "yerel okuma", source: e }),
    };
    let sink = ChannelWriter { tx: tx.clone(), buf: BytesMut::with_capacity(CHUNK) };
    enum Out {
        Plain(ChannelWriter),
        Age(age::stream::StreamWriter<ChannelWriter>),
    }
    let mut out = match keyring {
        Some(k) => Out::Age(k.encrypt_to(sink)?),
        None => Out::Plain(sink),
    };
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; READ_BUF];
    let pumped: io::Result<()> = (|| {
        loop {
            let n = match file.read(&mut buf) {
                Ok(0) => return Ok(()),
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            hasher.update(&buf[..n]);
            match &mut out {
                Out::Plain(w) => w.write_all(&buf[..n])?,
                Out::Age(w) => w.write_all(&buf[..n])?,
            }
        }
    })();
    match pumped {
        Err(e) if cancelled(&e) => return Ok(None),
        Err(e) => return Err(CloudError::Io { context: "yerel okuma", source: e }),
        Ok(()) => {}
    }
    let sink = match out {
        Out::Plain(w) => Ok(w),
        Out::Age(w) => w.finish(),
    };
    let sink = match sink {
        Ok(s) => s,
        Err(e) if cancelled(&e) => return Ok(None),
        Err(e) => return Err(CloudError::Crypto(e.to_string())),
    };
    let actual = Blake3Hash(*hasher.finalize().as_bytes());
    if actual != expected {
        let err = || CloudError::LocalCorrupt { expected: expected.to_hex(), actual: actual.to_hex() };
        // Akış bitmeden hata: sağlayıcı yüklemeyi iptal eder.
        let _sent = tx.blocking_send(Err(err()));
        return Err(err());
    }
    match sink.finish() {
        Err(e) if cancelled(&e) => Ok(None),
        Err(e) => Err(CloudError::Io { context: "akış", source: e }),
        Ok(()) => Ok(Some(actual)),
    }
}

fn now_ms() -> i64 {
    loupe_core::now_ms()
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, CloudError> + Send + 'static,
) -> Result<T, CloudError> {
    tokio::task::spawn_blocking(f).await?
}

impl BackupEngine {
    /// Şifreli hedefte anahtarlık zorunludur ve kurtarma alıcısı hedefe
    /// kayıtlı olanla aynı olmalıdır (yanlış anahtarla yedek alınmaz).
    pub fn new(
        pool: Arc<Pool>,
        provider: Arc<dyn CloudProvider>,
        target: Target,
        keyring: Option<Keyring>,
        roots: Arc<dyn LibraryRoots>,
    ) -> Result<Self, CloudError> {
        let keyring = match (target.encrypted, keyring) {
            (true, Some(k)) => {
                if target.age_recipient.as_deref() != Some(k.recovery_recipient().as_str()) {
                    return Err(CloudError::Config("anahtarlığın kurtarma alıcısı hedefinkiyle aynı değil".into()));
                }
                Some(Arc::new(k))
            }
            (true, None) => return Err(CloudError::Config("şifreli hedef için anahtarlık gerekli".into())),
            (false, _) => None,
        };
        let mut limiter = Limiter::unlimited();
        if let Some(bps) = target.bandwidth_bps {
            limiter = limiter.with(Arc::new(TokenBucket::per_second(bps)));
        }
        let owner = format!("{}/cloud-{}", std::process::id(), target.id);
        Ok(Self { pool, provider, target, keyring, limiter, roots, verify: VerifyMode::Checksum, owner })
    }

    /// Genel bant genişliği kovası (`cloud.global_bandwidth_bps`), tüm hedeflerle paylaşılır.
    #[must_use]
    pub fn with_global_limit(mut self, bucket: Arc<TokenBucket>) -> Self {
        self.limiter = self.limiter.with(bucket);
        self
    }

    #[must_use]
    pub const fn with_verify(mut self, mode: VerifyMode) -> Self {
        self.verify = mode;
        self
    }

    #[must_use]
    pub const fn target(&self) -> &Target {
        &self.target
    }

    /// Şifreli: `objects/<h[0:2]>/<h[2:4]>/<blake3>.age` (ad ve klasör yapısı
    /// bulutta görünmez). Şifresiz: yerel ile aynı okunabilir ağaç.
    #[must_use]
    pub fn remote_key(&self, blake3: &Blake3Hash, rel_path: &RelPath) -> String {
        let prefix = self.target.key_prefix.trim_matches('/');
        let prefix = if prefix.is_empty() { String::new() } else { format!("{prefix}/") };
        if self.keyring.is_some() {
            let h = blake3.to_hex();
            format!("{prefix}objects/{}/{}/{h}.age", &h[..2], &h[2..4])
        } else {
            format!("{prefix}{}", rel_path.as_str())
        }
    }

    /// Kuyruktan bir iş alır ve sonuçlandırır. Hazır iş yoksa `None`.
    pub async fn run_once(&self) -> Result<Option<UploadReport>, CloudError> {
        let (pool, target, owner) = (self.pool.clone(), self.target.id, self.owner.clone());
        let lease =
            blocking(move || write_tx(&pool, |c| queue::claim_next(c, target, &owner, LEASE_MS, now_ms()))).await?;
        let Some(lease) = lease else { return Ok(None) };

        let result = self.process(&lease).await;
        let pool = self.pool.clone();
        let report = match result {
            Ok(obj) => {
                let outcome = UploadOutcome::Synced {
                    remote_key: obj.remote_key.clone(),
                    remote_size: obj.remote_size,
                    checksum_algo: obj.remote_checksum_algo,
                };
                blocking(move || write_tx(&pool, |c| queue::mark_synced(c, &lease, &obj, now_ms()))).await?;
                UploadReport { media_id: lease.media_id, outcome, message: None }
            }
            Err(e) => {
                let class = e.class();
                let wait = e.retry_after().unwrap_or_else(|| backoff::delay(lease.attempt, backoff::jitter()));
                let now = now_ms();
                let at = now + i64::try_from(wait.as_millis()).unwrap_or(i64::MAX / 2);
                let msg = e.to_string();
                let m = msg.clone();
                let retry =
                    blocking(move || write_tx(&pool, |c| queue::mark_failed(c, &lease, class, &m, at, now))).await?;
                let outcome = match retry {
                    Some(at_ms) => UploadOutcome::Retrying { class, at_ms },
                    None => UploadOutcome::Failed { class },
                };
                UploadReport { media_id: lease.media_id, outcome, message: Some(msg) }
            }
        };
        Ok(Some(report))
    }

    /// Hazır işler bitene dek çalışır.
    pub async fn drain(&self) -> Result<Vec<UploadReport>, CloudError> {
        let mut out = Vec::new();
        while let Some(r) = self.run_once().await? {
            out.push(r);
        }
        Ok(out)
    }

    async fn process(&self, lease: &Lease) -> Result<SyncedObject, CloudError> {
        let (pool, id) = (self.pool.clone(), lease.media_id);
        let media = blocking(move || Ok(pool.read(|c| loupe_db::repo::media::get(c, id))?)).await?;
        let media = media.ok_or_else(|| CloudError::Config(format!("medya {} katalogda yok", id.0)))?;
        let root = self
            .roots
            .root(media.library_id)
            .ok_or_else(|| CloudError::LocalMissing(PathBuf::from(format!("kütüphane {}", media.library_id))))?;
        let path = media.rel_path.to_path(&root);
        let key = self.remote_key(&media.blake3, &media.rel_path);

        let receipt = self.upload(&path, &key, media.blake3).await?;
        let (checksum, algo) = self.verify(&key, &receipt, media.blake3).await?;
        Ok(SyncedObject {
            remote_key: key,
            remote_size: receipt.size,
            remote_checksum: checksum,
            remote_checksum_algo: algo.as_db(),
            key_fingerprint: self.keyring.as_ref().map(|k| k.fingerprint()),
        })
    }

    async fn upload(&self, path: &Path, key: &str, expected: Blake3Hash) -> Result<UploadReceipt, CloudError> {
        let (tx, rx) = mpsc::channel::<Result<Bytes, CloudError>>(4);
        let (keyring, path) = (self.keyring.clone(), path.to_path_buf());
        let producer = tokio::task::spawn_blocking(move || produce(&path, keyring.as_deref(), expected, tx));
        let limiter = self.limiter.clone();
        let body = ReceiverStream::new(rx).then(move |item| {
            let limiter = limiter.clone();
            async move {
                if let Ok(b) = &item {
                    limiter.acquire(b.len() as u64).await;
                }
                item
            }
        });
        let uploaded = self.provider.upload_stream(key, Box::pin(body)).await;
        match (producer.await?, uploaded) {
            (Err(local), Ok(_)) => {
                // Olmamalı (sağlayıcı akış hatasını görür), yine de uzak kopya bırakma.
                let _removed = self.provider.delete_object(key).await;
                Err(local)
            }
            (Err(local), Err(_)) => Err(local),
            (Ok(_), Err(remote)) => Err(remote),
            (Ok(None), Ok(_)) => Err(CloudError::RemoteMismatch("akış yarıda kesildi".into())),
            (Ok(Some(_)), Ok(receipt)) => Ok(receipt),
        }
    }

    async fn verify(
        &self,
        key: &str,
        receipt: &UploadReceipt,
        expected: Blake3Hash,
    ) -> Result<(String, ChecksumAlgo), CloudError> {
        if self.verify == VerifyMode::Checksum {
            let meta = self
                .provider
                .head_object(key)
                .await?
                .ok_or_else(|| CloudError::RemoteMismatch(format!("{key} yüklendikten sonra bulunamadı")))?;
            if meta.size != receipt.size {
                return Err(CloudError::RemoteMismatch(format!("boyut {} ≠ {}", meta.size, receipt.size)));
            }
            match (&receipt.expected, &meta.checksum) {
                (Some(e), Some(g)) if e.algo == g.algo && e.value == g.value => return Ok((g.value.clone(), g.algo)),
                // ETag her sağlayıcıda MD5 değildir (SSE-KMS…): geri okumaya düş.
                (Some(e), Some(g)) if e.algo == g.algo && g.algo != ChecksumAlgo::Etag => {
                    return Err(CloudError::RemoteMismatch(format!("{} {} ≠ {}", g.algo.as_db(), g.value, e.value)));
                }
                _ => {}
            }
        }
        let identity = self.keyring.as_ref().map(|k| k.device_identity().clone());
        let (actual, _) = self.fetch_plain(key, identity, io::sink()).await?;
        if actual != expected {
            return Err(CloudError::RemoteMismatch(format!(
                "geri okunan BLAKE3 {} ≠ {}",
                actual.to_hex(),
                expected.to_hex()
            )));
        }
        Ok((actual.to_hex(), ChecksumAlgo::Blake3))
    }

    /// Nesneyi indirir, (gerekirse) çözer ve düz metni `sink`'e yazarken BLAKE3 hesaplar.
    async fn fetch_plain<W: Write + Send + 'static>(
        &self,
        key: &str,
        identity: Option<Identity>,
        sink: W,
    ) -> Result<(Blake3Hash, W), CloudError> {
        let stream = self.provider.download_stream(key).await?.map_err(io::Error::from);
        let bridge = SyncIoBridge::new(StreamReader::new(stream));
        blocking(move || {
            let mut out = HashingWriter { inner: sink, hasher: blake3::Hasher::new() };
            match identity {
                Some(id) => {
                    let mut r = decrypt_from(&id, bridge)?;
                    io::copy(&mut r, &mut out).map_err(|e| CloudError::Crypto(e.to_string()))?;
                }
                None => {
                    let mut r = bridge;
                    io::copy(&mut r, &mut out).map_err(CloudError::io("indirme"))?;
                }
            }
            out.flush().map_err(CloudError::io("geri yükleme yazma"))?;
            Ok((Blake3Hash(*out.hasher.finalize().as_bytes()), out.inner))
        })
        .await
    }

    /// Uzak kopyayı geri okuyup doğrular (haftalık örneklem / aylık tam
    /// doğrulama); `last_verified_at` güncellenir.
    pub async fn verify_remote(&self, media_id: MediaId) -> Result<(), CloudError> {
        let (key, expected, _) = self.synced_object(media_id).await?;
        let identity = self.keyring.as_ref().map(|k| k.device_identity().clone());
        let (actual, _) = self.fetch_plain(&key, identity, io::sink()).await?;
        if actual != expected {
            return Err(CloudError::RemoteMismatch("uzak kopya katalogla eşleşmiyor".into()));
        }
        let (pool, target) = (self.pool.clone(), self.target.id);
        blocking(move || write_tx(&pool, |c| queue::mark_verified(c, media_id, target, now_ms()))).await
    }

    async fn synced_object(&self, media_id: MediaId) -> Result<(String, Blake3Hash, bool), CloudError> {
        let (pool, target) = (self.pool.clone(), self.target.id);
        let (status, media) = blocking(move || {
            Ok(pool.read(|c| {
                let s = queue::status(c, media_id, target).map_err(|e| loupe_db::DbError::Validation(e.to_string()))?;
                Ok((s, loupe_db::repo::media::get(c, media_id)?))
            })?)
        })
        .await?;
        let status = status
            .filter(|s| s.state == "synced")
            .ok_or_else(|| CloudError::Config(format!("medya {} bu hedefte yedekli değil", media_id.0)))?;
        let media = media.ok_or_else(|| CloudError::Config(format!("medya {} katalogda yok", media_id.0)))?;
        let key = status.remote_key.ok_or_else(|| CloudError::Config("remote_key yok".into()))?;
        Ok((key, media.blake3, status.encryption == "age"))
    }

    /// Geri yükleme: indir, `identity` ile çöz (kurtarma anahtarı ya da cihaz
    /// anahtarı), BLAKE3'ü katalogla karşılaştır, sonra `dest`'e taşı. Eşleşmezse
    /// `dest` oluşturulmaz.
    pub async fn restore(&self, media_id: MediaId, identity: &Identity, dest: &Path) -> Result<Blake3Hash, CloudError> {
        let (key, expected, encrypted) = self.synced_object(media_id).await?;
        let dir = dest.parent().ok_or_else(|| CloudError::Config("hedef yolun dizini yok".into()))?;
        let name = dest.file_name().and_then(|n| n.to_str()).unwrap_or("geri-yukleme");
        let tmp = dir.join(format!(".{name}.loupe-restore"));
        let file = std::fs::File::create(&tmp).map_err(CloudError::io("geri yükleme dosyası"))?;
        let id = encrypted.then(|| identity.clone());
        let fetched = self.fetch_plain(&key, id, file).await;
        let check = fetched.and_then(|(actual, file)| {
            file.sync_all().map_err(CloudError::io("geri yükleme fsync"))?;
            if actual == expected {
                Ok(actual)
            } else {
                Err(CloudError::RemoteMismatch(format!(
                    "geri yüklenen BLAKE3 {} ≠ {}",
                    actual.to_hex(),
                    expected.to_hex()
                )))
            }
        });
        match check {
            Ok(h) => {
                std::fs::rename(&tmp, dest).map_err(CloudError::io("geri yükleme yerleştirme"))?;
                Ok(h)
            }
            Err(e) => {
                let _removed = std::fs::remove_file(&tmp);
                Err(e)
            }
        }
    }
}

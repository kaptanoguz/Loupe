//! Model indirici (RFC-001G §3): manifestteki kaynakları indirir, boyut ve
//! SHA-256 ile doğrular, arşivden üyeyi çıkarır ve dosyayı atomik olarak
//! `~/.local/share/loupe/models/` altına koyar.
//!
//! - Yarım indirme `.partial/<kaynak>.part`'ta kalır; sonraki deneme `Range`
//!   ile kaldığı yerden sürer (sunucu desteklemezse baştan).
//! - Doğrulanmamış bayt asla asıl ada yazılmaz: önce `.partial/` altında
//!   tamamlanır, özet tutarsa `rename` edilir. Özet tutmazsa yarım dosya silinir
//!   ve hata döner; eski (doğrulanmış) dosya varsa yerinde kalır.
//! - Zaten doğrulanmış dosyanın kaynağı hiç indirilmez.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::AiError;
use crate::models::{
    Expected, FileState, Lock, LockEntry, Manifest, ModelFile, Source, commit, extract_member, hex, partial_dir,
    sha256_file, verify,
};

pub const HF_BASE: &str = "https://huggingface.co";
const ATTEMPTS: usize = 4;

/// İlerleme bildirimi (arayüz ve CLI).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub source: String,
    /// `resolve` | `download` | `verify` | `extract` | `done`
    pub phase: &'static str,
    pub done: u64,
    pub total: u64,
}

pub type ProgressFn = Arc<dyn Fn(Progress) + Send + Sync>;

/// Kurulum sonucu.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallReport {
    /// Bu çalıştırmada yerine konan dosyalar.
    pub installed: Vec<String>,
    /// Zaten doğrulanmış olduğu için dokunulmayanlar.
    pub unchanged: Vec<String>,
    /// İndirilen bayt (devam edilen kısım hariç).
    pub downloaded_bytes: u64,
    /// Kurulamayan kaynaklar ve nedeni; diğer kaynaklar yine kurulur.
    pub failed: Vec<SourceFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceFailure {
    pub source: String,
    pub files: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct Fetcher {
    client: reqwest::Client,
    hf_base: String,
    /// İlk yeniden denemeden önceki bekleme; her denemede ikiye katlanır.
    retry_delay: Duration,
}

fn dl_err(url: &str) -> impl Fn(reqwest::Error) -> AiError + '_ {
    move |e| AiError::Download { url: url.to_owned(), reason: e.to_string() }
}

fn io(what: &str, path: &Path) -> impl FnOnce(std::io::Error) -> AiError {
    let msg = format!("{what} ({})", path.display());
    move |e| AiError::Io(format!("{msg}: {e}"))
}

impl Fetcher {
    /// Gerçek kaynaklar: [`http_client`] + huggingface.co.
    pub fn new() -> Result<Self, AiError> {
        Ok(Self::with_client(http_client()?, HF_BASE))
    }

    /// Testler: yerel sunucu ve sahte Hugging Face API'si.
    #[must_use]
    pub fn with_client(client: reqwest::Client, hf_base: &str) -> Self {
        Self { client, hf_base: hf_base.trim_end_matches('/').to_owned(), retry_delay: Duration::from_secs(2) }
    }

    #[must_use]
    pub const fn with_retry_delay(mut self, d: Duration) -> Self {
        self.retry_delay = d;
        self
    }

    /// Hugging Face LFS dosyasının boyutu ve SHA-256'sı (indirmeden).
    pub async fn resolve_hf(&self, s: &Source) -> Result<LockEntry, AiError> {
        let hf = s.hf.as_ref().ok_or_else(|| AiError::InvalidInput(format!("{}: hf alanı yok", s.id)))?;
        let (dir, _) = hf.path.rsplit_once('/').unwrap_or(("", &hf.path));
        let url = format!("{}/api/models/{}/tree/{}/{}", self.hf_base, hf.repo, hf.revision, dir);
        let res = self.client.get(&url).send().await.map_err(dl_err(&url))?;
        if !res.status().is_success() {
            return Err(AiError::Download { url, reason: format!("HTTP {}", res.status()) });
        }
        let body = res.bytes().await.map_err(dl_err(&url))?;
        let entries: Vec<serde_json::Value> = serde_json::from_slice(&body)
            .map_err(|e| AiError::Download { url: url.clone(), reason: format!("beklenmeyen yanıt: {e}") })?;
        let e = entries
            .iter()
            .find(|e| e["path"] == hf.path.as_str())
            .ok_or_else(|| AiError::Download { url: url.clone(), reason: format!("{} listede yok", hf.path) })?;
        let lfs = &e["lfs"];
        let (Some(oid), Some(size)) = (lfs["oid"].as_str(), lfs["size"].as_u64()) else {
            return Err(AiError::Download { url, reason: format!("{} LFS dosyası değil: özet yok", hf.path) });
        };
        crate::models::parse_hex32(oid)?;
        Ok(LockEntry { size, sha256: oid.to_ascii_lowercase(), revision: hf.revision.clone() })
    }

    /// Eksik ya da bozuk her dosyayı indirir, doğrular ve yerine koyar.
    pub async fn install(
        &self,
        manifest: &Manifest,
        dir: &Path,
        progress: ProgressFn,
        cancel: Arc<AtomicBool>,
    ) -> Result<InstallReport, AiError> {
        tokio::fs::create_dir_all(dir).await.map_err(io("model dizini", dir))?;
        let mut lock = Lock::load(dir);
        let mut report = InstallReport::default();
        for src in &manifest.sources {
            let files: Vec<_> = manifest.files.iter().filter(|f| f.source == src.id).collect();
            let mut todo = Vec::new();
            for f in &files {
                let (path, exp) = (dir.join(&f.file), manifest.file_expected(f, &lock));
                let state = tokio::task::spawn_blocking(move || verify(&path, exp))
                    .await
                    .map_err(|e| AiError::Io(e.to_string()))??;
                if state == FileState::Verified {
                    report.unchanged.push(f.file.clone());
                } else {
                    todo.push(*f);
                }
            }
            if todo.is_empty() {
                continue;
            }
            if cancel.load(Ordering::Relaxed) {
                return Err(AiError::Cancelled);
            }
            match self.install_source(manifest, src, &todo, dir, &mut lock, &progress, &cancel).await {
                Ok(n) => {
                    report.downloaded_bytes += n;
                    report.installed.extend(todo.iter().map(|f| f.file.clone()));
                }
                Err(AiError::Cancelled) => return Err(AiError::Cancelled),
                Err(e) => report.failed.push(SourceFailure {
                    source: src.id.clone(),
                    files: todo.iter().map(|f| f.file.clone()).collect(),
                    reason: e.to_string(),
                }),
            }
        }
        Ok(report)
    }

    #[allow(clippy::too_many_arguments)]
    async fn install_source(
        &self,
        manifest: &Manifest,
        src: &Source,
        todo: &[&ModelFile],
        dir: &Path,
        lock: &mut Lock,
        progress: &ProgressFn,
        cancel: &AtomicBool,
    ) -> Result<u64, AiError> {
        let expected = match manifest.source_expected(src, lock) {
            Some(e) => e,
            None => {
                progress(Progress { source: src.id.clone(), phase: "resolve", done: 0, total: 0 });
                let entry = self.resolve_hf(src).await?;
                lock.sources.insert(src.id.clone(), entry);
                lock.save(dir)?;
                lock.get(&src.id).ok_or_else(|| AiError::Io("kilit dosyası".into()))?
            }
        };
        let part = partial_dir(dir)?.join(format!("{}.part", src.id));
        let fetched = self.download(src, &part, expected, progress, cancel).await?;

        for f in todo {
            let exp = manifest
                .file_expected(f, lock)
                .ok_or_else(|| AiError::InvalidInput(format!("{}: beklenen özet yok", f.file)))?;
            match (src.archive, &f.member) {
                (Some(kind), Some(member)) => {
                    progress(Progress { source: src.id.clone(), phase: "extract", done: 0, total: exp.size });
                    let tmp = partial_dir(dir)?.join(format!("{}.tmp", f.file));
                    let (a, m, t, d, name) =
                        (part.clone(), member.clone(), tmp.clone(), dir.to_path_buf(), f.file.clone());
                    tokio::task::spawn_blocking(move || {
                        extract_member(&a, kind, &m, &t, exp)?;
                        commit(&d, &t, &name)
                    })
                    .await
                    .map_err(|e| AiError::Io(e.to_string()))??;
                }
                _ => {
                    // Düz kaynak: indirilen dosya doğrulandı, olduğu gibi yerine konur.
                    let (p, d, name) = (part.clone(), dir.to_path_buf(), f.file.clone());
                    tokio::task::spawn_blocking(move || commit(&d, &p, &name))
                        .await
                        .map_err(|e| AiError::Io(e.to_string()))??;
                }
            }
        }
        let _ignored = tokio::fs::remove_file(&part).await;
        progress(Progress { source: src.id.clone(), phase: "done", done: expected.size, total: expected.size });
        Ok(fetched)
    }

    /// `part`'ı tamamlar ve doğrular; bu çağrıda ağdan gelen bayt sayısını döndürür.
    async fn download(
        &self,
        src: &Source,
        part: &PathBuf,
        exp: Expected,
        progress: &ProgressFn,
        cancel: &AtomicBool,
    ) -> Result<u64, AiError> {
        let mut fetched = 0u64;
        let mut last_err = None;
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(self.retry_delay * (1u32 << (attempt - 1))).await;
            }
            match self.download_once(src, part, exp, progress, cancel, &mut fetched).await {
                Ok(()) => {
                    progress(Progress { source: src.id.clone(), phase: "verify", done: exp.size, total: exp.size });
                    let p = part.clone();
                    let (n, sha) = tokio::task::spawn_blocking(move || sha256_file(&p))
                        .await
                        .map_err(|e| AiError::Io(e.to_string()))??;
                    if n == exp.size && sha == exp.sha256 {
                        return Ok(fetched);
                    }
                    let _ignored = tokio::fs::remove_file(part).await;
                    return Err(AiError::Verify {
                        file: src.id.clone(),
                        expected: format!("{} bayt, sha256 {}", exp.size, hex(&exp.sha256)),
                        actual: format!("{n} bayt, sha256 {}", hex(&sha)),
                    });
                }
                // Bozuk içerik ve iptal yeniden denenmez.
                Err(e @ (AiError::Verify { .. } | AiError::Cancelled)) => return Err(e),
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or(AiError::Cancelled))
    }

    async fn download_once(
        &self,
        src: &Source,
        part: &Path,
        exp: Expected,
        progress: &ProgressFn,
        cancel: &AtomicBool,
        fetched: &mut u64,
    ) -> Result<(), AiError> {
        let mut have = tokio::fs::metadata(part).await.map(|m| m.len()).unwrap_or(0);
        if have > exp.size {
            tokio::fs::remove_file(part).await.map_err(io("silinemedi", part))?;
            have = 0;
        }
        if have == exp.size {
            return Ok(());
        }
        let mut req = self.client.get(&src.url);
        if have > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let res = req.send().await.map_err(dl_err(&src.url))?;
        let status = res.status();
        let resumed = status == reqwest::StatusCode::PARTIAL_CONTENT && have > 0;
        if !status.is_success() {
            return Err(AiError::Download { url: src.url.clone(), reason: format!("HTTP {status}") });
        }
        let mut file = if resumed {
            tokio::fs::OpenOptions::new().append(true).open(part).await.map_err(io("açılamadı", part))?
        } else {
            have = 0;
            tokio::fs::File::create(part).await.map_err(io("oluşturulamadı", part))?
        };
        let mut stream = res.bytes_stream();
        let mut n = have;
        while let Some(chunk) = stream.next().await {
            if cancel.load(Ordering::Relaxed) {
                file.flush().await.map_err(io("yazılamadı", part))?;
                return Err(AiError::Cancelled);
            }
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    // Gelen baytlar diskte kalsın: sonraki deneme Range ile sürer.
                    file.flush().await.map_err(io("yazılamadı", part))?;
                    return Err(dl_err(&src.url)(e));
                }
            };
            n += chunk.len() as u64;
            if n > exp.size {
                drop(file);
                let _ignored = tokio::fs::remove_file(part).await;
                return Err(AiError::Verify {
                    file: src.id.clone(),
                    expected: format!("{} bayt", exp.size),
                    actual: format!("en az {n} bayt"),
                });
            }
            file.write_all(&chunk).await.map_err(io("yazılamadı", part))?;
            *fetched += chunk.len() as u64;
            progress(Progress { source: src.id.clone(), phase: "download", done: n, total: exp.size });
        }
        file.sync_all().await.map_err(io("fsync", part))?;
        if n < exp.size {
            // Bağlantı erken kapandı: sonraki deneme Range ile sürer.
            return Err(AiError::Download { url: src.url.clone(), reason: format!("{n}/{} baytta kesildi", exp.size) });
        }
        Ok(())
    }
}

/// rustls (ring) + webpki kökleri. `SSL_CERT_FILE` verilmişse içindeki
/// sertifikalar da güvenilir sayılır (kurumsal vekil sunucu).
pub fn http_client() -> Result<reqwest::Client, AiError> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(path) = std::env::var_os("SSL_CERT_FILE") {
        use rustls::pki_types::CertificateDer;
        use rustls::pki_types::pem::PemObject;
        if let Ok(iter) = CertificateDer::pem_file_iter(&path) {
            for cert in iter.flatten() {
                let _ignored = roots.add(cert);
            }
        }
    }
    let tls = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| AiError::Io(e.to_string()))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    reqwest::Client::builder()
        .use_preconfigured_tls(tls)
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .user_agent(concat!("loupe-models/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| AiError::Io(e.to_string()))
}

/// `loupe-models pin`: Hugging Face kaynaklarını çözüp manifesti sabit
/// özetlerle yeniden yazar (depoya işlemek için).
pub async fn pin(fetcher: &Fetcher, manifest: &Manifest) -> Result<Manifest, AiError> {
    let mut out = manifest.clone();
    for s in &mut out.sources {
        if s.sha256.is_none() {
            let e = fetcher.resolve_hf(s).await?;
            s.size = Some(e.size);
            s.sha256 = Some(e.sha256);
            // Sabitlenmiş kaynağın adresi de aynı sürüme işaret etmeli.
            if let Some(hf) = s.hf.take() {
                s.url = format!("{}/{}/resolve/{}/{}", HF_BASE, hf.repo, hf.revision, hf.path);
            }
        }
    }
    out.validate()?;
    Ok(out)
}

/// Sha256 yardımcı (testler ve CLI için).
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

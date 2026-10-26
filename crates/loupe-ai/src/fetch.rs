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
//! - `external_data` grafiğinin ağırlıkları grafikten sonra, grafikle aynı
//!   Hugging Face dizininden tek tek indirilir; eksik olanlar sonraki
//!   çalıştırmada tamamlanır (doğrulanmış ağırlık yeniden indirilmez).

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
    Expected, ExternalEntry, FileState, HfRef, Lock, LockEntry, Manifest, ModelFile, Source, commit, extract_member,
    file_state, git_blob_sha1, hex, onnx_external_locations, parse_hex32, partial_dir, partial_path, sha256_file,
    verify, verify_external, write_atomic,
};

pub const HF_BASE: &str = "https://huggingface.co";
const ATTEMPTS: usize = 4;
/// LFS'te olmayan (düz git) dosyalar Hugging Face'te en çok 10 MB'tır; bellekte indirilir.
const MAX_INLINE_BYTES: u64 = 16 << 20;

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

    /// Hugging Face dizin listesi (`/api/models/<repo>/tree/<sürüm>/<dizin>`);
    /// uzun listeler `Link: <…>; rel="next"` ile sayfalanır.
    async fn hf_tree(&self, hf: &HfRef, dir: &str) -> Result<Vec<serde_json::Value>, AiError> {
        let mut url = format!("{}/api/models/{}/tree/{}/{}", self.hf_base, hf.repo, hf.revision, dir);
        let mut entries = Vec::new();
        for _page in 0..64 {
            let res = self.client.get(&url).send().await.map_err(dl_err(&url))?;
            if !res.status().is_success() {
                return Err(AiError::Download { url, reason: format!("HTTP {}", res.status()) });
            }
            let next = res
                .headers()
                .get(reqwest::header::LINK)
                .and_then(|v| v.to_str().ok())
                .and_then(next_link)
                .map(|n| if n.starts_with('/') { format!("{}{n}", self.hf_base) } else { n });
            let body = res.bytes().await.map_err(dl_err(&url))?;
            let page: Vec<serde_json::Value> = serde_json::from_slice(&body)
                .map_err(|e| AiError::Download { url: url.clone(), reason: format!("beklenmeyen yanıt: {e}") })?;
            entries.extend(page);
            match next {
                Some(n) => url = n,
                None => return Ok(entries),
            }
        }
        Err(AiError::Download { url, reason: "liste çok uzun".into() })
    }

    /// Hugging Face LFS dosyasının boyutu ve SHA-256'sı (indirmeden).
    pub async fn resolve_hf(&self, s: &Source) -> Result<LockEntry, AiError> {
        let hf = s.hf.as_ref().ok_or_else(|| AiError::InvalidInput(format!("{}: hf alanı yok", s.id)))?;
        let (dir, _) = hf.path.rsplit_once('/').unwrap_or(("", &hf.path));
        let url = format!("{}/api/models/{}/tree/{}/{}", self.hf_base, hf.repo, hf.revision, dir);
        let entries = self.hf_tree(hf, dir).await?;
        let e = entries
            .iter()
            .find(|e| e["path"] == hf.path.as_str())
            .ok_or_else(|| AiError::Download { url: url.clone(), reason: format!("{} listede yok", hf.path) })?;
        let lfs = &e["lfs"];
        let (Some(oid), Some(size)) = (lfs["oid"].as_str(), lfs["size"].as_u64()) else {
            return Err(AiError::Download { url, reason: format!("{} LFS dosyası değil: özet yok", hf.path) });
        };
        parse_hex32(oid)?;
        Ok(LockEntry { size, sha256: oid.to_ascii_lowercase(), revision: hf.revision.clone(), external: None })
    }

    /// Grafiğin istediği ağırlık dosyalarının boyut ve özetleri (grafikle aynı dizinden).
    async fn resolve_external(
        &self,
        hf: &HfRef,
        locations: &[String],
    ) -> Result<std::collections::BTreeMap<String, ExternalEntry>, AiError> {
        let (dir, _) = hf.path.rsplit_once('/').unwrap_or(("", &hf.path));
        let entries = self.hf_tree(hf, dir).await?;
        let mut out = std::collections::BTreeMap::new();
        for loc in locations {
            let path = if dir.is_empty() { loc.clone() } else { format!("{dir}/{loc}") };
            let e = entries.iter().find(|e| e["path"] == path.as_str()).ok_or_else(|| AiError::Download {
                url: format!("{}/{}", hf.repo, path),
                reason: "grafiğin istediği ağırlık dosyası depoda yok".into(),
            })?;
            let lfs = &e["lfs"];
            let entry = match (lfs["oid"].as_str(), lfs["size"].as_u64(), e["oid"].as_str(), e["size"].as_u64()) {
                (Some(oid), Some(size), _, _) => {
                    parse_hex32(oid)?;
                    ExternalEntry { size, sha256: Some(oid.to_ascii_lowercase()), git_sha1: None }
                }
                (None, _, Some(oid), Some(size))
                    if oid.len() == 40 && oid.bytes().all(|b| b.is_ascii_hexdigit()) && size <= MAX_INLINE_BYTES =>
                {
                    ExternalEntry { size, sha256: None, git_sha1: Some(oid.to_ascii_lowercase()) }
                }
                _ => {
                    return Err(AiError::Download {
                        url: format!("{}/{}", hf.repo, path),
                        reason: "ağırlık dosyasının özeti yok".into(),
                    });
                }
            };
            out.insert(loc.clone(), entry);
        }
        Ok(out)
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
                let (m, f2, d, l) = (manifest.clone(), (*f).clone(), dir.to_path_buf(), lock.clone());
                let state = tokio::task::spawn_blocking(move || file_state(&m, &f2, &d, &l))
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
        // Grafiği doğru, yalnızca ağırlıkları eksik dosyalar için kaynak yeniden indirilmez.
        let mut graph_ok = true;
        for f in todo {
            let (path, exp) = (dir.join(&f.file), manifest.file_expected(f, lock));
            let state = tokio::task::spawn_blocking(move || verify(&path, exp))
                .await
                .map_err(|e| AiError::Io(e.to_string()))??;
            graph_ok &= f.external_data && state == FileState::Verified;
        }
        let mut fetched = 0;
        if !graph_ok {
            fetched += self.fetch_source(manifest, src, todo, dir, lock, expected, progress, cancel).await?;
        }
        for f in todo.iter().filter(|f| f.external_data) {
            fetched += self.install_external(src, f, dir, lock, progress, cancel).await?;
        }
        progress(Progress { source: src.id.clone(), phase: "done", done: expected.size, total: expected.size });
        Ok(fetched)
    }

    /// Kaynağı indirir, doğrular ve dosyaları (arşivse üyeleri) yerine koyar.
    #[allow(clippy::too_many_arguments)]
    async fn fetch_source(
        &self,
        manifest: &Manifest,
        src: &Source,
        todo: &[&ModelFile],
        dir: &Path,
        lock: &mut Lock,
        expected: Expected,
        progress: &ProgressFn,
        cancel: &AtomicBool,
    ) -> Result<u64, AiError> {
        let part = partial_dir(dir)?.join(format!("{}.part", src.id));
        let fetched = self.download(&src.id, &src.url, &part, expected, progress, cancel).await?;

        for f in todo {
            let exp = manifest
                .file_expected(f, lock)
                .ok_or_else(|| AiError::InvalidInput(format!("{}: beklenen özet yok", f.file)))?;
            match (src.archive, &f.member) {
                (Some(kind), Some(member)) => {
                    progress(Progress { source: src.id.clone(), phase: "extract", done: 0, total: exp.size });
                    let tmp = partial_path(dir, &f.file, "tmp")?;
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
        // Yeni grafik: ağırlık listesi yeniden çözülmeli.
        if todo.iter().any(|f| f.external_data) {
            if let Some(e) = lock.sources.get_mut(&src.id) {
                e.external = None;
            }
            lock.save(dir)?;
        }
        Ok(fetched)
    }

    /// `external_data` grafiğinin eksik ağırlıklarını indirir ve grafiğin
    /// yanına koyar. Konumlar grafikten okunur; özetler Hugging Face'ten.
    async fn install_external(
        &self,
        src: &Source,
        f: &ModelFile,
        dir: &Path,
        lock: &mut Lock,
        progress: &ProgressFn,
        cancel: &AtomicBool,
    ) -> Result<u64, AiError> {
        let hf = src.hf.as_ref().ok_or_else(|| AiError::InvalidInput(format!("{}: hf alanı yok", src.id)))?;
        let graph = dir.join(&f.file);
        let locations = tokio::task::spawn_blocking(move || onnx_external_locations(&graph))
            .await
            .map_err(|e| AiError::Io(e.to_string()))??;
        let known = lock.external(&src.id).is_some_and(|m| locations.iter().all(|l| m.contains_key(l)));
        if !known {
            progress(Progress { source: src.id.clone(), phase: "resolve", done: 0, total: 0 });
            let map = self.resolve_external(hf, &locations).await?;
            let entry = lock.sources.get_mut(&src.id).ok_or_else(|| AiError::Io("kilit dosyası".into()))?;
            entry.external = Some(map);
            lock.save(dir)?;
        }
        let map = lock.external(&src.id).cloned().unwrap_or_default();
        let sub = Path::new(&f.file).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let (hf_dir, _) = hf.path.rsplit_once('/').unwrap_or(("", &hf.path));
        let total: u64 = map.values().map(|e| e.size).sum();
        let (mut offset, mut fetched) = (0u64, 0u64);
        for (name, e) in &map {
            let rel = if sub.is_empty() { name.clone() } else { format!("{sub}/{name}") };
            let (path, e2) = (dir.join(&rel), e.clone());
            let state = tokio::task::spawn_blocking(move || verify_external(&path, &e2, true))
                .await
                .map_err(|e| AiError::Io(e.to_string()))??;
            if state != FileState::Verified {
                if cancel.load(Ordering::Relaxed) {
                    return Err(AiError::Cancelled);
                }
                let remote = if hf_dir.is_empty() { name.clone() } else { format!("{hf_dir}/{name}") };
                let url = format!("{}/{}/resolve/{}/{}", self.hf_base, hf.repo, hf.revision, remote);
                fetched += self.fetch_external(&src.id, &url, dir, &rel, e, offset, total, progress, cancel).await?;
            }
            offset += e.size;
            progress(Progress { source: src.id.clone(), phase: "download", done: offset, total });
        }
        Ok(fetched)
    }

    #[allow(clippy::too_many_arguments)]
    async fn fetch_external(
        &self,
        id: &str,
        url: &str,
        dir: &Path,
        rel: &str,
        e: &ExternalEntry,
        offset: u64,
        total: u64,
        progress: &ProgressFn,
        cancel: &AtomicBool,
    ) -> Result<u64, AiError> {
        if let Some(sha) = &e.sha256 {
            let exp = Expected { size: e.size, sha256: parse_hex32(sha)? };
            let outer = progress.clone();
            let shifted: ProgressFn = Arc::new(move |mut p: Progress| {
                p.done += offset;
                p.total = total;
                outer(p);
            });
            let part = partial_path(dir, &format!("{id}/{rel}"), "part")?;
            let n = self.download(id, url, &part, exp, &shifted, cancel).await?;
            let (d, p, r) = (dir.to_path_buf(), part.clone(), rel.to_owned());
            tokio::task::spawn_blocking(move || commit(&d, &p, &r)).await.map_err(|e| AiError::Io(e.to_string()))??;
            return Ok(n);
        }
        // Düz git dosyası (küçük): bellekte indirilir, git blob SHA-1'i ile doğrulanır.
        let want = e.git_sha1.as_deref().ok_or_else(|| AiError::InvalidInput(format!("{rel}: özet yok")))?;
        let mut last_err = None;
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(self.retry_delay * (1u32 << (attempt - 1))).await;
            }
            let res = match self.client.get(url).send().await {
                Ok(r) if r.status().is_success() => r,
                Ok(r) => {
                    last_err = Some(AiError::Download { url: url.to_owned(), reason: format!("HTTP {}", r.status()) });
                    continue;
                }
                Err(err) => {
                    last_err = Some(dl_err(url)(err));
                    continue;
                }
            };
            let body = match res.bytes().await {
                Ok(b) => b,
                Err(err) => {
                    last_err = Some(dl_err(url)(err));
                    continue;
                }
            };
            let got = hex(&git_blob_sha1(&body));
            if body.len() as u64 != e.size || got != want {
                return Err(AiError::Verify {
                    file: rel.to_owned(),
                    expected: format!("{} bayt, git sha1 {want}", e.size),
                    actual: format!("{} bayt, git sha1 {got}", body.len()),
                });
            }
            let (d, r) = (dir.to_path_buf(), rel.to_owned());
            tokio::task::spawn_blocking(move || write_atomic(&d, &r, &body))
                .await
                .map_err(|e| AiError::Io(e.to_string()))??;
            return Ok(e.size);
        }
        Err(last_err.unwrap_or(AiError::Cancelled))
    }

    /// `part`'ı tamamlar ve doğrular; bu çağrıda ağdan gelen bayt sayısını döndürür.
    async fn download(
        &self,
        id: &str,
        url: &str,
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
            match self.download_once(id, url, part, exp, progress, cancel, &mut fetched).await {
                Ok(()) => {
                    progress(Progress { source: id.to_owned(), phase: "verify", done: exp.size, total: exp.size });
                    let p = part.clone();
                    let (n, sha) = tokio::task::spawn_blocking(move || sha256_file(&p))
                        .await
                        .map_err(|e| AiError::Io(e.to_string()))??;
                    if n == exp.size && sha == exp.sha256 {
                        return Ok(fetched);
                    }
                    let _ignored = tokio::fs::remove_file(part).await;
                    return Err(AiError::Verify {
                        file: id.to_owned(),
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

    #[allow(clippy::too_many_arguments)]
    async fn download_once(
        &self,
        id: &str,
        url: &str,
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
        let mut req = self.client.get(url);
        if have > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let res = req.send().await.map_err(dl_err(url))?;
        let status = res.status();
        let resumed = status == reqwest::StatusCode::PARTIAL_CONTENT && have > 0;
        if !status.is_success() {
            return Err(AiError::Download { url: url.to_owned(), reason: format!("HTTP {status}") });
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
                    return Err(dl_err(url)(e));
                }
            };
            n += chunk.len() as u64;
            if n > exp.size {
                drop(file);
                let _ignored = tokio::fs::remove_file(part).await;
                return Err(AiError::Verify {
                    file: id.to_owned(),
                    expected: format!("{} bayt", exp.size),
                    actual: format!("en az {n} bayt"),
                });
            }
            file.write_all(&chunk).await.map_err(io("yazılamadı", part))?;
            *fetched += chunk.len() as u64;
            progress(Progress { source: id.to_owned(), phase: "download", done: n, total: exp.size });
        }
        file.sync_all().await.map_err(io("fsync", part))?;
        if n < exp.size {
            // Bağlantı erken kapandı: sonraki deneme Range ile sürer.
            return Err(AiError::Download { url: url.to_owned(), reason: format!("{n}/{} baytta kesildi", exp.size) });
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

/// `Link` başlığındaki `rel="next"` adresi.
fn next_link(h: &str) -> Option<String> {
    h.split(',').find(|p| p.contains("rel=\"next\"")).and_then(|p| {
        let (a, b) = (p.find('<')?, p.find('>')?);
        p.get(a + 1..b).map(str::to_owned)
    })
}

/// `loupe-models pin`: Hugging Face kaynaklarını çözüp manifesti sabit
/// özetlerle yeniden yazar (depoya işlemek için).
pub async fn pin(fetcher: &Fetcher, manifest: &Manifest) -> Result<Manifest, AiError> {
    let mut out = manifest.clone();
    for s in &mut out.sources {
        // Harici ağırlıklı grafiğin ağırlıkları Hugging Face dizininden çözülür: kaynak hf kalır.
        let external = manifest.files.iter().any(|f| f.source == s.id && f.external_data);
        if s.sha256.is_none() && !external {
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

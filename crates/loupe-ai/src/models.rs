//! Model yöneticisi (RFC-001G §3): hangi dosyanın nereden geleceği, beklenen
//! boyutu ve SHA-256 özeti; diskteki dosyanın doğrulanması ve arşivden
//! (zip / tar.gz) tek üye çıkarma. Ağ kısmı [`crate::fetch`]'tedir.
//!
//! Manifest ikiliye gömülüdür (`models.json`). Kaynak iki biçimde sabitlenir:
//! - **Sabit özet:** `size` + `sha256` manifestte yazılı; indirilen bayt bir
//!   bit bile farklıysa dosya reddedilir.
//! - **Hugging Face LFS:** `hf` alanı; özet ve boyut indirmeden hemen önce
//!   Hugging Face API'sinden (LFS `oid` = içeriğin SHA-256'sı) alınır ve
//!   `manifest.lock.json`'a yazılır. Sonraki doğrulamalar kilit dosyasına göre
//!   yapılır. `loupe-models pin` bu değerleri manifest biçiminde yazdırır.
//!
//! **Harici ağırlıklı ONNX** (`external_data`): 2 GB'ı aşan modeller (M-CLIP
//! metin kodlayıcısı) ağırlıklarını grafiğin yanındaki ayrı dosyalarda tutar.
//! Grafik indirildikten sonra içindeki `TensorProto.external_data` konumları
//! okunur, her biri aynı Hugging Face dizininden indirilir ve grafikle aynı alt
//! dizine konur (ONNX Runtime konumları grafiğin dizinine göre çözer). Grafik
//! tek başına doğrulansa bile ağırlıkların hepsi yerinde değilse model
//! [`FileState::Incomplete`] sayılır.

use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};

use crate::AiError;

/// Gömülü manifest.
pub const BUILTIN_MANIFEST: &str = include_str!("../models.json");
/// Hugging Face'ten çözülen özetlerin model dizinindeki kaydı.
pub const LOCK_FILE: &str = "manifest.lock.json";
/// Yarım indirmeler ve çıkarılan geçici dosyalar (aynı dosya sisteminde: `rename` atomik).
pub const PARTIAL_DIR: &str = ".partial";
const READ_BUF: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HfRef {
    pub repo: String,
    pub revision: String,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Archive {
    Zip,
    Tgz,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub id: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf: Option<HfRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<Archive>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelFile {
    /// Model dizinindeki ad.
    pub file: String,
    /// `face_detect` | `face_embed` | `clip_image` | `clip_text` | `tokenizer` | `runtime`
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dim: Option<usize>,
    pub source: String,
    /// Arşiv içindeki yol (kaynak arşivse zorunlu).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// ONNX grafiği; ağırlıkları kaynağın Hugging Face dizinindeki ayrı
    /// dosyalarda. Dosya bir alt dizinde olmalı (`mclip_text/model.onnx`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub external_data: bool,
    /// Yalnızca bu işletim sisteminde gerekli (`std::env::consts::OS`:
    /// `linux` | `macos` | `windows`); yoksa her yerde.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub sources: Vec<Source>,
    pub files: Vec<ModelFile>,
}

/// Beklenen içerik: boyut + SHA-256.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expected {
    pub size: u64,
    pub sha256: [u8; 32],
}

/// Diskteki dosyanın durumu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileState {
    /// Yok.
    Missing,
    /// Boyut ve SHA-256 tutuyor.
    Verified,
    /// Var ama boyut ya da özet tutmuyor: kullanılmaz, yeniden indirilir.
    Corrupt,
    /// Var; beklenen özet henüz bilinmiyor (Hugging Face'ten çözülmemiş).
    Unpinned,
    /// Grafik doğru ama harici ağırlık dosyalarından biri eksik ya da yarım:
    /// model yüklenemez, kurulum eksikleri indirir.
    Incomplete,
}

/// Varsayılan model dizini: uygulama veri dizininin altındaki `models/`
/// (Linux'ta `~/.local/share/loupe/models`). Uygulama aynı yolu kullanır.
#[must_use]
pub fn default_dir() -> Option<PathBuf> {
    platform_dirs().map(|(data, _)| data.join("models"))
}

/// Uygulamanın veri ve önbellek dizinleri.
///
/// - Linux: `$XDG_DATA_HOME/loupe` (`~/.local/share/loupe`), `$XDG_CACHE_HOME/loupe` (`~/.cache/loupe`)
/// - macOS: `~/Library/Application Support/Loupe`, `~/Library/Caches/Loupe`
/// - Windows: `%LOCALAPPDATA%\Loupe`, `%LOCALAPPDATA%\Loupe\cache`
#[must_use]
pub fn platform_dirs() -> Option<(PathBuf, PathBuf)> {
    let env_dir = |var: &str| std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute());
    if cfg!(windows) {
        let base = env_dir("LOCALAPPDATA")
            .or_else(|| env_dir("USERPROFILE").map(|h| h.join("AppData").join("Local")))?
            .join("Loupe");
        return Some((base.clone(), base.join("cache")));
    }
    let home = env_dir("HOME");
    if cfg!(target_os = "macos") {
        let lib = home?.join("Library");
        return Some((lib.join("Application Support").join("Loupe"), lib.join("Caches").join("Loupe")));
    }
    let data = env_dir("XDG_DATA_HOME").or_else(|| home.as_ref().map(|h| h.join(".local/share")))?;
    let cache = env_dir("XDG_CACHE_HOME").or_else(|| home.as_ref().map(|h| h.join(".cache")))?;
    Some((data.join("loupe"), cache.join("loupe")))
}

pub fn invalid(m: impl Into<String>) -> AiError {
    AiError::InvalidInput(m.into())
}

fn io(what: &str, path: &Path) -> impl FnOnce(std::io::Error) -> AiError {
    let msg = format!("{what} ({})", path.display());
    move |e| AiError::Io(format!("{msg}: {e}"))
}

pub fn parse_hex32(s: &str) -> Result<[u8; 32], AiError> {
    let s = s.trim();
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid(format!("SHA-256 64 haneli hex olmalı: {s:?}")));
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).map_err(|e| invalid(e.to_string()))?;
    }
    Ok(out)
}

#[must_use]
pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Model dizininde yalnızca düz dosya adı (yol bileşeni, gizli dosya yok).
fn safe_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\', '\0']) && name.len() <= 128
}

/// Model dizinine göre yol: düz ad ya da tek alt dizin (`mclip_text/model.onnx`).
fn safe_rel(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    parts.len() <= 2 && parts.iter().all(|p| safe_name(p))
}

/// `.partial/` içinde düz ad (alt dizinli model yolları için).
fn flat(name: &str) -> String {
    name.replace('/', "__")
}

/// İndirme adresi: https; yalnızca testlerde döngü adresine http.
fn allowed_url(url: &str) -> bool {
    if url.starts_with("https://") {
        return true;
    }
    let Some(rest) = url.strip_prefix("http://") else { return false };
    rest.starts_with("[::1]") || matches!(rest.split(['/', ':']).next(), Some("127.0.0.1" | "localhost"))
}

impl Manifest {
    /// Gömülü manifest, bu işletim sistemine göre süzülmüş.
    pub fn builtin() -> Result<Self, AiError> {
        Ok(Self::parse(BUILTIN_MANIFEST)?.for_os(std::env::consts::OS))
    }

    /// Başka işletim sistemine ait dosyaları ve yalnızca onlara hizmet eden
    /// kaynakları atar (örn. Windows'ta `libonnxruntime.so`).
    #[must_use]
    pub fn for_os(mut self, os: &str) -> Self {
        self.files.retain(|f| f.os.as_deref().is_none_or(|o| o == os));
        let used: HashSet<String> = self.files.iter().map(|f| f.source.clone()).collect();
        self.sources.retain(|s| used.contains(&s.id));
        self
    }

    pub fn parse(json: &str) -> Result<Self, AiError> {
        let m: Self = serde_json::from_str(json).map_err(|e| invalid(format!("model manifesti: {e}")))?;
        m.validate()?;
        Ok(m)
    }

    /// Yapısal denetim: indirmeye başlamadan önce manifest kendi içinde tutarlı mı.
    pub fn validate(&self) -> Result<(), AiError> {
        if self.version != 1 {
            return Err(invalid(format!("desteklenmeyen manifest sürümü {}", self.version)));
        }
        let mut ids = HashSet::new();
        for s in &self.sources {
            if s.id.is_empty() || !ids.insert(s.id.as_str()) {
                return Err(invalid(format!("kaynak kimliği boş ya da tekrar: {:?}", s.id)));
            }
            if !allowed_url(&s.url) {
                return Err(invalid(format!("{}: adres https olmalı", s.id)));
            }
            match (&s.sha256, s.size, &s.hf) {
                (Some(h), Some(n), _) if n > 0 => {
                    parse_hex32(h)?;
                }
                (None, None, Some(hf)) if !hf.repo.is_empty() && !hf.path.is_empty() && !hf.revision.is_empty() => {}
                _ => return Err(invalid(format!("{}: ya size + sha256 ya da hf gerekli", s.id))),
            }
        }
        let mut names = HashSet::new();
        for f in &self.files {
            if !safe_rel(&f.file) || !names.insert(f.file.as_str()) {
                return Err(invalid(format!("geçersiz ya da tekrar eden dosya adı: {:?}", f.file)));
            }
            let src =
                self.source(&f.source).ok_or_else(|| invalid(format!("{}: bilinmeyen kaynak {}", f.file, f.source)))?;
            match (src.archive, &f.member) {
                (Some(_), Some(m)) if !m.is_empty() && !m.split('/').any(|c| c == "..") => {
                    let (Some(h), Some(n)) = (&f.sha256, f.size) else {
                        return Err(invalid(format!("{}: arşiv üyesinin size + sha256'sı gerekli", f.file)));
                    };
                    if n == 0 {
                        return Err(invalid(format!("{}: boyut sıfır", f.file)));
                    }
                    parse_hex32(h)?;
                }
                (None, None) if f.sha256.is_none() && f.size.is_none() => {
                    if self.files.iter().filter(|o| o.source == f.source).count() > 1 {
                        return Err(invalid(format!("{}: arşiv olmayan kaynak tek dosya verir", f.source)));
                    }
                }
                _ => return Err(invalid(format!("{}: member yalnızca arşiv kaynağında olur", f.file))),
            }
            if f.os.as_deref().is_some_and(|o| !matches!(o, "linux" | "macos" | "windows")) {
                return Err(invalid(format!("{}: bilinmeyen işletim sistemi {:?}", f.file, f.os)));
            }
            if f.external_data && (src.hf.is_none() || src.archive.is_some() || !f.file.contains('/')) {
                return Err(invalid(format!(
                    "{}: external_data yalnızca Hugging Face kaynağında ve alt dizindeki dosyada olur",
                    f.file
                )));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn source(&self, id: &str) -> Option<&Source> {
        self.sources.iter().find(|s| s.id == id)
    }

    #[must_use]
    pub fn file(&self, name: &str) -> Option<&ModelFile> {
        self.files.iter().find(|f| f.file == name)
    }

    /// Kaynağın beklenen içeriği: manifestte sabitse o, değilse kilit dosyasındaki.
    #[must_use]
    pub fn source_expected(&self, s: &Source, lock: &Lock) -> Option<Expected> {
        match (&s.sha256, s.size) {
            (Some(h), Some(size)) => parse_hex32(h).ok().map(|sha256| Expected { size, sha256 }),
            _ => lock.get(&s.id),
        }
    }

    /// Model dizinindeki dosyanın beklenen içeriği.
    #[must_use]
    pub fn file_expected(&self, f: &ModelFile, lock: &Lock) -> Option<Expected> {
        match (&f.sha256, f.size) {
            (Some(h), Some(size)) => parse_hex32(h).ok().map(|sha256| Expected { size, sha256 }),
            _ => self.source(&f.source).and_then(|s| self.source_expected(s, lock)),
        }
    }

    /// Diskte kaplayacağı toplam boyut: grafik + bilinen harici ağırlıklar.
    #[must_use]
    pub fn file_total_size(&self, f: &ModelFile, lock: &Lock) -> Option<u64> {
        let graph = self.file_expected(f, lock)?.size;
        let ext = lock.external(&f.source).map_or(0, |m| m.values().map(|e| e.size).sum());
        Some(graph + ext)
    }
}

/// `manifest.lock.json`: kaynak kimliği → çözülen boyut ve SHA-256.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lock {
    pub sources: BTreeMap<String, LockEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockEntry {
    pub size: u64,
    pub sha256: String,
    /// Çözüldüğü Hugging Face sürümü (ör. `main`).
    pub revision: String,
    /// `external_data` grafiğinin ağırlık dosyaları (ad → içerik). `None`:
    /// henüz çözülmedi; grafik değişince sıfırlanır.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<BTreeMap<String, ExternalEntry>>,
}

/// Harici ağırlık dosyası. Hugging Face'te büyük dosyalar LFS'tedir (özet
/// SHA-256); küçükler düz git nesnesidir ve yalnızca git blob SHA-1'i bilinir.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalEntry {
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_sha1: Option<String>,
}

impl Lock {
    /// Okunamayan ya da bozuk kilit dosyası boş sayılır (özetler yeniden çözülür).
    #[must_use]
    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(dir.join(LOCK_FILE))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> Result<(), AiError> {
        let json = serde_json::to_vec_pretty(self).map_err(|e| AiError::Io(e.to_string()))?;
        write_atomic(dir, LOCK_FILE, &json)
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<Expected> {
        let e = self.sources.get(id)?;
        Some(Expected { size: e.size, sha256: parse_hex32(&e.sha256).ok()? })
    }

    #[must_use]
    pub fn external(&self, id: &str) -> Option<&BTreeMap<String, ExternalEntry>> {
        self.sources.get(id)?.external.as_ref()
    }
}

/// Akışlı SHA-256 + boyut.
pub fn sha256_file(path: &Path) -> Result<(u64, [u8; 32]), AiError> {
    let mut f = File::open(path).map_err(io("açılamadı", path))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; READ_BUF];
    let mut n = 0u64;
    loop {
        let r = f.read(&mut buf).map_err(io("okunamadı", path))?;
        if r == 0 {
            break;
        }
        h.update(&buf[..r]);
        n += r as u64;
    }
    Ok((n, h.finalize().into()))
}

/// Dosya beklenenle aynı mı. Boyut tutmuyorsa özet hiç hesaplanmaz.
pub fn verify(path: &Path, expected: Option<Expected>) -> Result<FileState, AiError> {
    let Ok(meta) = std::fs::metadata(path) else { return Ok(FileState::Missing) };
    if !meta.is_file() {
        return Ok(FileState::Corrupt);
    }
    let Some(exp) = expected else { return Ok(FileState::Unpinned) };
    if meta.len() != exp.size {
        return Ok(FileState::Corrupt);
    }
    let (_, sha) = sha256_file(path)?;
    Ok(if sha == exp.sha256 { FileState::Verified } else { FileState::Corrupt })
}

/// Git blob SHA-1'i: `sha1("blob <boyut>\0" ‖ içerik)`.
#[must_use]
pub fn git_blob_sha1(bytes: &[u8]) -> [u8; 20] {
    let mut h = Sha1::new();
    h.update(format!("blob {}\0", bytes.len()).as_bytes());
    h.update(bytes);
    h.finalize().into()
}

fn git_blob_sha1_file(path: &Path, size: u64) -> Result<[u8; 20], AiError> {
    let mut f = File::open(path).map_err(io("açılamadı", path))?;
    let mut h = Sha1::new();
    h.update(format!("blob {size}\0").as_bytes());
    let mut buf = vec![0u8; READ_BUF];
    loop {
        let r = f.read(&mut buf).map_err(io("okunamadı", path))?;
        if r == 0 {
            break;
        }
        h.update(&buf[..r]);
    }
    Ok(h.finalize().into())
}

/// Harici ağırlık dosyası beklenenle aynı mı. `full` değilse yalnızca boyut
/// (arayüzün sık yoklaması); kurulum ve `verify` tam özet karşılaştırır.
pub fn verify_external(path: &Path, e: &ExternalEntry, full: bool) -> Result<FileState, AiError> {
    let Ok(meta) = std::fs::metadata(path) else { return Ok(FileState::Missing) };
    if !meta.is_file() || meta.len() != e.size {
        return Ok(FileState::Corrupt);
    }
    if !full {
        return Ok(FileState::Verified);
    }
    let ok = match (&e.sha256, &e.git_sha1) {
        (Some(h), _) => sha256_file(path)?.1 == parse_hex32(h)?,
        (None, Some(h)) => hex(&git_blob_sha1_file(path, e.size)?) == h.to_ascii_lowercase(),
        (None, None) => return Ok(FileState::Unpinned),
    };
    Ok(if ok { FileState::Verified } else { FileState::Corrupt })
}

/// `external_data` dosyasının ağırlıkları: dosya böyle değilse `None`; kilitte
/// çözülmemişse ya da biri eksik, yarım veya bozuksa `Incomplete`.
pub fn external_state(f: &ModelFile, dir: &Path, lock: &Lock, full: bool) -> Result<Option<FileState>, AiError> {
    if !f.external_data {
        return Ok(None);
    }
    let Some(map) = lock.external(&f.source) else { return Ok(Some(FileState::Incomplete)) };
    let base = dir.join(&f.file);
    let base = base.parent().unwrap_or(dir);
    for (name, e) in map {
        if verify_external(&base.join(name), e, full)? != FileState::Verified {
            return Ok(Some(FileState::Incomplete));
        }
    }
    Ok(Some(FileState::Verified))
}

/// Dosyanın tam durumu: SHA-256 ve (varsa) harici ağırlıkların hepsi.
pub fn file_state(manifest: &Manifest, f: &ModelFile, dir: &Path, lock: &Lock) -> Result<FileState, AiError> {
    let state = verify(&dir.join(&f.file), manifest.file_expected(f, lock))?;
    if state != FileState::Verified {
        return Ok(state);
    }
    Ok(external_state(f, dir, lock, true)?.unwrap_or(state))
}

/// Bir model dosyasının durumu (arayüz ve `loupe-models status`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileStatus {
    pub file: String,
    pub role: String,
    pub state: FileState,
    /// Beklenen boyut (bilinmiyorsa `None`).
    pub size: Option<u64>,
    pub sha256: Option<String>,
    pub license: Option<String>,
}

/// Manifestteki her dosyanın diskteki durumu.
pub fn status(manifest: &Manifest, dir: &Path) -> Result<Vec<FileStatus>, AiError> {
    let lock = Lock::load(dir);
    manifest
        .files
        .iter()
        .map(|f| {
            let exp = manifest.file_expected(f, &lock);
            Ok(FileStatus {
                file: f.file.clone(),
                role: f.role.clone(),
                state: file_state(manifest, f, dir, &lock)?,
                size: manifest.file_total_size(f, &lock),
                sha256: exp.map(|e| hex(&e.sha256)),
                license: manifest.source(&f.source).and_then(|s| s.license.clone()),
            })
        })
        .collect()
}

/// `dir/.partial/<ad>.tmp`'ye yazar, fsync eder, `dir/<ad>` üzerine `rename`
/// eder ve dizini fsync eder: yarım dosya hiçbir zaman asıl adla görünmez.
pub fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), AiError> {
    let tmp = partial_dir(dir)?.join(format!("{}.tmp", flat(name)));
    let mut f = File::create(&tmp).map_err(io("yazılamadı", &tmp))?;
    f.write_all(bytes).map_err(io("yazılamadı", &tmp))?;
    f.sync_all().map_err(io("fsync", &tmp))?;
    drop(f);
    commit(dir, &tmp, name)
}

pub fn partial_dir(dir: &Path) -> Result<PathBuf, AiError> {
    let p = dir.join(PARTIAL_DIR);
    std::fs::create_dir_all(&p).map_err(io("dizin oluşturulamadı", &p))?;
    Ok(p)
}

/// Doğrulanmış geçici dosyayı yerine koyar (`name` alt dizinli olabilir).
pub fn commit(dir: &Path, tmp: &Path, name: &str) -> Result<(), AiError> {
    let dst = dir.join(name);
    let parent = dst.parent().unwrap_or(dir);
    std::fs::create_dir_all(parent).map_err(io("dizin oluşturulamadı", parent))?;
    std::fs::rename(tmp, &dst).map_err(io("yerine konamadı", &dst))?;
    fsync_dir(parent)
}

/// Rename'in diske yazılması için dizini fsync eder. Windows'ta dizin `File`
/// olarak açılamaz (os error 5); NTFS üst veriyi kendi günlüğüyle korur.
fn fsync_dir(dir: &Path) -> Result<(), AiError> {
    #[cfg(unix)]
    {
        File::open(dir).and_then(|d| d.sync_all()).map_err(io("dizin fsync", dir))
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Ok(())
    }
}

/// `.partial/` içinde bu model dosyası için geçici ad.
pub fn partial_path(dir: &Path, name: &str, ext: &str) -> Result<PathBuf, AiError> {
    Ok(partial_dir(dir)?.join(format!("{}.{ext}", flat(name))))
}

/// Harici ağırlık taşıyan grafik en çok bu kadar olur (ağırlıklar dışarıda).
const MAX_GRAPH_BYTES: u64 = 256 << 20;

/// ONNX grafiğinin (`ModelProto`) başlatıcılarındaki harici veri konumları,
/// ilk görülme sırasıyla. Yalnızca protobuf'un ihtiyaç duyulan alanları okunur:
/// `ModelProto.graph` (7) → `GraphProto.initializer` (5) →
/// `TensorProto.external_data` (13) → `StringStringEntryProto` (1 = key, 2 = value).
pub fn onnx_external_locations(path: &Path) -> Result<Vec<String>, AiError> {
    let len = std::fs::metadata(path).map_err(io("okunamadı", path))?.len();
    if len > MAX_GRAPH_BYTES {
        return Err(invalid(format!("{}: harici ağırlıklı grafik için fazla büyük", path.display())));
    }
    let bytes = std::fs::read(path).map_err(io("okunamadı", path))?;
    let mut out: Vec<String> = Vec::new();
    for graph in pb_fields(&bytes, 7)? {
        for tensor in pb_fields(graph, 5)? {
            for entry in pb_fields(tensor, 13)? {
                let key = pb_fields(entry, 1)?;
                if key.first().copied() != Some(b"location".as_slice()) {
                    continue;
                }
                let Some(value) = pb_fields(entry, 2)?.into_iter().next() else { continue };
                let loc = std::str::from_utf8(value).map_err(|_| invalid("onnx: konum UTF-8 değil"))?;
                if !safe_name(loc) {
                    return Err(invalid(format!("onnx: güvensiz harici veri konumu {loc:?}")));
                }
                if !out.iter().any(|o| o == loc) {
                    out.push(loc.to_owned());
                }
            }
        }
    }
    Ok(out)
}

fn pb_varint(b: &[u8], p: &mut usize) -> Result<u64, AiError> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *b.get(*p).ok_or_else(|| invalid("onnx: kesik varint"))?;
        *p += 1;
        v |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(v);
        }
    }
    Err(invalid("onnx: bozuk varint"))
}

/// Mesajdaki `field` numaralı uzunluk-önekli alanların gövdeleri.
fn pb_fields(b: &[u8], field: u64) -> Result<Vec<&[u8]>, AiError> {
    let mut out = Vec::new();
    let mut p = 0usize;
    while p < b.len() {
        let tag = pb_varint(b, &mut p)?;
        match tag & 7 {
            0 => {
                pb_varint(b, &mut p)?;
            }
            1 => p += 8,
            2 => {
                let n = usize::try_from(pb_varint(b, &mut p)?).map_err(|_| invalid("onnx: bozuk uzunluk"))?;
                let end = p.checked_add(n).filter(|&e| e <= b.len()).ok_or_else(|| invalid("onnx: kesik alan"))?;
                if tag >> 3 == field {
                    out.push(&b[p..end]);
                }
                p = end;
            }
            5 => p += 4,
            w => return Err(invalid(format!("onnx: desteklenmeyen protobuf tel türü {w}"))),
        }
    }
    if p > b.len() {
        return Err(invalid("onnx: kesik mesaj"));
    }
    Ok(out)
}

/// Arşivden tek üyeyi `out`'a çıkarır; yazarken SHA-256 hesaplar ve beklenenle
/// karşılaştırır. Tutmazsa `out` silinir.
pub fn extract_member(archive: &Path, kind: Archive, member: &str, out: &Path, exp: Expected) -> Result<(), AiError> {
    let result = match kind {
        Archive::Zip => extract_zip(archive, member, out),
        Archive::Tgz => extract_tgz(archive, member, out),
    }
    .and_then(|(n, sha)| {
        if n != exp.size || sha != exp.sha256 {
            Err(AiError::Verify {
                file: member.to_owned(),
                expected: format!("{} bayt, sha256 {}", exp.size, hex(&exp.sha256)),
                actual: format!("{n} bayt, sha256 {}", hex(&sha)),
            })
        } else {
            Ok(())
        }
    });
    if result.is_err() {
        let _ignored = std::fs::remove_file(out);
    }
    result
}

/// Okunanı hem dosyaya yazar hem özetler.
fn copy_hashing(mut r: impl Read, out: &Path, limit: u64) -> Result<(u64, [u8; 32]), AiError> {
    let mut f = File::create(out).map_err(io("yazılamadı", out))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; READ_BUF];
    let mut n = 0u64;
    loop {
        let r = r.read(&mut buf).map_err(io("arşiv okunamadı", out))?;
        if r == 0 {
            break;
        }
        n += r as u64;
        if n > limit {
            return Err(invalid("arşiv üyesi bildirilen boyuttan büyük"));
        }
        h.update(&buf[..r]);
        f.write_all(&buf[..r]).map_err(io("yazılamadı", out))?;
    }
    f.sync_all().map_err(io("fsync", out))?;
    Ok((n, h.finalize().into()))
}

fn u16le(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32le(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Zip: merkez dizinden üyeyi bulur (yerel başlıktaki boyutlar veri
/// tanımlayıcısı yüzünden sıfır olabilir), saklı ya da deflate.
fn extract_zip(archive: &Path, member: &str, out: &Path) -> Result<(u64, [u8; 32]), AiError> {
    let mut f = File::open(archive).map_err(io("açılamadı", archive))?;
    let len = f.metadata().map_err(io("okunamadı", archive))?.len();
    let tail_len = len.min(65_557);
    f.seek(SeekFrom::Start(len - tail_len)).map_err(io("okunamadı", archive))?;
    let mut tail = vec![0u8; usize::try_from(tail_len).map_err(|e| invalid(e.to_string()))?];
    f.read_exact(&mut tail).map_err(io("okunamadı", archive))?;
    let eocd = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&i| u32le(&tail, i) == 0x0605_4b50)
        .ok_or_else(|| invalid("zip: merkez dizin sonu yok"))?;
    let entries = u16le(&tail, eocd + 10);
    let cd_size = u32le(&tail, eocd + 12);
    let cd_off = u32le(&tail, eocd + 16);
    if cd_off == u32::MAX || entries == u16::MAX {
        return Err(invalid("zip64 desteklenmiyor"));
    }
    f.seek(SeekFrom::Start(u64::from(cd_off))).map_err(io("okunamadı", archive))?;
    let mut cd = vec![0u8; cd_size as usize];
    f.read_exact(&mut cd).map_err(io("okunamadı", archive))?;
    let mut p = 0usize;
    for _ in 0..entries {
        if p + 46 > cd.len() || u32le(&cd, p) != 0x0201_4b50 {
            return Err(invalid("zip: bozuk merkez dizin"));
        }
        let method = u16le(&cd, p + 10);
        let csize = u64::from(u32le(&cd, p + 20));
        let usize_ = u64::from(u32le(&cd, p + 24));
        let (nlen, xlen, clen) =
            (usize::from(u16le(&cd, p + 28)), usize::from(u16le(&cd, p + 30)), usize::from(u16le(&cd, p + 32)));
        let local = u64::from(u32le(&cd, p + 42));
        let name = cd.get(p + 46..p + 46 + nlen).ok_or_else(|| invalid("zip: bozuk ad"))?;
        p += 46 + nlen + xlen + clen;
        if name != member.as_bytes() {
            continue;
        }
        let mut lh = [0u8; 30];
        f.seek(SeekFrom::Start(local)).map_err(io("okunamadı", archive))?;
        f.read_exact(&mut lh).map_err(io("okunamadı", archive))?;
        if u32le(&lh, 0) != 0x0403_4b50 {
            return Err(invalid("zip: bozuk yerel başlık"));
        }
        let data = local + 30 + u64::from(u16le(&lh, 26)) + u64::from(u16le(&lh, 28));
        f.seek(SeekFrom::Start(data)).map_err(io("okunamadı", archive))?;
        let raw = (&mut f).take(csize);
        return match method {
            0 => copy_hashing(raw, out, usize_),
            8 => copy_hashing(flate2::read::DeflateDecoder::new(raw), out, usize_),
            m => Err(invalid(format!("zip: desteklenmeyen sıkıştırma {m}"))),
        };
    }
    Err(invalid(format!("arşivde yok: {member}")))
}

/// tar.gz: 512 baytlık ustar başlıkları; yalnızca düz dosya üyesi.
fn extract_tgz(archive: &Path, member: &str, out: &Path) -> Result<(u64, [u8; 32]), AiError> {
    let f = File::open(archive).map_err(io("açılamadı", archive))?;
    let mut r = flate2::read::GzDecoder::new(std::io::BufReader::new(f));
    let mut hdr = [0u8; 512];
    loop {
        r.read_exact(&mut hdr).map_err(io("tar okunamadı", archive))?;
        if hdr.iter().all(|&b| b == 0) {
            return Err(invalid(format!("arşivde yok: {member}")));
        }
        let field = |a: usize, b: usize| {
            let s = &hdr[a..b];
            let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
            String::from_utf8_lossy(&s[..end]).into_owned()
        };
        let size_str = field(124, 136);
        let size = u64::from_str_radix(size_str.trim(), 8).map_err(|_| invalid("tar: bozuk boyut"))?;
        let prefix = field(345, 500);
        let name = if prefix.is_empty() { field(0, 100) } else { format!("{prefix}/{}", field(0, 100)) };
        let kind = hdr[156];
        let padded = size.div_ceil(512) * 512;
        if (kind == b'0' || kind == 0) && name == member {
            let res = copy_hashing((&mut r).take(size), out, size)?;
            return Ok(res);
        }
        std::io::copy(&mut (&mut r).take(padded), &mut std::io::sink()).map_err(io("tar okunamadı", archive))?;
    }
}

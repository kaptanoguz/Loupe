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

use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
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
}

/// Varsayılan model dizini: `$XDG_DATA_HOME/loupe/models` (yoksa
/// `~/.local/share/loupe/models`). Uygulama aynı yolu kullanır.
#[must_use]
pub fn default_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("loupe").join("models"))
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

/// İndirme adresi: https; yalnızca testlerde döngü adresine http.
fn allowed_url(url: &str) -> bool {
    if url.starts_with("https://") {
        return true;
    }
    let Some(rest) = url.strip_prefix("http://") else { return false };
    rest.starts_with("[::1]") || matches!(rest.split(['/', ':']).next(), Some("127.0.0.1" | "localhost"))
}

impl Manifest {
    pub fn builtin() -> Result<Self, AiError> {
        Self::parse(BUILTIN_MANIFEST)
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
            if !safe_name(&f.file) || !names.insert(f.file.as_str()) {
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
                state: verify(&dir.join(&f.file), exp)?,
                size: exp.map(|e| e.size),
                sha256: exp.map(|e| hex(&e.sha256)),
                license: manifest.source(&f.source).and_then(|s| s.license.clone()),
            })
        })
        .collect()
}

/// `dir/.partial/<ad>.tmp`'ye yazar, fsync eder, `dir/<ad>` üzerine `rename`
/// eder ve dizini fsync eder: yarım dosya hiçbir zaman asıl adla görünmez.
pub fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), AiError> {
    let tmp = partial_dir(dir)?.join(format!("{name}.tmp"));
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

/// Doğrulanmış geçici dosyayı yerine koyar.
pub fn commit(dir: &Path, tmp: &Path, name: &str) -> Result<(), AiError> {
    let dst = dir.join(name);
    std::fs::rename(tmp, &dst).map_err(io("yerine konamadı", &dst))?;
    File::open(dir).and_then(|d| d.sync_all()).map_err(io("dizin fsync", dir))
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

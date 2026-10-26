//! Basit düzenleme: tarifin katalogda saklanması, düzenlenmiş küçük resimler,
//! JPEG dışa aktarma ve "Darktable/GIMP'te aç".
//!
//! - Tarif ([`Recipe`]) medyanın güncel sürümünün (`versions`, ad "Ana") işlem
//!   satırlarıdır (`edits`). Her kayıtta satırlar yeniden yazılır; tetikleyici
//!   `versions.revision`'ı artırır. Orijinale dönmek satırları siler.
//! - Düzenlenmiş görüntü `thumb://<sınıf>/<hash>?e=<revizyon>` ile istenir
//!   (`orig` dahil). İlk istekte `lg` küçük resimden (1:1 için orijinalden)
//!   üretilir ve `cache/edits/<hh>/<hash>-<sınıf>-r<rev>.jpg` olarak saklanır.
//!   Revizyon adresin parçası olduğu için yanıt `immutable` işaretlenebilir.
//! - Tarif ayrıca `.loupe.json`'a yazılır (disk çevrimdışıysa yalnızca katalogda kalır).

use std::borrow::Cow;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use http::header::{ACCESS_CONTROL_ALLOW_ORIGIN, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, HeaderValue};
use http::{Method, Request, Response, StatusCode, Uri};
use image::RgbImage;
use loupe_core::{Blake3Hash, MediaFileType, MediaId, now_ms};
use loupe_db::Pool;
use loupe_jobs::{JobKind, JobQueue, NewJob, Priority};
use loupe_media::edit::{AutoTone, Recipe};
use loupe_media::protocol::CACHE_IMMUTABLE;
use loupe_media::{Sidecar, ThumbCache, ThumbSize};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::library::Libraries;
use crate::{AppError, AppResult};

/// Dışa aktarma JPEG kalitesi (varsayılan).
pub const EXPORT_QUALITY: u8 = 92;

/// Düzenleme ekranının ve kaydın döndürdüğü durum.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditStateDto {
    pub id: i64,
    /// Küçük resim adresindeki `?e=`; düzenlenmemişse 0.
    pub revision: i64,
    pub recipe: Recipe,
    /// Düzenlenmiş en/boy oranı (ızgara düzeni için).
    pub aspect: f32,
    /// `.loupe.json`: `written` | `skipped` (disk çevrimdışı) | `failed`
    pub sidecar: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportDto {
    pub path: String,
    pub width: u32,
    pub height: u32,
    /// Orijinal çözülemedi (ör. libvips olmadan HEIC): `lg` küçük resimden üretildi.
    pub low_res: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalEditorDto {
    pub id: &'static str,
    pub name: &'static str,
}

/// `thumb://…?e=<rev>`: düzenlenmiş sürüm isteniyorsa revizyon.
#[must_use]
pub fn requested_revision(uri: &Uri) -> Option<i64> {
    uri.query()?.split('&').find_map(|kv| kv.strip_prefix("e=")).and_then(|v| v.parse::<i64>().ok()).filter(|&r| r > 0)
}

/// İstenen görüntü: küçük resim sınıfı ya da tam çözünürlük.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Thumb(ThumbSize),
    Orig,
}

impl Target {
    const fn name(self) -> &'static str {
        match self {
            Self::Thumb(s) => s.as_str(),
            Self::Orig => "orig",
        }
    }

    const fn quality(self) -> u8 {
        match self {
            Self::Thumb(ThumbSize::Sm) => 80,
            Self::Thumb(ThumbSize::Md) => 85,
            Self::Thumb(ThumbSize::Lg) => 90,
            Self::Orig => 92,
        }
    }
}

struct Source {
    id: MediaId,
    file_type: MediaFileType,
    orientation: Option<u8>,
    path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct Editor {
    pool: Arc<Pool>,
    libraries: Arc<Libraries>,
    queue: Arc<JobQueue>,
    thumbs: ThumbCache,
    dir: PathBuf,
}

/// Güncel sürüm ve revizyonu; sürüm yoksa `None`.
fn current_version(c: &Connection, id: MediaId) -> AppResult<Option<(i64, i64)>> {
    Ok(c.query_row("SELECT id, revision FROM versions WHERE media_id = ?1 AND is_current = 1", [id.0], |r| {
        Ok((r.get(0)?, r.get(1)?))
    })
    .optional()?)
}

/// Medyanın tarifi ve revizyonu (düzenlenmemişse varsayılan tarif, 0).
pub fn load(c: &Connection, id: MediaId) -> AppResult<(Recipe, i64)> {
    let Some((version, revision)) = current_version(c, id)? else { return Ok((Recipe::default(), 0)) };
    let rows: Vec<(String, String)> = c
        .prepare_cached("SELECT op, params FROM edits WHERE version_id = ?1 AND is_enabled = 1 ORDER BY seq")?
        .query_map([version], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    if rows.is_empty() {
        return Ok((Recipe::default(), 0));
    }
    let parsed: Vec<(String, serde_json::Value)> = rows
        .into_iter()
        .map(|(op, p)| {
            Ok((op, serde_json::from_str(&p).map_err(|e| AppError::Invalid(format!("düzenleme satırı: {e}")))?))
        })
        .collect::<AppResult<_>>()?;
    Ok((Recipe::from_ops(parsed.iter().map(|(o, p)| (o.as_str(), p))), revision))
}

fn base_aspect(c: &Connection, id: MediaId) -> AppResult<f32> {
    let (w, h, o): (Option<i64>, Option<i64>, Option<i64>) = c
        .query_row("SELECT width, height, orientation FROM media_files WHERE id = ?1", [id.0], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?
        .ok_or(AppError::NotFound { entity: "medya", id: id.0 })?;
    Ok(crate::timeline::aspect(w, h, o))
}

impl Editor {
    #[must_use]
    pub fn new(
        pool: Arc<Pool>,
        libraries: Arc<Libraries>,
        queue: Arc<JobQueue>,
        thumbs: ThumbCache,
        dir: PathBuf,
    ) -> Self {
        Self { pool, libraries, queue, thumbs, dir }
    }

    fn state(&self, id: MediaId, sidecar: &'static str) -> AppResult<EditStateDto> {
        self.pool
            .read(|c| {
                let (recipe, revision) = load(c, id).map_err(to_db)?;
                let aspect = recipe.aspect(base_aspect(c, id).map_err(to_db)?);
                Ok(EditStateDto { id: id.0, revision, recipe, aspect, sidecar })
            })
            .map_err(AppError::from)
    }

    pub fn get(&self, id: i64) -> AppResult<EditStateDto> {
        let id = MediaId(id);
        self.source(id)?;
        self.state(id, "skipped")
    }

    /// Tarifi kaydeder. Düzenlenmemiş tarif (her şey sıfır) orijinale dönmektir.
    pub fn save(&self, id: i64, recipe: Recipe) -> AppResult<EditStateDto> {
        let id = MediaId(id);
        let src = self.source(id)?;
        if src.file_type == MediaFileType::Video {
            return Err(AppError::Invalid("videolar düzenlenemez".into()));
        }
        let recipe = recipe.normalized().map_err(AppError::Invalid)?;
        let hash = self.pool.write(|c| {
            let tx = c.transaction()?;
            let now = now_ms();
            let version = match current_version(&tx, id).map_err(to_db)? {
                Some((v, _)) => v,
                None if recipe.is_identity() => {
                    tx.commit()?;
                    return Ok(None);
                }
                None => {
                    tx.execute(
                        "INSERT INTO versions (media_id, is_current, created_at, updated_at) VALUES (?1, 1, ?2, ?2)",
                        params![id.0, now],
                    )?;
                    tx.last_insert_rowid()
                }
            };
            tx.execute("DELETE FROM edits WHERE version_id = ?1", [version])?;
            for (seq, (op, p)) in recipe.to_ops().into_iter().enumerate() {
                tx.execute(
                    "INSERT INTO edits (version_id, seq, op, params, engine_version, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![version, seq as i64, op, p.to_string(), loupe_media::edit::ENGINE_VERSION, now],
                )?;
            }
            tx.execute("UPDATE versions SET updated_at = ?2 WHERE id = ?1", params![version, now])?;
            let hash: Vec<u8> = tx.query_row("SELECT blake3 FROM media_files WHERE id = ?1", [id.0], |r| r.get(0))?;
            tx.commit()?;
            Ok(Some(hash))
        })?;
        if let Some(hash) = hash.and_then(|h| <[u8; 32]>::try_from(h).ok()) {
            self.purge(&Blake3Hash(hash));
        }
        let sidecar = match &src.path {
            None => "skipped",
            Some(p) => match Sidecar::for_media(p).write_edits(recipe.to_sidecar()) {
                Ok(()) => "written",
                Err(_) => "failed",
            },
        };
        self.state(id, sidecar)
    }

    pub fn revert(&self, id: i64) -> AppResult<EditStateDto> {
        self.save(id, Recipe::default())
    }

    /// Tek tıkla iyileştirme önerisi (`md` küçük resimden; kaydetmez).
    pub fn auto(&self, id: i64) -> AppResult<AutoTone> {
        let id = MediaId(id);
        let hash = self.hash_of(id)?;
        let img = [ThumbSize::Md, ThumbSize::Lg, ThumbSize::Sm]
            .into_iter()
            .find_map(|s| self.thumbs.lookup(&hash, s))
            .ok_or_else(|| AppError::Invalid("küçük resim henüz hazır değil".into()))
            .and_then(|(p, _)| decode_file(&p))?;
        Ok(loupe_media::edit::auto_tone(&img))
    }

    /// Güncel tarifle tam çözünürlüklü JPEG yazar (atomik). Hedef orijinalin
    /// kendisi olamaz; uzantı yoksa `.jpg` eklenir.
    pub fn export(&self, id: i64, dest: &Path, quality: u8) -> AppResult<ExportDto> {
        let id = MediaId(id);
        let src = self.source(id)?;
        if src.file_type == MediaFileType::Video {
            return Err(AppError::Invalid("videolar dışa aktarılamaz".into()));
        }
        let dest = with_jpeg_extension(dest);
        if src.path.as_deref() == Some(dest.as_path()) {
            return Err(AppError::Invalid("orijinal dosyanın üzerine yazılamaz".into()));
        }
        let (recipe, _) = self.pool.read(|c| load(c, id).map_err(to_db))?;
        let (img, icc, low_res) = self.full_image(&src)?;
        let out = loupe_media::edit::render(img, &recipe, None);
        let exif = src.path.as_deref().and_then(loupe_media::edit::export_exif);
        let bytes = loupe_media::edit::encode_jpeg(&out, quality, icc, exif)?;
        crate::full::write_atomic(&dest, &bytes)?;
        Ok(ExportDto { path: dest.display().to_string(), width: out.width(), height: out.height(), low_res })
    }

    /// Dosyanın dışa aktarma önerisi: orijinalin klasörü ve `<ad>-edit.jpg`.
    pub fn export_suggestion(&self, id: i64) -> AppResult<(Option<PathBuf>, String)> {
        let src = self.source(MediaId(id))?;
        let name = self.pool.read(|c| {
            Ok(c.query_row("SELECT original_filename FROM media_files WHERE id = ?1", [id], |r| r.get::<_, String>(0))?)
        })?;
        let stem = Path::new(&name).file_stem().map_or_else(|| "loupe".into(), |s| s.to_string_lossy().into_owned());
        Ok((src.path.as_deref().and_then(Path::parent).map(Path::to_path_buf), format!("{stem}-edit.jpg")))
    }

    fn full_image(&self, src: &Source) -> AppResult<(RgbImage, Option<Vec<u8>>, bool)> {
        let path = src.path.as_deref().ok_or_else(|| AppError::LibraryOffline("dosyanın diski bağlı değil".into()))?;
        match loupe_media::edit::decode_full(path, src.file_type, src.orientation) {
            Ok((img, icc)) => Ok((img, icc, false)),
            Err(loupe_media::MediaError::Unsupported { .. }) => {
                let hash = self.hash_of(src.id)?;
                let (p, _) = self
                    .thumbs
                    .lookup(&hash, ThumbSize::Lg)
                    .ok_or_else(|| AppError::Invalid("bu biçim için önizleme henüz hazır değil".into()))?;
                Ok((decode_file(&p)?, None, true))
            }
            Err(e) => Err(e.into()),
        }
    }

    fn hash_of(&self, id: MediaId) -> AppResult<Blake3Hash> {
        self.pool
            .read(|c| loupe_db::repo::media::get(c, id))?
            .map(|m| m.blake3)
            .ok_or(AppError::NotFound { entity: "medya", id: id.0 })
    }

    fn source(&self, id: MediaId) -> AppResult<Source> {
        let m = self
            .pool
            .read(|c| loupe_db::repo::media::get(c, id))?
            .ok_or(AppError::NotFound { entity: "medya", id: id.0 })?;
        let orientation: Option<i64> = self
            .pool
            .read(|c| Ok(c.query_row("SELECT orientation FROM media_files WHERE id = ?1", [id.0], |r| r.get(0))?))?;
        Ok(Source {
            id,
            file_type: m.file_type,
            orientation: orientation.and_then(|o| u8::try_from(o).ok()),
            path: self.libraries.media_path(&m)?,
        })
    }

    fn cache_path(&self, hash: &Blake3Hash, target: Target, revision: i64) -> PathBuf {
        let hex = hash.to_hex();
        self.dir.join(&hex[..2]).join(format!("{hex}-{}-r{revision}.jpg", target.name()))
    }

    /// Bu medyanın eski revizyon önbelleklerini siler.
    fn purge(&self, hash: &Blake3Hash) {
        let hex = hash.to_hex();
        let Ok(entries) = std::fs::read_dir(self.dir.join(&hex[..2])) else { return };
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with(&hex) {
                let _ignored = std::fs::remove_file(e.path());
            }
        }
    }

    /// Düzenlenmiş görüntünün baytları. `Ok(None)`: düzenleme yok (çağıran
    /// düzenlenmemiş görüntüye düşer).
    fn bytes(&self, hash: &Blake3Hash, target: Target) -> AppResult<Option<(Vec<u8>, i64)>> {
        let id = self
            .pool
            .read(|c| loupe_db::repo::media::find_by_blake3(c, hash))?
            .ok_or(AppError::NotFound { entity: "medya", id: 0 })?;
        let (recipe, revision) = self.pool.read(|c| load(c, id).map_err(to_db))?;
        if revision == 0 {
            return Ok(None);
        }
        let cached = self.cache_path(hash, target, revision);
        if let Ok(b) = std::fs::read(&cached) {
            return Ok(Some((b, revision)));
        }
        let img = match target {
            Target::Orig => self.full_image(&self.source(id)?)?.0,
            Target::Thumb(size) => {
                // sm için md yeterli; md ve lg kırpmada çözünürlük kaybetmesin diye lg'den.
                let order: &[ThumbSize] = match size {
                    ThumbSize::Sm => &[ThumbSize::Md, ThumbSize::Lg, ThumbSize::Sm],
                    _ => &[ThumbSize::Lg, ThumbSize::Md],
                };
                let Some((p, _)) = order.iter().find_map(|&s| self.thumbs.lookup(hash, s)) else {
                    let _queued = self.queue.enqueue(&NewJob::media(JobKind::Thumb, id, "lg", Priority::Immediate));
                    return Err(AppError::NotFound { entity: "küçük resim", id: id.0 });
                };
                decode_file(&p)?
            }
        };
        let edge = match target {
            Target::Thumb(s) => Some(s.long_edge()),
            Target::Orig => None,
        };
        let out = loupe_media::edit::render(img, &recipe, edge);
        let bytes = loupe_media::edit::encode_jpeg(&out, target.quality(), None, None)?;
        crate::full::write_atomic(&cached, &bytes)?;
        Ok(Some((bytes, revision)))
    }

    /// `thumb://…?e=<rev>` isteği. Düzenleme yoksa (geri alındıysa) `None`:
    /// çağıran düzenlenmemiş görüntüyü verir.
    pub fn handle<B>(
        &self,
        req: &Request<B>,
        hash: &Blake3Hash,
        target: Target,
        wanted: i64,
    ) -> Option<Response<Cow<'static, [u8]>>> {
        let head = match *req.method() {
            Method::GET => false,
            Method::HEAD => true,
            _ => return Some(status(StatusCode::METHOD_NOT_ALLOWED)),
        };
        let (bytes, revision) = match self.bytes(hash, target) {
            Ok(Some(v)) => v,
            Ok(None) => return None,
            Err(AppError::NotFound { .. }) => return Some(status(StatusCode::NOT_FOUND)),
            Err(AppError::LibraryOffline(_)) => return Some(status(StatusCode::SERVICE_UNAVAILABLE)),
            Err(AppError::Media(_) | AppError::Invalid(_)) => return Some(status(StatusCode::UNPROCESSABLE_ENTITY)),
            Err(_) => return Some(status(StatusCode::INTERNAL_SERVER_ERROR)),
        };
        let len = bytes.len();
        let mut resp = Response::new(if head { Cow::Borrowed(&[][..]) } else { Cow::Owned(bytes) });
        let h = resp.headers_mut();
        h.insert(CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
        // Adresteki revizyon eskiyse güncel olan verilir ama saklanmaz.
        h.insert(
            CACHE_CONTROL,
            HeaderValue::from_static(if revision == wanted { CACHE_IMMUTABLE } else { "no-store" }),
        );
        h.insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
        h.insert(CONTENT_LENGTH, HeaderValue::from(len));
        Some(resp)
    }

    // ─────────────────────────── Dış düzenleyiciler ───────────────────────────

    /// Bu makinede kurulu düzenleyiciler (Darktable, GIMP).
    #[must_use]
    pub fn external_editors() -> Vec<ExternalEditorDto> {
        KNOWN.iter().filter(|k| launcher(k).is_some()).map(|k| ExternalEditorDto { id: k.id, name: k.name }).collect()
    }

    /// Orijinal dosyayı düzenleyicide açar (RAW, Darktable'da kendi işlemesiyle).
    pub fn open_in(&self, id: i64, editor: &str) -> AppResult<()> {
        let known = KNOWN
            .iter()
            .find(|k| k.id == editor)
            .ok_or_else(|| AppError::Invalid(format!("bilinmeyen düzenleyici: {editor}")))?;
        let (program, mut args) =
            launcher(known).ok_or_else(|| AppError::Invalid(format!("{} kurulu değil", known.name)))?;
        let path = self
            .source(MediaId(id))?
            .path
            .ok_or_else(|| AppError::LibraryOffline("dosyanın diski bağlı değil".into()))?;
        args.push(path.into_os_string());
        std::process::Command::new(&program)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(AppError::io(format!("{} çalıştırılamadı", known.name)))?;
        Ok(())
    }
}

fn status(code: StatusCode) -> Response<Cow<'static, [u8]>> {
    let mut resp = Response::new(Cow::Borrowed(&[][..]));
    *resp.status_mut() = code;
    resp.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

fn decode_file(path: &Path) -> AppResult<RgbImage> {
    let bytes = std::fs::read(path).map_err(AppError::io(format!("{}", path.display())))?;
    Ok(image::load_from_memory(&bytes).map_err(|e| AppError::Invalid(format!("görüntü çözülemedi: {e}")))?.to_rgb8())
}

fn with_jpeg_extension(p: &Path) -> PathBuf {
    let ok = p
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"));
    if ok {
        p.to_path_buf()
    } else {
        let mut s = p.as_os_str().to_owned();
        s.push(".jpg");
        PathBuf::from(s)
    }
}

fn to_db(e: AppError) -> loupe_db::DbError {
    match e {
        AppError::Db(d) => d,
        other => loupe_db::DbError::Validation(other.to_string()),
    }
}

struct Known {
    id: &'static str,
    name: &'static str,
    /// PATH'te aranan programlar.
    bins: &'static [&'static str],
    /// Flatpak uygulama kimliği (`exports/bin/<kimlik>` başlatıcısı).
    flatpak: &'static str,
    /// macOS `/Applications` altındaki paket adları.
    mac: &'static [&'static str],
    /// Windows `Program Files` altındaki klasör öneki ve `bin` içindeki program öneki.
    windows: (&'static str, &'static str),
}

const KNOWN: &[Known] = &[
    Known {
        id: "darktable",
        name: "Darktable",
        bins: &["darktable"],
        flatpak: "org.darktable.Darktable",
        mac: &["darktable.app"],
        windows: ("darktable", "darktable"),
    },
    Known {
        id: "gimp",
        name: "GIMP",
        bins: &["gimp", "gimp-3.0", "gimp-2.10"],
        flatpak: "org.gimp.GIMP",
        mac: &["GIMP.app", "GIMP-3.0.app", "GIMP-2.10.app"],
        windows: ("GIMP", "gimp"),
    },
];

/// Program ve dosya yolundan önceki argümanlar.
fn launcher(k: &Known) -> Option<(PathBuf, Vec<OsString>)> {
    if cfg!(target_os = "macos") {
        let app = k.mac.iter().map(|a| Path::new("/Applications").join(a)).find(|p| p.is_dir())?;
        return Some((PathBuf::from("open"), vec!["-a".into(), app.into_os_string()]));
    }
    if cfg!(windows) {
        let root = std::env::var_os("ProgramFiles").map_or_else(|| PathBuf::from(r"C:\Program Files"), PathBuf::from);
        let (dir_prefix, exe_prefix) = k.windows;
        let found = std::fs::read_dir(&root).ok()?.flatten().find_map(|d| {
            if !d.file_name().to_string_lossy().starts_with(dir_prefix) {
                return None;
            }
            std::fs::read_dir(d.path().join("bin")).ok()?.flatten().map(|e| e.path()).find(|p| {
                let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
                name.starts_with(exe_prefix) && name.ends_with(".exe") && !name.contains("console")
            })
        })?;
        return Some((found, Vec::new()));
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    if let Some(bin) = std::env::split_paths(&path).find_map(|d| k.bins.iter().map(|b| d.join(b)).find(|p| p.is_file()))
    {
        return Some((bin, Vec::new()));
    }
    let mut roots = vec![PathBuf::from("/var/lib/flatpak")];
    if let Some(home) = std::env::var_os("HOME") {
        roots.insert(0, PathBuf::from(home).join(".local/share/flatpak"));
    }
    roots.into_iter().map(|r| r.join("exports/bin").join(k.flatpak)).find(|p| p.is_file()).map(|p| (p, Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_comes_from_the_query() {
        let hex = "ab".repeat(32);
        assert_eq!(requested_revision(&format!("thumb://sm/{hex}?e=7").parse().unwrap()), Some(7));
        assert_eq!(requested_revision(&format!("thumb://sm/{hex}?r=2&e=3").parse().unwrap()), Some(3));
        assert_eq!(requested_revision(&format!("thumb://sm/{hex}?e=0").parse().unwrap()), None);
        assert_eq!(requested_revision(&format!("thumb://sm/{hex}").parse().unwrap()), None);
    }

    #[test]
    fn export_name_gets_a_jpeg_extension() {
        assert_eq!(with_jpeg_extension(Path::new("/a/b.JPG")), PathBuf::from("/a/b.JPG"));
        assert_eq!(with_jpeg_extension(Path::new("/a/b")), PathBuf::from("/a/b.jpg"));
        assert_eq!(with_jpeg_extension(Path::new("/a/b.png")), PathBuf::from("/a/b.png.jpg"));
    }
}

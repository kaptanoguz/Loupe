//! Medya hattının iş işleyicileri: `thumb`, `metadata_full` ve `geocode`.
//! Kuyruk iş türlerini bilmez; bu modül `loupe-media` ile `loupe-db`'yi
//! [`JobHandler`] üzerinden birbirine bağlar.
//!
//! `md` küçük resmi üretilince AI ardıl işleri kuyruğa girer: medya başına
//! `face_detect` ve `clip_embed` (Background), kütüphane başına tek
//! `cluster_events` (Idle, debounce'lu: içe aktarma bitince bir kez çalışır).
//! İşleyicileri `loupe-ai` sağlar; [`crate::JobRouter`] ile aynı havuza takılır.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use loupe_core::{MediaFileType, MediaId};
use loupe_db::models::MediaFile;
use loupe_db::repo::{location, media, thumbnail};
use loupe_db::{DbError, Pool};
use loupe_media::thumb::{SourceKind, webp_dimensions};
use loupe_media::{GeoIndex, MediaError, Place, ThumbSize, Thumbnailer, read_metadata};

use crate::model::{Job, JobError, JobKind, NewJob, Priority};
use crate::worker::{JobContext, JobHandler};

/// Kataloğdaki satırdan dosyanın bugünkü mutlak yolu. Disk takılı değilse
/// `None` döner ve iş geçici hatayla ertelenir.
pub type PathResolver = dyn Fn(&MediaFile) -> Option<PathBuf> + Send + Sync;

impl From<MediaError> for JobError {
    fn from(e: MediaError) -> Self {
        let code = match &e {
            MediaError::Io { .. } => "IO",
            MediaError::Exif { .. } => "EXIF",
            MediaError::Decode { .. } => "DECODE",
            MediaError::Encode(_) => "ENCODE",
            MediaError::Unsupported { .. } => "UNSUPPORTED",
            MediaError::NoEmbeddedPreview(_) => "NO_PREVIEW",
            MediaError::Sidecar { .. } => "SIDECAR",
            MediaError::GeoData { .. } => "GEODATA",
            MediaError::InvalidCoordinate { .. } => "BAD_COORDINATE",
            MediaError::Tool { .. } => "TOOL",
        };
        Self { code: code.to_owned(), message: e.to_string(), retryable: !e.is_permanent() }
    }
}

impl From<DbError> for JobError {
    fn from(e: DbError) -> Self {
        Self::transient("DB", e.to_string())
    }
}

pub struct MediaPipeline {
    pool: Arc<Pool>,
    thumbnailer: Thumbnailer,
    geo: Option<Arc<GeoIndex>>,
    resolve: Box<PathResolver>,
    ai_followups: bool,
    event_debounce: Duration,
}

impl std::fmt::Debug for MediaPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaPipeline")
            .field("thumbnailer", &self.thumbnailer)
            .field("geo", &self.geo.as_ref().map(|g| g.dataset_ver().to_owned()))
            .finish_non_exhaustive()
    }
}

impl MediaPipeline {
    #[must_use]
    pub fn new(
        pool: Arc<Pool>,
        thumbnailer: Thumbnailer,
        geo: Option<Arc<GeoIndex>>,
        resolve: impl Fn(&MediaFile) -> Option<PathBuf> + Send + Sync + 'static,
    ) -> Self {
        Self {
            pool,
            thumbnailer,
            geo,
            resolve: Box::new(resolve),
            ai_followups: true,
            event_debounce: Duration::from_secs(30),
        }
    }

    /// AI işleyicisi olmayan kurulumlar (ör. yalnızca küçük resim testleri) için.
    #[must_use]
    pub const fn without_ai_followups(mut self) -> Self {
        self.ai_followups = false;
        self
    }

    /// Son içe aktarmadan sonra etkinlik kümelemesinin ne kadar bekleyeceği.
    #[must_use]
    pub const fn with_event_debounce(mut self, d: Duration) -> Self {
        self.event_debounce = d;
        self
    }

    fn enqueue_ai(&self, media: MediaId, ctx: &JobContext<'_>) -> Result<(), JobError> {
        let jobs = [
            NewJob::media(JobKind::FaceDetect, media, "", Priority::Background),
            NewJob::media(JobKind::ClipEmbed, media, "", Priority::Background),
            NewJob::global(JobKind::ClusterEvents, Priority::Idle).with_delay(self.event_debounce),
        ];
        for j in &jobs {
            ctx.enqueue(j).map_err(|e| JobError::transient("ENQUEUE", e.to_string()))?;
        }
        Ok(())
    }

    fn media(&self, job: &Job) -> Result<(MediaFile, PathBuf), JobError> {
        let id = job.media_id.ok_or_else(|| JobError::permanent("NO_MEDIA", "iş bir medyaya bağlı değil"))?;
        let m = self
            .pool
            .read(|c| media::get(c, id))?
            .ok_or_else(|| JobError::permanent("MEDIA_GONE", format!("medya {} katalogda yok", id.0)))?;
        let path = (self.resolve)(&m)
            .ok_or_else(|| JobError::transient("VOLUME_OFFLINE", format!("{} şu an erişilemez", m.rel_path)))?;
        Ok((m, path))
    }

    fn thumb(&self, job: &Job, ctx: &JobContext<'_>) -> Result<(), JobError> {
        let size: ThumbSize = job
            .variant
            .parse()
            .map_err(|()| JobError::permanent("BAD_VARIANT", format!("geçersiz boyut: {}", job.variant)))?;
        let (m, path) = self.media(job)?;
        let orientation = self.pool.read(|c| media::render_hints(c, m.id))?.and_then(|(o, _)| o);
        let generated = self.thumbnailer.generate(&path, &m.blake3, orientation, &[size])?;
        let row = match generated.into_iter().next() {
            Some(g) => thumbnail::ThumbnailRow {
                media_id: m.id,
                size_class: size.as_str().to_owned(),
                format: g.format.extension().to_owned(),
                width: g.width,
                height: g.height,
                byte_size: g.byte_size,
                source_blake3: m.blake3,
                source_kind: g.source_kind.as_str().to_owned(),
            },
            // İçerik adresli önbellekte zaten var (aynı içerik başka bir yerde de duruyor).
            None => {
                let (cached, format) = self
                    .thumbnailer
                    .cache()
                    .lookup(&m.blake3, size)
                    .ok_or_else(|| JobError::transient("CACHE_RACE", "küçük resim önbellekten silindi"))?;
                let bytes = std::fs::read(&cached).map_err(|e| JobError::transient("IO", e.to_string()))?;
                let (width, height) = webp_dimensions(&bytes)?;
                let kind = match m.file_type {
                    MediaFileType::Raw | MediaFileType::Dng => SourceKind::EmbeddedPreview,
                    MediaFileType::Video => SourceKind::VideoFrame,
                    _ => SourceKind::Decode,
                };
                thumbnail::ThumbnailRow {
                    media_id: m.id,
                    size_class: size.as_str().to_owned(),
                    format: format.extension().to_owned(),
                    width,
                    height,
                    byte_size: bytes.len() as u64,
                    source_blake3: m.blake3,
                    source_kind: kind.as_str().to_owned(),
                }
            }
        };
        self.pool.write(|c| thumbnail::upsert(c, &row))?;
        if size == ThumbSize::Md && m.phash.is_none() {
            self.fill_phash(&m)?;
        }
        // Yüz ve CLIP yalnızca fotoğraflar için (video karesi dizinlenmez).
        if self.ai_followups && size == ThumbSize::Md && m.file_type != MediaFileType::Video {
            self.enqueue_ai(m.id, ctx)?;
        }
        Ok(())
    }

    /// pHash içe aktarmada hesaplanmaz (tam çözünürlükte çözmek yavaş);
    /// 768 px `md` küçük resmi aynı DCT hash'i için fazlasıyla yeterli.
    fn fill_phash(&self, m: &MediaFile) -> Result<(), JobError> {
        let Some((cached, _)) = self.thumbnailer.cache().lookup(&m.blake3, ThumbSize::Md) else { return Ok(()) };
        let bytes = std::fs::read(&cached).map_err(|e| JobError::transient("IO", e.to_string()))?;
        if let Some((phash, _)) = loupe_storage::hash::phash_from_bytes(&bytes) {
            self.pool.write(|c| media::fill_phash(c, m.id, phash))?;
        }
        Ok(())
    }

    fn metadata(&self, job: &Job) -> Result<(), JobError> {
        let (m, path) = self.media(job)?;
        let meta = read_metadata(&path)?;
        let update = media::MetadataUpdate {
            width: meta.width,
            height: meta.height,
            orientation: meta.orientation,
            taken_at: meta.taken_at.map(|t| (t.utc_ms, t.tz_offset_min, t.source)),
            gps: meta.gps.map(|g| (g.lat, g.lon, g.alt_m)),
            camera_make: meta.camera_make,
            camera_model: meta.camera_model,
            lens_model: meta.lens_model,
            focal_length_mm: meta.focal_length_mm,
            aperture_f: meta.aperture_f,
            exposure_s: meta.exposure_s,
            iso: meta.iso,
            duration_ms: meta.duration_ms,
        };
        self.pool.write(|c| media::apply_metadata(c, m.id, &update))?;
        // Ters coğrafi kodlama bellekte birkaç mikrosaniye: ayrı iş açmaya değmez.
        if let Some(g) = meta.gps {
            self.assign_place(m.id, g.lat, g.lon)?;
        }
        Ok(())
    }

    fn geocode(&self, job: &Job) -> Result<(), JobError> {
        let id = job.media_id.ok_or_else(|| JobError::permanent("NO_MEDIA", "iş bir medyaya bağlı değil"))?;
        let Some((_, Some((lat, lon)))) = self.pool.read(|c| media::render_hints(c, id))? else {
            return Ok(()); // GPS yok: yapılacak iş yok.
        };
        self.assign_place(id, lat, lon)
    }

    fn assign_place(&self, id: MediaId, lat: f64, lon: f64) -> Result<(), JobError> {
        let Some(geo) = &self.geo else { return Ok(()) };
        let Some(place) = geo.reverse(lat, lon)? else { return Ok(()) };
        let loc = new_location(&place, geo.dataset_ver());
        self.pool.write(|c| {
            let tx = c.transaction()?;
            let lid = location::upsert(&tx, &loc)?;
            location::assign_to_media(&tx, id, lid)?;
            tx.commit()?;
            Ok(())
        })?;
        Ok(())
    }
}

/// GeoNames yerini `locations` satırına çevirir: ülke, il (admin1), ilçe
/// (admin2) ve yerleşim (locality).
#[must_use]
pub fn new_location(p: &Place, dataset_ver: &str) -> location::NewLocation {
    location::NewLocation {
        geoname_id: Some(p.geoname_id),
        country_code: p.country_code.clone(),
        country: p.country.clone(),
        admin1: p.admin1.clone(),
        admin2: p.admin2.clone(),
        locality: Some(p.name.clone()),
        lat: p.lat,
        lon: p.lon,
        dataset_ver: dataset_ver.to_owned(),
    }
}

impl JobHandler for MediaPipeline {
    fn run(&self, job: &Job, ctx: &JobContext<'_>) -> Result<(), JobError> {
        match job.kind {
            JobKind::Thumb => self.thumb(job, ctx),
            JobKind::MetadataFull => self.metadata(job),
            JobKind::Geocode => self.geocode(job),
            other => Err(JobError::permanent("UNSUPPORTED_KIND", format!("{other} bu işleyicide değil"))),
        }
    }
}

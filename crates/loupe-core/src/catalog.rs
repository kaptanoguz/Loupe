//! Depolama katmanının katalogdan beklediği sözleşmeler.
//!
//! `loupe-storage` dosya sistemi işini yapar, kararı veren bilgiyi (bu hash zaten
//! var mı, bu satırı yaz, bu bulguyu kaydet) bu trait'ler üzerinden ister.
//! `loupe-db` bunları SQLite ile uygular; testler bellek içi sahte de yazabilir.

use crate::{Blake3Hash, ImportSessionId, LibraryId, MediaFileType, MediaId, PHash, Placement, RelPath, TakenAtSource};

/// `media_files` satırı oluşturmak için gereken alanlar. Geri kalan sütunlar
/// şema varsayılanlarını alır; EXIF/AI alanlarını sonraki işler doldurur.
#[derive(Debug, Clone, PartialEq)]
pub struct NewMediaFile {
    pub library_id: LibraryId,
    pub rel_path: RelPath,
    pub original_filename: String,
    pub placement: Placement,
    pub import_session_id: Option<ImportSessionId>,
    pub file_type: MediaFileType,
    pub raw_format: Option<String>,
    pub mime: String,
    pub size_bytes: u64,
    pub mtime_ns: i64,
    pub inode: Option<u64>,
    pub device_id: Option<u64>,
    pub blake3: Blake3Hash,
    pub phash: Option<PHash>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub taken_at_utc: i64,
    pub tz_offset_min: Option<i32>,
    pub taken_at_source: TakenAtSource,
    pub imported_at: i64,
}

/// Satır yazma sonucu. `Duplicate`, `blake3 UNIQUE` kısıtına takılan yarışı da
/// kapsar: iki import aynı içeriği aynı anda yazmaya çalışırsa biri kazanır.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitOutcome {
    Inserted(MediaId),
    Duplicate(MediaId),
}

pub trait ImportCatalog {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Aynı içerik katalogda var mı? Dosyaya dokunmadan önce sorulur.
    fn find_by_blake3(&mut self, hash: &Blake3Hash) -> Result<Option<MediaId>, Self::Error>;

    /// Dosya yerine yazılıp fsync edildikten SONRA çağrılır (dosya → fsync → DB).
    fn commit_media(&mut self, media: &NewMediaFile) -> Result<CommitOutcome, Self::Error>;

    /// Kopya nedeniyle atlanan dosyayı sayaçlara işlemek için (isteğe bağlı).
    fn note_duplicate(&mut self, _existing: MediaId) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// Bit-rot taramasında tek dosyanın katalogdaki beklenen hali.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyEntry {
    pub media_id: MediaId,
    pub rel_path: RelPath,
    pub blake3: Blake3Hash,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyOutcome {
    Ok,
    Missing,
    /// Okunan içeriğin hash'i katalogdakinden farklı (bit-rot veya dış değişiklik).
    Mismatch {
        actual: Blake3Hash,
    },
    Unreadable {
        reason: String,
    },
}

impl VerifyOutcome {
    #[must_use]
    pub const fn is_problem(&self) -> bool {
        !matches!(self, Self::Ok)
    }
}

pub trait VerifyCatalog {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Kütüphane için `after` kimliğinden sonraki en fazla `limit` kayıt, id sırasıyla.
    fn next_batch(&mut self, after: Option<MediaId>, limit: usize) -> Result<Vec<VerifyEntry>, Self::Error>;

    /// Tek dosyanın sonucunu kaydeder. `quarantine` bozuk dosyanın karantina kopyası.
    fn record(
        &mut self,
        entry: &VerifyEntry,
        outcome: &VerifyOutcome,
        bytes_read: u64,
        quarantine: Option<&RelPath>,
    ) -> Result<(), Self::Error>;
}

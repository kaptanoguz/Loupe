//! Satır tipleri. Yalnızca Adım 2A'nın ihtiyaç duyduğu sütunlar okunur;
//! EXIF, AI ve kullanıcı alanlarının tamamı ilgili crate'ler gelince eklenir.

use loupe_core::{
    Blake3Hash, ColorLabel, Flag, ImportItemId, ImportItemState, ImportMode, ImportSessionId, LibraryId, LibraryKind,
    LibraryState, MediaFileType, MediaId, PHash, Placement, RelPath, SessionMode, SessionState, TakenAtSource,
    VerifyRunId, VerifyState, VerifyTrigger,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewLibrary {
    /// `.loupe/library.json` içindeki kimlikle aynı.
    pub uuid: [u8; 16],
    pub name: String,
    pub kind: LibraryKind,
    pub volume_uuid: Option<String>,
    pub volume_label: Option<String>,
    pub network_source: Option<String>,
    pub fs_type: Option<String>,
    /// Mount noktasına göre kök; mount noktasının kendisiyse boş.
    pub rel_root: String,
    pub last_mount_path: Option<String>,
    pub supports_hardlink: bool,
    pub supports_noreplace: bool,
    pub supports_inotify: bool,
    pub is_managed: bool,
    pub default_import_mode: ImportMode,
    pub is_primary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Library {
    pub id: LibraryId,
    pub uuid: [u8; 16],
    pub name: String,
    pub kind: LibraryKind,
    pub volume_uuid: Option<String>,
    pub volume_label: Option<String>,
    pub network_source: Option<String>,
    pub fs_type: Option<String>,
    pub rel_root: String,
    pub last_mount_path: Option<String>,
    pub supports_hardlink: bool,
    pub supports_noreplace: bool,
    pub supports_inotify: bool,
    pub is_managed: bool,
    pub layout_template: String,
    pub default_import_mode: ImportMode,
    pub is_primary: bool,
    pub state: LibraryState,
    pub state_reason: Option<String>,
    pub total_bytes: Option<i64>,
    pub free_bytes: Option<i64>,
    pub last_seen_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Disk bağlandığında / çıkarıldığında güncellenen alanlar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryPresence {
    pub state: LibraryState,
    pub state_reason: Option<String>,
    pub last_mount_path: Option<String>,
    pub total_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaFile {
    pub id: MediaId,
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
    pub rating: u8,
    pub color_label: Option<ColorLabel>,
    pub is_favorite: bool,
    pub flag: Flag,
    pub soft_deleted_at: Option<i64>,
    pub verify_state: VerifyState,
    pub last_verified_at: i64,
    pub missing_since: Option<i64>,
    pub imported_at: i64,
    pub updated_at: i64,
}

/// Kullanıcının kırpma (culling) alanları. `None` = değiştirme.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserFieldsUpdate {
    pub rating: Option<u8>,
    pub color_label: Option<Option<ColorLabel>>,
    pub is_favorite: Option<bool>,
    pub flag: Option<Flag>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewImportSession {
    pub library_id: LibraryId,
    pub mode: SessionMode,
    pub source_root: String,
    pub source_volume: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportSession {
    pub id: ImportSessionId,
    pub library_id: LibraryId,
    pub mode: SessionMode,
    pub source_root: String,
    pub source_volume: Option<String>,
    pub state: SessionState,
    pub total_files: u64,
    pub total_bytes: u64,
    pub done_files: u64,
    pub done_bytes: u64,
    pub skipped_dup: u64,
    pub failed_files: u64,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewImportItem {
    /// `source_root`'a göre göreli.
    pub source_path: String,
    pub size_bytes: u64,
    pub source_mtime_ns: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportItem {
    pub id: ImportItemId,
    pub session_id: ImportSessionId,
    pub source_path: String,
    pub size_bytes: u64,
    pub source_mtime_ns: i64,
    pub blake3: Option<Blake3Hash>,
    pub state: ImportItemState,
    pub media_id: Option<MediaId>,
    pub error_code: Option<String>,
    pub attempts: u32,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyRun {
    pub id: VerifyRunId,
    pub library_id: LibraryId,
    pub trigger: VerifyTrigger,
    pub state: String,
    pub cursor_media_id: Option<MediaId>,
    pub total: u64,
    pub checked: u64,
    pub ok: u64,
    pub missing: u64,
    pub mismatched: u64,
    pub unreadable: u64,
    pub bytes_read: u64,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyFinding {
    pub id: i64,
    pub run_id: VerifyRunId,
    pub media_id: MediaId,
    pub kind: VerifyState,
    pub actual_blake3: Option<Blake3Hash>,
    pub quarantine_rel: Option<String>,
    pub found_at: i64,
}

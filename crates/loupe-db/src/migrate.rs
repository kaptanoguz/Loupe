//! Migrasyon motoru.
//!
//! - SQL dosyaları ikili dosyaya gömülür (`include_str!`); dağıtımda dosya araması yok.
//! - Her dosya kendi `BEGIN IMMEDIATE` transaction'ında uygulanır: yarıda kalan
//!   migrasyon diye bir durum olmaz.
//! - Dosya `PRAGMA user_version = N` ile kendi numarasını yazar; motor bunu
//!   doğrular. Ayrıca `_loupe_migrations` tablosuna dosyanın BLAKE3 özeti yazılır;
//!   yayımlanmış bir migrasyon sonradan değiştirilirse açılış hata verir.
//! - Bilinen en yeni sürümden yeni bir veritabanı eski uygulamayla açılmaz.

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::{CLIP_DIM, DbError, FACE_DIM};

#[derive(Debug, Clone, Copy)]
pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub sql: &'static str,
}

impl Migration {
    #[must_use]
    pub fn checksum(&self) -> String {
        blake3::hash(self.sql.as_bytes()).to_hex().to_string()
    }
}

pub const MIGRATIONS: &[Migration] = &[
    Migration { version: 1, name: "0001_initial", sql: include_str!("../migrations/0001_initial.sql") },
    Migration { version: 2, name: "0002_jobs_lan_cloud", sql: include_str!("../migrations/0002_jobs_lan_cloud.sql") },
    Migration { version: 3, name: "0003_clip_dim", sql: include_str!("../migrations/0003_clip_dim.sql") },
];

/// Migrasyonlardan sonra var olması gereken tablolar (sanal tablolar dahil).
pub const EXPECTED_TABLES: &[&str] = &[
    "album_media",
    "albums",
    "cloud_targets",
    "device_assets",
    "devices",
    "edits",
    "event_media",
    "events",
    "face_clusters",
    "faces",
    "import_items",
    "import_sessions",
    "jobs",
    "lan_transfers",
    "libraries",
    "locations",
    "media_duplicates",
    "media_files",
    "media_metadata",
    "ml_models",
    "near_duplicate_groups",
    "persons",
    "settings",
    "smart_albums",
    "sync_queue",
    "sync_status",
    "thumbnails",
    "vec_face_centroids",
    "vec_faces",
    "vec_media_clip",
    "verify_findings",
    "verify_runs",
    "versions",
];

/// 0001'deki tetikleyici sayısı (RFC-001A). Eksik tetikleyici sayaçları bozar.
pub const EXPECTED_TRIGGERS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    pub from_version: i64,
    pub to_version: i64,
    pub applied: Vec<u32>,
}

const HISTORY_DDL: &str = "CREATE TABLE IF NOT EXISTS _loupe_migrations (
    version    INTEGER PRIMARY KEY,
    name       TEXT    NOT NULL,
    checksum   TEXT    NOT NULL CHECK (length(checksum) = 64),
    applied_at INTEGER NOT NULL
) STRICT";

fn user_version(conn: &Connection) -> Result<i64, DbError> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

#[must_use]
pub fn latest_version() -> u32 {
    MIGRATIONS.iter().map(|m| m.version).max().unwrap_or(0)
}

pub fn migrate(conn: &mut Connection) -> Result<MigrationReport, DbError> {
    let known = latest_version();
    {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(HISTORY_DDL)?;
        tx.commit()?;
    }
    let from = user_version(conn)?;
    if from > i64::from(known) {
        return Err(DbError::SchemaTooNew { found: from, known });
    }

    let mut applied = Vec::new();
    for m in MIGRATIONS {
        if i64::from(m.version) <= from {
            check_history(conn, m, from)?;
            continue;
        }
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(m.sql).map_err(|source| DbError::MigrationFailed {
            version: m.version,
            name: m.name,
            source,
        })?;
        let found = user_version(&tx)?;
        if found != i64::from(m.version) {
            // tx düşürülünce geri alınır.
            return Err(DbError::MigrationVersion { version: m.version, name: m.name, found });
        }
        tx.execute(
            "INSERT INTO _loupe_migrations (version, name, checksum, applied_at) VALUES (?1, ?2, ?3, ?4)",
            params![m.version, m.name, m.checksum(), loupe_core::now_ms()],
        )?;
        tx.commit()?;
        applied.push(m.version);
    }

    Ok(MigrationReport { from_version: from, to_version: user_version(conn)?, applied })
}

fn check_history(conn: &Connection, m: &Migration, found: i64) -> Result<(), DbError> {
    let recorded: Option<String> = conn
        .query_row("SELECT checksum FROM _loupe_migrations WHERE version = ?1", [m.version], |r| r.get(0))
        .optional()?;
    let current = m.checksum();
    match recorded {
        None => Err(DbError::MissingHistory { version: m.version, found }),
        Some(rec) if rec != current => {
            Err(DbError::ChecksumMismatch { version: m.version, name: m.name, recorded: rec, current })
        }
        Some(_) => Ok(()),
    }
}

/// Açılış doğrulaması: bütünlük, yabancı anahtarlar, beklenen nesneler,
/// sqlite-vec ve vektör boyutlarının Rust sabitleriyle uyumu.
pub fn validate_schema(conn: &Connection) -> Result<(), DbError> {
    let quick: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    if quick != "ok" {
        return Err(DbError::Validation(format!("quick_check: {quick}")));
    }

    let mut fk = conn.prepare("PRAGMA foreign_key_check")?;
    let violations: Vec<String> = fk
        .query_map([], |r| Ok(format!("{}#{}", r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?.unwrap_or(-1))))?
        .collect::<Result<_, _>>()?;
    if !violations.is_empty() {
        return Err(DbError::Validation(format!("yabancı anahtar ihlali: {}", violations.join(", "))));
    }

    let mut stmt = conn.prepare_cached("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1")?;
    let missing: Vec<&str> =
        EXPECTED_TABLES.iter().copied().filter(|t| !matches!(stmt.exists([t]), Ok(true))).collect();
    if !missing.is_empty() {
        return Err(DbError::Validation(format!("eksik tablolar: {}", missing.join(", "))));
    }

    let triggers: i64 =
        conn.query_row("SELECT count(*) FROM sqlite_master WHERE type = 'trigger'", [], |r| r.get(0))?;
    if usize::try_from(triggers).ok() != Some(EXPECTED_TRIGGERS) {
        return Err(DbError::Validation(format!("{triggers} tetikleyici var, {EXPECTED_TRIGGERS} bekleniyordu")));
    }

    let _vec_version: String = conn.query_row("SELECT vec_version()", [], |r| r.get(0))?;
    for (table, dim) in [("vec_media_clip", CLIP_DIM), ("vec_faces", FACE_DIM), ("vec_face_centroids", FACE_DIM)] {
        let found = vec_dimension(conn, table)?;
        if found != Some(dim) {
            return Err(DbError::Validation(format!("{table} boyutu {found:?}, Rust sabiti {dim}")));
        }
    }
    Ok(())
}

/// `CREATE VIRTUAL TABLE … float[N]` tanımından N'yi okur.
pub fn vec_dimension(conn: &Connection, table: &str) -> Result<Option<usize>, DbError> {
    let sql: Option<String> = conn
        .query_row("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1", [table], |r| r.get(0))
        .optional()?;
    Ok(sql.and_then(|s| {
        let start = s.find("float[")? + "float[".len();
        let len = s[start..].find(']')?;
        s[start..start + len].trim().parse().ok()
    }))
}

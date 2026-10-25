-- =============================================================================
-- Loupe — Katalog şeması, migrasyon 0004: küçük resim boyut sınıfları ve iş alma indeksi
-- Adım 2B kararı (2026-10-25): boyut sınıfları sm / md / lg = uzun kenar 256 / 768 / 1920 px
-- (eski grid / preview / full = 512 / 1600 / 2560 yerine). Önbellek içerik adreslidir:
--   $XDG_CACHE_HOME/loupe/thumbs/<blake3[0:2]>/<blake3>_<sm|md|lg>.webp
-- SQLite CHECK kısıtını değiştiremediği için iki tablo yeniden kurulur.
-- =============================================================================

PRAGMA user_version = 4;

-- ─────────────────────────────────────────────────────────────────────────────
-- 1. thumbnails: yalnızca önbellek envanteri. Eski satırlar yeni boyutlara
--    karşılık gelmez; atılır ve küçük resimler yeniden üretilir.
-- ─────────────────────────────────────────────────────────────────────────────
DROP TABLE thumbnails;
CREATE TABLE thumbnails (
  media_id       INTEGER NOT NULL REFERENCES media_files (id) ON DELETE CASCADE,
  size_class     TEXT    NOT NULL CHECK (size_class IN ('sm','md','lg')),
                                               -- uzun kenar: sm 256, md 768, lg 1920 px
  format         TEXT    NOT NULL CHECK (format IN ('webp','avif','jpeg')),
  width          INTEGER NOT NULL CHECK (width  > 0),
  height         INTEGER NOT NULL CHECK (height > 0),
  byte_size      INTEGER NOT NULL CHECK (byte_size > 0),
  source_blake3  BLOB    NOT NULL CHECK (length(source_blake3) = 32),
                                               -- önbellek anahtarı: thumb://<sınıf>/<blake3 hex>
  edit_version   INTEGER NOT NULL DEFAULT 0,
  source_kind    TEXT    NOT NULL
                 CHECK (source_kind IN ('embedded_preview','demosaic','decode','video_frame')),
  generated_at   INTEGER NOT NULL,
  last_access_at INTEGER,
  PRIMARY KEY (media_id, size_class)
) STRICT, WITHOUT ROWID;

CREATE INDEX thumbnails_lru ON thumbnails (size_class, last_access_at);

-- ─────────────────────────────────────────────────────────────────────────────
-- 2. jobs: thumb varyantları sm/md/lg. Bekleyen işler eşlenerek taşınır.
-- ─────────────────────────────────────────────────────────────────────────────
CREATE TABLE jobs_new (
  id               INTEGER PRIMARY KEY,
  kind             TEXT    NOT NULL CHECK (kind IN (
                     'thumb',            -- variant: 'sm' | 'md' | 'lg'
                     'metadata_full',
                     'phash',
                     'geocode',
                     'face_detect',
                     'clip_embed',
                     'cluster_faces',
                     'cluster_events',
                     'sidecar_write')),
  media_id         INTEGER REFERENCES media_files (id) ON DELETE CASCADE,
  variant          TEXT    NOT NULL DEFAULT '',
  base_priority    INTEGER NOT NULL CHECK (base_priority BETWEEN 0 AND 9),
  state            TEXT    NOT NULL DEFAULT 'pending'
                   CHECK (state IN ('pending','running','failed','dead')),
  attempts         INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  max_attempts     INTEGER NOT NULL DEFAULT 5 CHECK (max_attempts >= 1),
  run_after        INTEGER NOT NULL,
  lease_owner      TEXT,
  lease_until      INTEGER,
  model_id         INTEGER REFERENCES ml_models (id),
  last_error_code  TEXT,
  last_error_msg   TEXT,
  created_at       INTEGER NOT NULL,
  updated_at       INTEGER NOT NULL,
  UNIQUE (kind, media_id, variant),
  CHECK ((state = 'running') = (lease_until IS NOT NULL AND lease_owner IS NOT NULL)),
  CHECK (kind IN ('cluster_faces','cluster_events') OR media_id IS NOT NULL),
  CHECK (kind <> 'thumb' OR variant IN ('sm','md','lg'))
) STRICT;

INSERT INTO jobs_new
SELECT id, kind, media_id,
       CASE WHEN kind = 'thumb' THEN
         CASE variant WHEN 'grid' THEN 'sm' WHEN 'preview' THEN 'md' ELSE 'lg' END
       ELSE variant END,
       base_priority, state, attempts, max_attempts, run_after, lease_owner, lease_until,
       model_id, last_error_code, last_error_msg, created_at, updated_at
FROM jobs;

DROP TABLE jobs;
ALTER TABLE jobs_new RENAME TO jobs;

CREATE INDEX jobs_ready   ON jobs (kind, base_priority, run_after, id)
  WHERE state IN ('pending','failed');
CREATE UNIQUE INDEX jobs_global_once ON jobs (kind, variant) WHERE media_id IS NULL;
CREATE INDEX jobs_media   ON jobs (media_id) WHERE media_id IS NOT NULL;
CREATE INDEX jobs_leases  ON jobs (lease_until) WHERE state = 'running';
CREATE INDEX jobs_dead    ON jobs (kind, updated_at) WHERE state = 'dead';
-- Tür fark etmeksizin "en acil hazır iş": işçi havuzu tek sorguda alır (loupe-jobs).
CREATE INDEX jobs_claim   ON jobs (base_priority, run_after, id)
  WHERE state IN ('pending','failed');

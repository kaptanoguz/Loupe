-- =============================================================================
-- Loupe — Katalog şeması, migrasyon 0002: iş kuyrukları, LAN, bulut kuyruğu, ayarlar
-- RFC-001B §1.4, §2.7, §3.5. 0001_initial.sql üzerine uygulanır.
--
-- Kuyruk tabloları (jobs, sync_queue) yalnızca YAPILACAK işi tutar: başarılı iş
-- satırı silinir, sonucu alan tablosunda yaşar (thumbnails, media_ml_state,
-- sync_status). Böylece tablolar 100k'lık bir taramadan sonra da küçük kalır.
-- =============================================================================

PRAGMA user_version = 2;

-- ─────────────────────────────────────────────────────────────────────────────
-- 1. ARKA PLAN İŞ KUYRUĞU (küçük resim, EXIF, pHash, AI, kümeleme, sidecar)
--    Etkin öncelik bellekte hesaplanır (viewport); burada yalnızca taban öncelik.
-- ─────────────────────────────────────────────────────────────────────────────
CREATE TABLE jobs (
  id               INTEGER PRIMARY KEY,
  kind             TEXT    NOT NULL CHECK (kind IN (
                     'thumb',            -- variant: 'grid' | 'preview' | 'full'
                     'metadata_full',    -- tam EXIF/XMP dökümü (media_metadata)
                     'phash',
                     'geocode',
                     'face_detect',      -- tespit + ArcFace vektörü tek işte
                     'clip_embed',
                     'cluster_faces',    -- media_id NULL: kütüphane geneli
                     'cluster_events',   -- media_id NULL
                     'sidecar_write')),
  media_id         INTEGER REFERENCES media_files (id) ON DELETE CASCADE,
  variant          TEXT    NOT NULL DEFAULT '',
  base_priority    INTEGER NOT NULL CHECK (base_priority BETWEEN 0 AND 9),
                                               -- RFC-001B §1.2 öncelik sınıfları (0 = en acil)
  state            TEXT    NOT NULL DEFAULT 'pending'
                   CHECK (state IN ('pending','running','failed','dead')),
                                               -- failed: backoff'ta bekliyor; dead: deneme bitti
  attempts         INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  max_attempts     INTEGER NOT NULL DEFAULT 5 CHECK (max_attempts >= 1),
  run_after        INTEGER NOT NULL,           -- exponential backoff zamanı
  lease_owner      TEXT,                       -- '<pid>/<işçi>'
  lease_until      INTEGER,                    -- çöken işçinin işi süre dolunca geri alınır
  model_id         INTEGER REFERENCES ml_models (id),  -- AI işleri: hangi modelle
  last_error_code  TEXT,
  last_error_msg   TEXT,
  created_at       INTEGER NOT NULL,
  updated_at       INTEGER NOT NULL,
  UNIQUE (kind, media_id, variant),
  CHECK ((state = 'running') = (lease_until IS NOT NULL AND lease_owner IS NOT NULL)),
  CHECK (kind IN ('cluster_faces','cluster_events') OR media_id IS NOT NULL),
  CHECK (kind <> 'thumb' OR variant IN ('grid','preview','full'))
) STRICT;

-- Planlayıcının sıradaki işleri sayfalayarak çektiği indeks (havuz = kind grubu)
CREATE INDEX jobs_ready   ON jobs (kind, base_priority, run_after, id)
  WHERE state IN ('pending','failed');
-- UNIQUE (kind, media_id, variant) NULL media_id'yi ayrı sayar: kütüphane geneli işler için ayrı kural
CREATE UNIQUE INDEX jobs_global_once ON jobs (kind, variant) WHERE media_id IS NULL;
CREATE INDEX jobs_media   ON jobs (media_id) WHERE media_id IS NOT NULL;
CREATE INDEX jobs_leases  ON jobs (lease_until) WHERE state = 'running';
CREATE INDEX jobs_dead    ON jobs (kind, updated_at) WHERE state = 'dead';

-- ─────────────────────────────────────────────────────────────────────────────
-- 2. BULUT YÜKLEME KUYRUĞU (sync_status'ın iş tarafı)
-- ─────────────────────────────────────────────────────────────────────────────
CREATE TABLE sync_queue (
  id                   INTEGER PRIMARY KEY,
  target_id            INTEGER NOT NULL REFERENCES cloud_targets (id) ON DELETE CASCADE,
  media_id             INTEGER NOT NULL REFERENCES media_files (id) ON DELETE CASCADE,
  op                   TEXT    NOT NULL CHECK (op IN ('upload','verify','delete_remote','restore')),
  state                TEXT    NOT NULL DEFAULT 'pending'
                       CHECK (state IN ('pending','running','failed','dead')),
  priority             INTEGER NOT NULL DEFAULT 5 CHECK (priority BETWEEN 0 AND 9),
                                               -- favoriler / yeni LAN aktarımları önce
  attempts             INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  run_after            INTEGER NOT NULL,
  lease_owner          TEXT,
  lease_until          INTEGER,
  bytes_total          INTEGER CHECK (bytes_total IS NULL OR bytes_total >= 0),
  bytes_done           INTEGER NOT NULL DEFAULT 0 CHECK (bytes_done >= 0),
  multipart_upload_id  TEXT,                   -- S3 UploadId: çökme sonrası devam
  multipart_parts      TEXT    CHECK (multipart_parts IS NULL OR json_valid(multipart_parts)),
                                               -- [{ "n": 1, "etag": "…", "sha256": "…" }, …]
  last_error_code      TEXT,                   -- RFC-001B §3.4 hata sınıfları
  last_error_msg       TEXT,
  created_at           INTEGER NOT NULL,
  updated_at           INTEGER NOT NULL,
  UNIQUE (target_id, media_id, op),
  CHECK ((state = 'running') = (lease_until IS NOT NULL AND lease_owner IS NOT NULL)),
  CHECK (bytes_total IS NULL OR bytes_done <= bytes_total)
) STRICT;
CREATE INDEX sync_queue_ready  ON sync_queue (target_id, priority, run_after, id)
  WHERE state IN ('pending','failed');
CREATE INDEX sync_queue_leases ON sync_queue (lease_until) WHERE state = 'running';
CREATE INDEX sync_queue_media  ON sync_queue (media_id);

-- ─────────────────────────────────────────────────────────────────────────────
-- 3. LAN: EŞLEŞMİŞ CİHAZLAR
--    Eşleşme belirteçleri (60 sn) diske hiç yazılmaz; yalnızca bellekte, hash'leriyle.
--    Yerel CA'nın özel anahtarı Secret Service'tedir, DB'de değil.
-- ─────────────────────────────────────────────────────────────────────────────
CREATE TABLE devices (
  id                  INTEGER PRIMARY KEY,
  name                TEXT    NOT NULL CHECK (length(name) BETWEEN 1 AND 80),
  platform            TEXT    NOT NULL CHECK (platform IN ('android','ios','desktop')),
  cert_sha256         BLOB    NOT NULL UNIQUE CHECK (length(cert_sha256) = 32),
                                               -- istemci sertifikasının parmak izi
  spki_sha256         BLOB    NOT NULL CHECK (length(spki_sha256) = 32),
                                               -- açık anahtar parmak izi (sertifika yenilense de aynı)
  cert_serial         BLOB    NOT NULL UNIQUE,
  cert_not_after      INTEGER NOT NULL,
  target_library_id   INTEGER REFERENCES libraries (id) ON DELETE SET NULL,
  conflict_policy     TEXT    NOT NULL DEFAULT 'ask'
                      CHECK (conflict_policy IN ('ask','keep_newest','keep_both','skip')),
  delete_after_verify INTEGER NOT NULL DEFAULT 0 CHECK (delete_after_verify IN (0,1)),
                                               -- telefonda "doğrulanınca sil" izni
  paired_at           INTEGER NOT NULL,
  last_seen_at        INTEGER,
  last_address        TEXT,                    -- yalnızca tanı amaçlı
  revoked_at          INTEGER,
  revoke_reason       TEXT,
  CHECK (revoked_at IS NULL OR revoke_reason IS NOT NULL)
) STRICT;
CREATE INDEX devices_active ON devices (spki_sha256) WHERE revoked_at IS NULL;

-- Telefondaki varlık kimliği ↔ katalog eşlemesi: "zaten aktarıldı" ve çakışma tespiti
-- client_asset_id: Android MediaStore _ID + DATE_ADDED, iOS PHAsset.localIdentifier
CREATE TABLE device_assets (
  device_id           INTEGER NOT NULL REFERENCES devices (id) ON DELETE CASCADE,
  client_asset_id     TEXT    NOT NULL,
  blake3              BLOB    NOT NULL CHECK (length(blake3) = 32),
  media_id            INTEGER REFERENCES media_files (id) ON DELETE SET NULL,
  client_modified_at  INTEGER NOT NULL,
  synced_at           INTEGER NOT NULL,
  PRIMARY KEY (device_id, client_asset_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX device_assets_media ON device_assets (media_id) WHERE media_id IS NOT NULL;

-- tus yükleme oturumları
CREATE TABLE lan_transfers (
  upload_id           TEXT    PRIMARY KEY CHECK (length(upload_id) = 22),  -- 128-bit base64url
  device_id           INTEGER NOT NULL REFERENCES devices (id) ON DELETE CASCADE,
  library_id          INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
  client_asset_id     TEXT    NOT NULL,
  filename            TEXT    NOT NULL,
  expected_blake3     BLOB    NOT NULL CHECK (length(expected_blake3) = 32),
  expected_size       INTEGER NOT NULL CHECK (expected_size >= 0),
  offset_bytes        INTEGER NOT NULL DEFAULT 0,      -- yalnızca fsync edilmiş bayt
  staging_name        TEXT    NOT NULL,                -- <kök>/.loupe/staging/<upload_id>.part
  client_taken_at     INTEGER,
  client_modified_at  INTEGER NOT NULL,
  state               TEXT    NOT NULL DEFAULT 'receiving'
                      CHECK (state IN ('receiving','verifying','done','skipped','failed','expired')),
  resolution          TEXT    CHECK (resolution IN ('new','keep_newest','keep_both')),
  replaces_media_id   INTEGER REFERENCES media_files (id) ON DELETE SET NULL,  -- keep_newest
  result_media_id     INTEGER REFERENCES media_files (id) ON DELETE SET NULL,
  error_code          TEXT,
  created_at          INTEGER NOT NULL,
  updated_at          INTEGER NOT NULL,
  expires_at          INTEGER NOT NULL,                -- 7 gün; süresi dolan .part silinir
  CHECK (offset_bytes BETWEEN 0 AND expected_size),
  CHECK (state <> 'done' OR (offset_bytes = expected_size AND result_media_id IS NOT NULL)),
  CHECK (resolution IS NOT 'keep_newest' OR replaces_media_id IS NOT NULL)
) STRICT;
CREATE UNIQUE INDEX lan_transfers_active ON lan_transfers (device_id, client_asset_id)
  WHERE state IN ('receiving','verifying');
CREATE INDEX lan_transfers_expiry ON lan_transfers (expires_at)
  WHERE state IN ('receiving','verifying');

-- ─────────────────────────────────────────────────────────────────────────────
-- 4. AYARLAR (kaynak modu, model boşta bekleme süresi, bant genişliği…)
-- ─────────────────────────────────────────────────────────────────────────────
CREATE TABLE settings (
  key         TEXT    PRIMARY KEY CHECK (key GLOB '[a-z]*' AND key NOT GLOB '*[^a-z0-9._]*'),
  value       TEXT    NOT NULL CHECK (json_valid(value)),
  updated_at  INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

INSERT INTO settings (key, value, updated_at) VALUES
  ('resources.mode',              '"balanced"', 0),   -- quiet | balanced | full
  ('ai.idle_unload_s',            '300',        0),
  ('ai.text_encoder_idle_unload_s','120',       0),
  ('ai.gpu',                      '"auto"',     0),   -- off | auto | cuda | rocm | openvino
  ('thumbs.cache_max_bytes',      '8589934592', 0),   -- 8 GiB
  ('lan.enabled',                 'true',       0),
  ('lan.port',                    '47123',      0),
  ('cloud.global_bandwidth_bps',  'null',       0);

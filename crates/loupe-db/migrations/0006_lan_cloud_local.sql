-- =============================================================================
-- Loupe — Katalog şeması, migrasyon 0006: yerel/NAS bulut hedefi
-- Adım 2D (RFC-001D): `loupe-cloud` S3 uyumlu, WebDAV ve yerel/NAS hedeflerine
-- yazar. Yerel hedef (`provider = 'local'`) takılı bir diske ya da NFS/SMB
-- bağlamasına şifreli nesneleri aynı düzende yazar (3-2-1'in ikinci kopyası).
--
-- SQLite CHECK kısıtını değiştiremediği için cloud_targets yeniden kurulur.
-- Migrasyon transaction içinde çalıştığından foreign_keys kapatılamaz: eski
-- tablo silinince çocuk satırlar (sync_queue, sync_status) CASCADE ile gider.
-- Bu yüzden önce geçici tablolara alınır, yeni tablo yerine geçince geri yazılır.
-- =============================================================================

PRAGMA user_version = 6;

CREATE TEMP TABLE _sync_queue_keep  AS SELECT * FROM sync_queue;
CREATE TEMP TABLE _sync_status_keep AS SELECT * FROM sync_status;

CREATE TABLE cloud_targets_new (
  id              INTEGER PRIMARY KEY,
  name            TEXT    NOT NULL UNIQUE,     -- 'B2 — Soğuk Arşiv'
  provider        TEXT    NOT NULL CHECK (provider IN
                  ('s3','b2','r2','wasabi','gdrive','onedrive','webdav','proton_rclone','local')),
                                               -- local: takılı disk veya NAS bağlaması
  config          TEXT    NOT NULL CHECK (json_valid(config)),  -- gizli OLMAYAN: bucket, region, endpoint, yol
  secret_ref      TEXT    NOT NULL,            -- Secret Service anahtar adı; sır DB'de tutulmaz
  key_prefix      TEXT    NOT NULL DEFAULT 'loupe/',
  encryption      TEXT    NOT NULL DEFAULT 'none' CHECK (encryption IN ('none','age')),
  age_recipient   TEXT    CHECK (age_recipient IS NULL OR age_recipient LIKE 'age1%'),
  bandwidth_bps   INTEGER CHECK (bandwidth_bps IS NULL OR bandwidth_bps > 0),
  schedule        TEXT,                        -- '22:00-07:00'
  skip_metered    INTEGER NOT NULL DEFAULT 1 CHECK (skip_metered IN (0,1)),
  enabled         INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0,1)),
  created_at      INTEGER NOT NULL,
  updated_at      INTEGER NOT NULL,
  CHECK (encryption = 'none' OR age_recipient IS NOT NULL)
) STRICT;

INSERT INTO cloud_targets_new SELECT * FROM cloud_targets;
DROP TABLE cloud_targets;
ALTER TABLE cloud_targets_new RENAME TO cloud_targets;

INSERT INTO sync_queue  SELECT * FROM temp._sync_queue_keep;
INSERT INTO sync_status SELECT * FROM temp._sync_status_keep;
DROP TABLE temp._sync_queue_keep;
DROP TABLE temp._sync_status_keep;

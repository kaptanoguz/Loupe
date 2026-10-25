-- =============================================================================
-- Loupe — Katalog şeması, migrasyon 0001
-- RFC-001A §2. Hedef: SQLite >= 3.45, sqlite-vec >= 0.1.6
--
-- Kurallar:
--   * Tüm zaman damgaları UTC epoch milisaniye (INTEGER).
--   * Hash'ler ham BLOB (BLAKE3 = 32 bayt). pHash = 64-bit işaretli INTEGER.
--   * Tüm tablolar STRICT: yanlış tipte değer yazmak hata verir.
--   * Bağlantı başına PRAGMA'lar burada DEĞİL, loupe-db::Connection::open içinde
--     uygulanır (bkz. RFC-001A §2.2). Bu dosya tek transaction içinde çalışır.
--   * Bu dosya bir kez uygulandıktan sonra değiştirilmez; değişiklik = yeni migrasyon.
-- =============================================================================

PRAGMA user_version = 1;

-- ─────────────────────────────────────────────────────────────────────────────
-- 1. KÜTÜPHANELER (kullanıcının seçtiği kök dizinler)
-- ─────────────────────────────────────────────────────────────────────────────
CREATE TABLE libraries (
  id                    INTEGER PRIMARY KEY,
  uuid                  BLOB    NOT NULL UNIQUE CHECK (length(uuid) = 16),
                                -- <kök>/.loupe/library.json içindekiyle aynı
  name                  TEXT    NOT NULL CHECK (length(name) BETWEEN 1 AND 120),
  kind                  TEXT    NOT NULL CHECK (kind IN ('internal','removable','network')),

  -- Disk kimliği: mount yolu değişse de kütüphane bulunur.
  volume_uuid           TEXT,   -- dosya sistemi UUID'si (udisks2 IdUUID / blkid)
  volume_label          TEXT,   -- 'Arsiv-4TB'
  network_source        TEXT,   -- NAS: 'nas.local:/volume1/photos' veya '//nas/photos'
  fs_type               TEXT,   -- ext4, btrfs, xfs, exfat, ntfs3, nfs4, cifs
  rel_root              TEXT    NOT NULL DEFAULT '',
                                -- mount noktasına GÖRE kök: 'Fotograflar'
  last_mount_path       TEXT,   -- son bilinen mutlak yol (yalnızca ipucu; doğruluk kaynağı değil)

  -- Yetenekler (bağlanınca ölçülür, loupe-volume tarafından güncellenir)
  supports_hardlink     INTEGER NOT NULL DEFAULT 0 CHECK (supports_hardlink IN (0,1)),
  supports_noreplace    INTEGER NOT NULL DEFAULT 0 CHECK (supports_noreplace IN (0,1)),
  supports_inotify      INTEGER NOT NULL DEFAULT 0 CHECK (supports_inotify IN (0,1)),

  -- Davranış
  is_managed            INTEGER NOT NULL DEFAULT 1 CHECK (is_managed IN (0,1)),
                                -- 1: düzeni uygulama yönetir (kopyala/taşı/hardlink hedefi)
                                -- 0: referans kütüphanesi; dosyalar yeniden adlandırılmaz
  layout_template       TEXT    NOT NULL DEFAULT '{yyyy}/{mm}/{dd}',
  default_import_mode   TEXT    NOT NULL DEFAULT 'copy'
                                CHECK (default_import_mode IN ('copy','move','hardlink')),
  is_primary            INTEGER NOT NULL DEFAULT 0 CHECK (is_primary IN (0,1)),
  watch_mode            TEXT    NOT NULL DEFAULT 'inotify'
                                CHECK (watch_mode IN ('inotify','poll','off')),
  poll_interval_s       INTEGER NOT NULL DEFAULT 900 CHECK (poll_interval_s >= 60),
  verify_interval_days  INTEGER NOT NULL DEFAULT 30 CHECK (verify_interval_days >= 0),
                                -- 0 = yalnızca manuel "Verify Library"

  -- Durum
  state                 TEXT    NOT NULL DEFAULT 'online'
                                CHECK (state IN ('online','offline','readonly','error')),
  state_reason          TEXT,   -- 'VOLUME_NOT_MOUNTED', 'MARKER_MISMATCH', 'EROFS'…
  total_bytes           INTEGER CHECK (total_bytes IS NULL OR total_bytes >= 0),
  free_bytes            INTEGER CHECK (free_bytes  IS NULL OR free_bytes  >= 0),
  last_seen_at          INTEGER,
  created_at            INTEGER NOT NULL,
  updated_at            INTEGER NOT NULL,

  -- Çıkarılabilir disk UUID'siz, ağ diski kaynaksız tanımlanamaz
  CHECK (kind <> 'removable' OR volume_uuid IS NOT NULL),
  CHECK (kind <> 'network'   OR network_source IS NOT NULL),
  CHECK (is_managed = 1 OR default_import_mode = 'copy'),
  CHECK (rel_root NOT LIKE '/%' AND rel_root NOT LIKE '%../%' AND rel_root <> '..')
) STRICT;

CREATE UNIQUE INDEX libraries_one_primary ON libraries (is_primary) WHERE is_primary = 1;
CREATE UNIQUE INDEX libraries_volume_root ON libraries (volume_uuid, rel_root)
  WHERE volume_uuid IS NOT NULL;
CREATE UNIQUE INDEX libraries_network_root ON libraries (network_source, rel_root)
  WHERE network_source IS NOT NULL;

-- ─────────────────────────────────────────────────────────────────────────────
-- 2. KONUMLAR (çevrimdışı GeoNames ters coğrafi kodlama sonucu, normalize)
-- ─────────────────────────────────────────────────────────────────────────────
CREATE TABLE locations (
  id             INTEGER PRIMARY KEY,
  geoname_id     INTEGER UNIQUE,               -- en yakın GeoNames kaydı; kullanıcı yeri ise NULL
  country_code   TEXT    NOT NULL CHECK (length(country_code) = 2),  -- 'TR'
  country        TEXT    NOT NULL,             -- 'Türkiye'
  admin1         TEXT,                         -- il: 'Nevşehir'
  admin2         TEXT,                         -- ilçe: 'Ürgüp'
  locality       TEXT,                         -- yerleşim: 'Göreme'
  poi_name       TEXT,                         -- mekan (isteğe bağlı POI veri seti)
  user_label     TEXT,                         -- kullanıcı adı: 'Yazlık'
  lat            REAL    NOT NULL CHECK (lat BETWEEN -90 AND 90),
  lon            REAL    NOT NULL CHECK (lon BETWEEN -180 AND 180),
  dataset_ver    TEXT    NOT NULL,             -- 'geonames-cities500-2026-09'
  created_at     INTEGER NOT NULL
) STRICT;

CREATE INDEX locations_hierarchy ON locations (country_code, admin1, admin2, locality);

-- ─────────────────────────────────────────────────────────────────────────────
-- 3. İMPORT OTURUMLARI VE ÖĞELERİ
--    "Taşı" modunda kaynak dosyanın silinmesi gibi adımlar çökmeden sonra da
--    tamamlanabilsin diye her öğenin durumu kalıcıdır (RFC-001A §3.4).
-- ─────────────────────────────────────────────────────────────────────────────
CREATE TABLE import_sessions (
  id              INTEGER PRIMARY KEY,
  library_id      INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
  mode            TEXT    NOT NULL CHECK (mode IN ('copy','move','hardlink','reference','watch','lan')),
  source_root     TEXT    NOT NULL,            -- '/run/media/oguz/EOS_DIGITAL/DCIM' | 'lan:Pixel 9'
  source_volume   TEXT,                        -- kaynak disk UUID'si (SD kart yeniden takılırsa devam)
  state           TEXT    NOT NULL DEFAULT 'scanning'
                  CHECK (state IN ('scanning','running','paused','done','failed','cancelled')),
  total_files     INTEGER NOT NULL DEFAULT 0,
  total_bytes     INTEGER NOT NULL DEFAULT 0,
  done_files      INTEGER NOT NULL DEFAULT 0,
  done_bytes      INTEGER NOT NULL DEFAULT 0,
  skipped_dup     INTEGER NOT NULL DEFAULT 0,
  failed_files    INTEGER NOT NULL DEFAULT 0,
  started_at      INTEGER NOT NULL,
  finished_at     INTEGER,
  CHECK (finished_at IS NULL OR finished_at >= started_at)
) STRICT;

CREATE INDEX import_sessions_active ON import_sessions (state)
  WHERE state IN ('scanning','running','paused');

CREATE TABLE import_items (
  id              INTEGER PRIMARY KEY,
  session_id      INTEGER NOT NULL REFERENCES import_sessions (id) ON DELETE CASCADE,
  source_path     TEXT    NOT NULL,            -- source_root'a göre göreli
  size_bytes      INTEGER NOT NULL,
  source_mtime_ns INTEGER NOT NULL,
  blake3          BLOB    CHECK (blake3 IS NULL OR length(blake3) = 32),
  state           TEXT    NOT NULL DEFAULT 'pending'
                  CHECK (state IN ('pending','hashed','staged','placed','committed',
                                   'source_removed','skipped_duplicate','failed')),
  media_id        INTEGER REFERENCES media_files (id) ON DELETE SET NULL,
  error_code      TEXT,
  attempts        INTEGER NOT NULL DEFAULT 0,
  updated_at      INTEGER NOT NULL,
  UNIQUE (session_id, source_path)
) STRICT;

CREATE INDEX import_items_resume ON import_items (session_id, state)
  WHERE state NOT IN ('committed','source_removed','skipped_duplicate');

-- ─────────────────────────────────────────────────────────────────────────────
-- 4. YAKIN KOPYA GRUPLARI (pHash)
-- ─────────────────────────────────────────────────────────────────────────────
CREATE TABLE near_duplicate_groups (
  id               INTEGER PRIMARY KEY,
  keeper_media_id  INTEGER REFERENCES media_files (id) ON DELETE SET NULL,
                                               -- kullanıcının "bunu tut" dediği
  member_count     INTEGER NOT NULL DEFAULT 0,
  reviewed_at      INTEGER,                    -- kullanıcı gözden geçirdi → tekrar önerilmez
  created_at       INTEGER NOT NULL
) STRICT;

-- ─────────────────────────────────────────────────────────────────────────────
-- 5. MEDYA DOSYALARI
-- ─────────────────────────────────────────────────────────────────────────────
CREATE TABLE media_files (
  id                 INTEGER PRIMARY KEY,
  library_id         INTEGER NOT NULL REFERENCES libraries (id) ON DELETE RESTRICT,
  rel_path           TEXT    NOT NULL,         -- '2026/10/24/DSC_0412.NEF' (NFC normalize)
  original_filename  TEXT    NOT NULL,         -- kaynaktaki ad; yeniden adlandırılsa da korunur
  placement          TEXT    NOT NULL
                     CHECK (placement IN ('copy','move','hardlink','reference','lan')),
  import_session_id  INTEGER REFERENCES import_sessions (id) ON DELETE SET NULL,

  -- Dosya
  file_type          TEXT    NOT NULL
                     CHECK (file_type IN ('jpeg','heic','avif','png','tiff','webp','jxl',
                                          'raw','dng','video','other')),
  raw_format         TEXT,                     -- 'NEF','CR2','CR3','ARW','RAF','ORF','RW2'…
  mime               TEXT    NOT NULL,
  size_bytes         INTEGER NOT NULL CHECK (size_bytes >= 0),
  mtime_ns           INTEGER NOT NULL,         -- hızlı değişiklik tespiti (stat)
  inode              INTEGER,
  device_id          INTEGER,                  -- st_dev; hardlink tespitinde inode ile birlikte

  -- Hash'ler
  blake3             BLOB    NOT NULL UNIQUE CHECK (length(blake3) = 32),
                                               -- tam kopya engeli: aynı içerik iki kez yazılamaz
  phash              INTEGER,                  -- 64-bit DCT pHash (görüntüden; video: orta kare)
  near_dup_group_id  INTEGER REFERENCES near_duplicate_groups (id) ON DELETE SET NULL,

  -- Görüntü
  width              INTEGER CHECK (width  IS NULL OR width  > 0),
  height             INTEGER CHECK (height IS NULL OR height > 0),
  orientation        INTEGER CHECK (orientation IS NULL OR orientation BETWEEN 1 AND 8),
  duration_ms        INTEGER CHECK (duration_ms IS NULL OR duration_ms >= 0),
  thumbhash          BLOB    CHECK (thumbhash IS NULL OR length(thumbhash) <= 32),

  -- Zaman
  taken_at_utc       INTEGER NOT NULL,         -- zaman çizelgesi sıralama anahtarı
  tz_offset_min      INTEGER CHECK (tz_offset_min IS NULL OR tz_offset_min BETWEEN -840 AND 840),
  taken_at_source    TEXT    NOT NULL
                     CHECK (taken_at_source IN ('exif','gps','filename','mtime','user')),

  -- Konum
  gps_lat            REAL    CHECK (gps_lat IS NULL OR gps_lat BETWEEN -90 AND 90),
  gps_lon            REAL    CHECK (gps_lon IS NULL OR gps_lon BETWEEN -180 AND 180),
  gps_alt_m          REAL,
  location_id        INTEGER REFERENCES locations (id) ON DELETE SET NULL,

  -- Kamera (sık filtrelenenler sütun; tamamı media_metadata'da)
  camera_make        TEXT,
  camera_model       TEXT,
  lens_model         TEXT,
  focal_length_mm    REAL,
  aperture_f         REAL,
  exposure_s         REAL,
  iso                INTEGER,

  -- Eşleşmeler
  raw_pair_id        INTEGER REFERENCES media_files (id) ON DELETE SET NULL,  -- RAW ↔ JPEG
  live_pair_id       INTEGER REFERENCES media_files (id) ON DELETE SET NULL,  -- HEIC ↔ MOV

  -- Sidecar (XMP / uygulama JSON'u)
  xmp_rel_path       TEXT,
  xmp_blake3         BLOB    CHECK (xmp_blake3 IS NULL OR length(xmp_blake3) = 32),
  xmp_mtime_ns       INTEGER,                  -- dış program (darktable vb.) değiştirdi mi?
  sidecar_dirty      INTEGER NOT NULL DEFAULT 0 CHECK (sidecar_dirty IN (0,1)),
                                               -- DB'de değişti, sidecar'a henüz yazılmadı

  -- Kullanıcı
  rating             INTEGER NOT NULL DEFAULT 0 CHECK (rating BETWEEN 0 AND 5),
  color_label        TEXT    CHECK (color_label IN ('red','yellow','green','blue','purple')),
  is_favorite        INTEGER NOT NULL DEFAULT 0 CHECK (is_favorite IN (0,1)),
  flag               TEXT    NOT NULL DEFAULT 'none' CHECK (flag IN ('none','pick','reject')),
  soft_deleted_at    INTEGER,                  -- çöp kutusu; 30 gün sonra kalıcı silme

  -- Bütünlük (bit-rot)
  verify_state       TEXT    NOT NULL DEFAULT 'ok'
                     CHECK (verify_state IN ('ok','missing','mismatch','unreadable','offline')),
  last_verified_at   INTEGER NOT NULL,         -- son TAM hash doğrulaması
  missing_since      INTEGER,                  -- 'missing' olduğu an; geri gelirse NULL

  imported_at        INTEGER NOT NULL,
  updated_at         INTEGER NOT NULL,

  UNIQUE (library_id, rel_path),
  CHECK (rel_path NOT LIKE '/%' AND rel_path NOT LIKE '%../%'),
  CHECK (file_type <> 'raw' OR raw_format IS NOT NULL),
  CHECK ((gps_lat IS NULL) = (gps_lon IS NULL)),
  CHECK (raw_pair_id  IS NULL OR raw_pair_id  <> id),
  CHECK (live_pair_id IS NULL OR live_pair_id <> id),
  CHECK (verify_state <> 'missing' OR missing_since IS NOT NULL)
) STRICT;

-- Zaman çizelgesi: kısmi indeks, çöp kutusu hariç. (taken_at_utc, id) imleç tabanlı sayfalama için.
CREATE INDEX media_timeline ON media_files (taken_at_utc DESC, id DESC)
  WHERE soft_deleted_at IS NULL;
CREATE INDEX media_library_timeline ON media_files (library_id, taken_at_utc DESC)
  WHERE soft_deleted_at IS NULL;
CREATE INDEX media_location  ON media_files (location_id) WHERE location_id IS NOT NULL;
CREATE INDEX media_camera    ON media_files (camera_model, lens_model) WHERE camera_model IS NOT NULL;
CREATE INDEX media_culling   ON media_files (rating, color_label, is_favorite, flag)
  WHERE soft_deleted_at IS NULL;
CREATE INDEX media_trash     ON media_files (soft_deleted_at) WHERE soft_deleted_at IS NOT NULL;
-- Verify Library: en eski doğrulanandan başla, kütüphane bazında
CREATE INDEX media_verify    ON media_files (library_id, last_verified_at);
CREATE INDEX media_problems  ON media_files (verify_state) WHERE verify_state <> 'ok';
CREATE INDEX media_sidecar_dirty ON media_files (id) WHERE sidecar_dirty = 1;
-- inode eşleşmesi: izleyici rename/hardlink tespitinde yolu değil inode'u kullanır
CREATE INDEX media_inode     ON media_files (device_id, inode) WHERE inode IS NOT NULL;
CREATE INDEX media_near_dup  ON media_files (near_dup_group_id) WHERE near_dup_group_id IS NOT NULL;
CREATE INDEX media_raw_pair  ON media_files (raw_pair_id)  WHERE raw_pair_id  IS NOT NULL;
CREATE INDEX media_live_pair ON media_files (live_pair_id) WHERE live_pair_id IS NOT NULL;
CREATE INDEX media_session   ON media_files (import_session_id) WHERE import_session_id IS NOT NULL;
-- pHash bant indeksleri (güvercin yuvası ilkesi): 64 bit 4 × 16 bitlik banda bölünür.
-- Hamming mesafesi <= 3 olan her çift en az bir bantta birebir eşleşir → tam tarama gerekmez.
-- Sorgular bu ifadeleri AYNEN kullanmalıdır (RFC-001A §3.6).
CREATE INDEX media_phash_b0 ON media_files ((phash        & 65535)) WHERE phash IS NOT NULL;
CREATE INDEX media_phash_b1 ON media_files (((phash >> 16) & 65535)) WHERE phash IS NOT NULL;
CREATE INDEX media_phash_b2 ON media_files (((phash >> 32) & 65535)) WHERE phash IS NOT NULL;
CREATE INDEX media_phash_b3 ON media_files (((phash >> 48) & 65535)) WHERE phash IS NOT NULL;

-- Ham EXIF / XMP / QuickTime: denetçideki "tüm metadata" görünümü
CREATE TABLE media_metadata (
  media_id    INTEGER PRIMARY KEY REFERENCES media_files (id) ON DELETE CASCADE,
  extractor   TEXT    NOT NULL,                -- 'nom-exif@2.x' | 'exiftool@13.x'
  data        TEXT    NOT NULL CHECK (json_valid(data)),
  extracted_at INTEGER NOT NULL
) STRICT;

-- Referans kütüphanelerinde zaten var olan tam kopyalar: silinmez, kaydedilir.
CREATE TABLE media_duplicates (
  id           INTEGER PRIMARY KEY,
  media_id     INTEGER NOT NULL REFERENCES media_files (id) ON DELETE CASCADE,  -- kanonik kayıt
  library_id   INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
  rel_path     TEXT    NOT NULL,
  found_at     INTEGER NOT NULL,
  resolution   TEXT    NOT NULL DEFAULT 'pending'
               CHECK (resolution IN ('pending','kept','ignored','removed_by_user')),
  UNIQUE (library_id, rel_path)
) STRICT;
CREATE INDEX media_duplicates_media ON media_duplicates (media_id);

-- ─────────────────────────────────────────────────────────────────────────────
-- 6. KÜÇÜK RESİMLER
--    Baytlar DB'de değil önbellek dizinindedir:
--      $XDG_CACHE_HOME/loupe/thumbs/<blake3[0:2]>/<blake3>_<size_class>.<format>
--    Arayüz bunları thumb://localhost/<media_id>/<size_class> özel protokolüyle
--    alır (JSON IPC yok, base64 yok). Bu tablo yalnızca envanter ve geçerliliktir.
-- ─────────────────────────────────────────────────────────────────────────────
CREATE TABLE thumbnails (
  media_id       INTEGER NOT NULL REFERENCES media_files (id) ON DELETE CASCADE,
  size_class     TEXT    NOT NULL CHECK (size_class IN ('grid','preview','full')),
                                               -- uzun kenar: 256 / 1024 / 2048 px
  format         TEXT    NOT NULL CHECK (format IN ('webp','avif','jpeg')),
  width          INTEGER NOT NULL CHECK (width  > 0),
  height         INTEGER NOT NULL CHECK (height > 0),
  byte_size      INTEGER NOT NULL CHECK (byte_size > 0),
  source_blake3  BLOB    NOT NULL CHECK (length(source_blake3) = 32),
                                               -- orijinalin hash'i; değişirse küçük resim geçersiz
  edit_version   INTEGER NOT NULL DEFAULT 0,   -- düzenlenmiş sürüm render'ı için
  source_kind    TEXT    NOT NULL
                 CHECK (source_kind IN ('embedded_preview','demosaic','decode','video_frame')),
  generated_at   INTEGER NOT NULL,
  last_access_at INTEGER,                      -- önbellek tahliyesi (LRU) için; toplu güncellenir
  PRIMARY KEY (media_id, size_class)
) STRICT, WITHOUT ROWID;

CREATE INDEX thumbnails_lru ON thumbnails (size_class, last_access_at);

-- ─────────────────────────────────────────────────────────────────────────────
-- 7. KİŞİLER, YÜZ KÜMELERİ, YÜZLER
--    İsimlendirme küme seviyesindedir: bir kümeye kişi atamak tüm yüzlerini adlandırır.
--    Tek yüz için elle düzeltme faces.person_override_id ile yapılır.
--    Bir yüzün kişisi = COALESCE(person_override_id, face_clusters.person_id)
-- ─────────────────────────────────────────────────────────────────────────────
CREATE TABLE persons (
  id             INTEGER PRIMARY KEY,
  name           TEXT    NOT NULL CHECK (length(name) BETWEEN 1 AND 120),
  birth_date     TEXT    CHECK (birth_date IS NULL OR birth_date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'),
  cover_face_id  INTEGER REFERENCES faces (id) ON DELETE SET NULL,
  is_hidden      INTEGER NOT NULL DEFAULT 0 CHECK (is_hidden IN (0,1)),
  face_count     INTEGER NOT NULL DEFAULT 0,   -- tetikleyiciyle değil, kümeleme işi sonunda toplu
  created_at     INTEGER NOT NULL,
  updated_at     INTEGER NOT NULL
) STRICT;
CREATE INDEX persons_name ON persons (name COLLATE NOCASE) WHERE is_hidden = 0;

CREATE TABLE face_clusters (
  id             INTEGER PRIMARY KEY,
  person_id      INTEGER REFERENCES persons (id) ON DELETE SET NULL,  -- NULL = isimsiz küme
  face_count     INTEGER NOT NULL DEFAULT 0,
  is_confirmed   INTEGER NOT NULL DEFAULT 0 CHECK (is_confirmed IN (0,1)),
                                               -- kullanıcı onayladı → yeniden kümeleme bölmez
  is_ignored     INTEGER NOT NULL DEFAULT 0 CHECK (is_ignored IN (0,1)),
                                               -- "bu kişiyi gösterme" (yabancılar)
  cover_face_id  INTEGER REFERENCES faces (id) ON DELETE SET NULL,
  model_id       INTEGER NOT NULL REFERENCES ml_models (id),
  created_at     INTEGER NOT NULL,
  updated_at     INTEGER NOT NULL
) STRICT;
CREATE INDEX face_clusters_person ON face_clusters (person_id) WHERE person_id IS NOT NULL;
CREATE INDEX face_clusters_unnamed ON face_clusters (face_count DESC)
  WHERE person_id IS NULL AND is_ignored = 0;

CREATE TABLE faces (
  id                  INTEGER PRIMARY KEY,
  media_id            INTEGER NOT NULL REFERENCES media_files (id) ON DELETE CASCADE,
  cluster_id          INTEGER REFERENCES face_clusters (id) ON DELETE SET NULL,
  person_override_id  INTEGER REFERENCES persons (id) ON DELETE SET NULL,
  is_rejected         INTEGER NOT NULL DEFAULT 0 CHECK (is_rejected IN (0,1)),
                                               -- "bu yüz değil" (yanlış tespit)
  -- normalize sınırlayıcı kutu (0..1), EXIF yönlendirmesi uygulanmış görüntüye göre
  bbox_x   REAL NOT NULL CHECK (bbox_x BETWEEN 0 AND 1),
  bbox_y   REAL NOT NULL CHECK (bbox_y BETWEEN 0 AND 1),
  bbox_w   REAL NOT NULL CHECK (bbox_w > 0 AND bbox_x + bbox_w <= 1.0001),
  bbox_h   REAL NOT NULL CHECK (bbox_h > 0 AND bbox_y + bbox_h <= 1.0001),
  landmarks           BLOB CHECK (landmarks IS NULL OR length(landmarks) = 40),  -- 5 × (x,y) × f32
  det_score           REAL NOT NULL CHECK (det_score BETWEEN 0 AND 1),
  quality             REAL NOT NULL CHECK (quality BETWEEN 0 AND 1),
                                               -- bulanıklık + poz; eşik altı kümelemeye girmez
  detector_model_id   INTEGER NOT NULL REFERENCES ml_models (id),
  embedder_model_id   INTEGER NOT NULL REFERENCES ml_models (id),
  created_at          INTEGER NOT NULL
) STRICT;
CREATE INDEX faces_media    ON faces (media_id);
CREATE INDEX faces_cluster  ON faces (cluster_id) WHERE cluster_id IS NOT NULL;
CREATE INDEX faces_override ON faces (person_override_id) WHERE person_override_id IS NOT NULL;
CREATE INDEX faces_unclustered ON faces (id)
  WHERE cluster_id IS NULL AND is_rejected = 0;

CREATE VIEW v_face_person AS
SELECT f.id AS face_id,
       f.media_id,
       COALESCE(f.person_override_id, c.person_id) AS person_id
FROM faces f
LEFT JOIN face_clusters c ON c.id = f.cluster_id
WHERE f.is_rejected = 0;

-- ML model kaydı (yüz ve semantik vektörlerin hangi modelle üretildiği)
CREATE TABLE ml_models (
  id           INTEGER PRIMARY KEY,
  task         TEXT    NOT NULL CHECK (task IN ('face_detect','face_embed','clip_image','clip_text')),
  name         TEXT    NOT NULL,               -- 'scrfd_10g', 'arcface_r100', 'siglip2-base-multilingual'
  version      TEXT    NOT NULL,
  dim          INTEGER CHECK (dim IS NULL OR dim > 0),
  file_blake3  BLOB    NOT NULL CHECK (length(file_blake3) = 32),  -- bozuk model yüklenmez
  is_active    INTEGER NOT NULL DEFAULT 1 CHECK (is_active IN (0,1)),
  created_at   INTEGER NOT NULL,
  UNIQUE (task, name, version)
) STRICT;
CREATE UNIQUE INDEX ml_models_one_active ON ml_models (task) WHERE is_active = 1;

-- Vektörler (sqlite-vec). Asıl kopya burada; usearch kullanılırsa onun dizini
-- bu tablolardan yeniden üretilebilen bir önbellektir (RFC-001A §2.4).
CREATE VIRTUAL TABLE vec_faces USING vec0 (
  face_id    INTEGER PRIMARY KEY,
  embedding  float[512] distance_metric=cosine      -- ArcFace, L2 normalize
);
CREATE VIRTUAL TABLE vec_face_centroids USING vec0 (
  cluster_id INTEGER PRIMARY KEY,
  embedding  float[512] distance_metric=cosine      -- yeni yüzü en yakın kümeye atamak için
);
CREATE VIRTUAL TABLE vec_media_clip USING vec0 (
  media_id   INTEGER PRIMARY KEY,
  embedding  float[768] distance_metric=cosine      -- SigLIP 2 / çok dilli CLIP
);

-- ─────────────────────────────────────────────────────────────────────────────
-- 8. ETKİNLİKLER (otomatik zamansal kümeleme + kullanıcı düzeltmesi)
-- ─────────────────────────────────────────────────────────────────────────────
CREATE TABLE events (
  id               INTEGER PRIMARY KEY,
  title            TEXT    NOT NULL CHECK (length(title) BETWEEN 1 AND 200),
                                               -- 'Ekim 2026 — Kapadokya'
  title_source     TEXT    NOT NULL DEFAULT 'auto' CHECK (title_source IN ('auto','user')),
  origin           TEXT    NOT NULL DEFAULT 'auto' CHECK (origin IN ('auto','manual')),
  start_utc        INTEGER NOT NULL,
  end_utc          INTEGER NOT NULL,
  location_id      INTEGER REFERENCES locations (id) ON DELETE SET NULL,  -- baskın yer
  centroid_lat     REAL,
  centroid_lon     REAL,
  radius_km        REAL    CHECK (radius_km IS NULL OR radius_km >= 0),
  media_count      INTEGER NOT NULL DEFAULT 0,
  cover_media_id   INTEGER REFERENCES media_files (id) ON DELETE SET NULL,
  is_locked        INTEGER NOT NULL DEFAULT 0 CHECK (is_locked IN (0,1)),
                                               -- kullanıcı düzenledi → otomatik kümeleme dokunmaz
  cluster_params   TEXT    CHECK (cluster_params IS NULL OR json_valid(cluster_params)),
                                               -- üretildiği parametreler (yeniden üretilebilirlik)
  folder_rel_path  TEXT,                       -- '{yyyy}/{event}' şablonunda karşılık gelen klasör
  created_at       INTEGER NOT NULL,
  updated_at       INTEGER NOT NULL,
  CHECK (end_utc >= start_utc),
  CHECK ((centroid_lat IS NULL) = (centroid_lon IS NULL))
) STRICT;
CREATE INDEX events_time     ON events (start_utc DESC, end_utc);
CREATE INDEX events_location ON events (location_id) WHERE location_id IS NOT NULL;

CREATE TABLE event_media (
  event_id   INTEGER NOT NULL REFERENCES events (id) ON DELETE CASCADE,
  media_id   INTEGER NOT NULL REFERENCES media_files (id) ON DELETE CASCADE,
  source     TEXT    NOT NULL DEFAULT 'auto'
             CHECK (source IN ('auto','user_added','user_excluded')),
                                               -- user_excluded kalıcıdır: yeniden kümeleme geri koymaz
  PRIMARY KEY (event_id, media_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX event_media_media ON event_media (media_id, source);

-- ─────────────────────────────────────────────────────────────────────────────
-- 9. ALBÜMLER VE AKILLI ALBÜMLER
-- ─────────────────────────────────────────────────────────────────────────────
CREATE TABLE albums (
  id              INTEGER PRIMARY KEY,
  parent_id       INTEGER REFERENCES albums (id) ON DELETE RESTRICT,  -- klasörleme
  kind            TEXT    NOT NULL DEFAULT 'album' CHECK (kind IN ('album','folder')),
  name            TEXT    NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
  description     TEXT,
  cover_media_id  INTEGER REFERENCES media_files (id) ON DELETE SET NULL,
  sort_mode       TEXT    NOT NULL DEFAULT 'taken_asc'
                  CHECK (sort_mode IN ('taken_asc','taken_desc','manual','added_desc')),
  media_count     INTEGER NOT NULL DEFAULT 0,
  created_at      INTEGER NOT NULL,
  updated_at      INTEGER NOT NULL,
  CHECK (parent_id IS NULL OR parent_id <> id),
  CHECK (kind = 'album' OR cover_media_id IS NULL)
) STRICT;
CREATE INDEX albums_parent ON albums (parent_id, name);

CREATE TABLE album_media (
  album_id   INTEGER NOT NULL REFERENCES albums (id) ON DELETE CASCADE,
  media_id   INTEGER NOT NULL REFERENCES media_files (id) ON DELETE CASCADE,
  position   REAL    NOT NULL,                 -- kesirli sıralama: araya ekleme yeniden numaralamaz
  added_at   INTEGER NOT NULL,
  PRIMARY KEY (album_id, media_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX album_media_order ON album_media (album_id, position);
CREATE INDEX album_media_media ON album_media (media_id);

-- Akıllı albüm = kayıtlı filtre. rules, beyaz listeli alan/operatörlerden oluşan bir
-- JSON AST'dir; Rust'ta tipli yapıya ayrıştırılıp parametreli SQL'e derlenir.
-- Ham SQL ASLA saklanmaz (enjeksiyon yapısal olarak imkansız). Örnek: RFC-001A §2.5.
CREATE TABLE smart_albums (
  id             INTEGER PRIMARY KEY,
  parent_id      INTEGER REFERENCES albums (id) ON DELETE SET NULL,  -- bir klasör altında
  name           TEXT    NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
  rules          TEXT    NOT NULL CHECK (json_valid(rules) AND json_type(rules) = 'object'),
  rules_version  INTEGER NOT NULL DEFAULT 1 CHECK (rules_version >= 1),
  sort_mode      TEXT    NOT NULL DEFAULT 'taken_desc'
                 CHECK (sort_mode IN ('taken_asc','taken_desc','rating_desc')),
  result_limit   INTEGER CHECK (result_limit IS NULL OR result_limit > 0),
  is_pinned      INTEGER NOT NULL DEFAULT 0 CHECK (is_pinned IN (0,1)),
  created_at     INTEGER NOT NULL,
  updated_at     INTEGER NOT NULL
) STRICT;
CREATE INDEX smart_albums_pinned ON smart_albums (is_pinned, name);

-- ─────────────────────────────────────────────────────────────────────────────
-- 10. SÜRÜMLER VE DÜZENLEMELER (gelecekteki non-destructive düzenleme)
--     Orijinal dosya asla değiştirilmez. İşlem yığını DB'de ve .loupe.json sidecar'ında.
-- ─────────────────────────────────────────────────────────────────────────────
CREATE TABLE versions (
  id                 INTEGER PRIMARY KEY,
  media_id           INTEGER NOT NULL REFERENCES media_files (id) ON DELETE CASCADE,
  name               TEXT    NOT NULL DEFAULT 'Ana',   -- 'Ana', 'Siyah-beyaz', 'Kırpılmış 4:5'
  parent_version_id  INTEGER REFERENCES versions (id) ON DELETE SET NULL,
  is_current         INTEGER NOT NULL DEFAULT 0 CHECK (is_current IN (0,1)),
  revision           INTEGER NOT NULL DEFAULT 0,       -- her edits değişikliğinde +1 (önbellek anahtarı)
  created_at         INTEGER NOT NULL,
  updated_at         INTEGER NOT NULL,
  UNIQUE (media_id, name)
) STRICT;
CREATE UNIQUE INDEX versions_one_current ON versions (media_id) WHERE is_current = 1;

CREATE TABLE edits (
  id              INTEGER PRIMARY KEY,
  version_id      INTEGER NOT NULL REFERENCES versions (id) ON DELETE CASCADE,
  seq             INTEGER NOT NULL CHECK (seq >= 0),    -- uygulama sırası
  op              TEXT    NOT NULL CHECK (op IN ('crop','rotate','flip','straighten','exposure',
                                                 'white_balance','tone_curve','hsl','sharpen',
                                                 'noise_reduction','lens_correction','vignette')),
  params          TEXT    NOT NULL CHECK (json_valid(params) AND json_type(params) = 'object'),
  engine_version  INTEGER NOT NULL CHECK (engine_version >= 1),
                                               -- işlem tanımı sürümü: eski düzenleme aynı görünür
  is_enabled      INTEGER NOT NULL DEFAULT 1 CHECK (is_enabled IN (0,1)),
  created_at      INTEGER NOT NULL,
  UNIQUE (version_id, seq)
) STRICT;

-- ─────────────────────────────────────────────────────────────────────────────
-- 11. BULUT HEDEFLERİ VE SENKRONİZASYON DURUMU
--     (Kuyruk tablosu sync_queue Adım 1B'de, iş kuyruğu mimarisiyle birlikte.)
-- ─────────────────────────────────────────────────────────────────────────────
CREATE TABLE cloud_targets (
  id              INTEGER PRIMARY KEY,
  name            TEXT    NOT NULL UNIQUE,     -- 'B2 — Soğuk Arşiv'
  provider        TEXT    NOT NULL CHECK (provider IN
                  ('s3','b2','r2','wasabi','gdrive','onedrive','webdav','proton_rclone')),
  config          TEXT    NOT NULL CHECK (json_valid(config)),  -- gizli OLMAYAN: bucket, region, endpoint
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

-- Her (medya, hedef) çifti için gerçek durum. Arayüzdeki mikro ikon buradan türetilir.
CREATE TABLE sync_status (
  media_id              INTEGER NOT NULL REFERENCES media_files (id) ON DELETE RESTRICT,
                                               -- buluttaki kopyası olan medya, önce uzaktan
                                               -- silinmeden kalıcı silinemez
  target_id             INTEGER NOT NULL REFERENCES cloud_targets (id) ON DELETE CASCADE,
  state                 TEXT    NOT NULL DEFAULT 'local_only'
                        CHECK (state IN ('local_only','queued','uploading','synced',
                                         'conflict','error','remote_deleted')),
  local_checksum        BLOB    NOT NULL CHECK (length(local_checksum) = 32),
                                               -- yükleme anındaki orijinal BLAKE3
  remote_key            TEXT,                  -- şifreliyse 'objects/ab/cd/<blake3>.age'
  remote_size           INTEGER CHECK (remote_size IS NULL OR remote_size >= 0),
  remote_checksum       TEXT,                  -- sağlayıcının, yüklenen baytlar için hesapladığı
  remote_checksum_algo  TEXT    CHECK (remote_checksum_algo IN
                                       ('sha256','sha1','md5','etag','quickxor','blake3')),
  encryption            TEXT    NOT NULL DEFAULT 'none' CHECK (encryption IN ('none','age')),
  key_fingerprint       TEXT,                  -- hangi age alıcısıyla şifrelendi (rotasyon)
  uploaded_at           INTEGER,
  last_verified_at      INTEGER,               -- son uzak doğrulama
  conflict_status       TEXT    NOT NULL DEFAULT 'none'
                        CHECK (conflict_status IN ('none','remote_missing','remote_changed',
                                                   'local_changed','both_changed')),
  last_error_code       TEXT,
  updated_at            INTEGER NOT NULL,
  PRIMARY KEY (media_id, target_id),
  CHECK (state <> 'synced' OR (remote_key IS NOT NULL AND remote_checksum IS NOT NULL
                               AND uploaded_at IS NOT NULL)),
  CHECK (encryption = 'none' OR key_fingerprint IS NOT NULL),
  CHECK ((state = 'conflict') = (conflict_status <> 'none'))
) STRICT, WITHOUT ROWID;
CREATE INDEX sync_status_target_state ON sync_status (target_id, state);
CREATE INDEX sync_status_verify ON sync_status (target_id, last_verified_at)
  WHERE state = 'synced';
CREATE INDEX sync_status_attention ON sync_status (state)
  WHERE state IN ('conflict','error');

-- ─────────────────────────────────────────────────────────────────────────────
-- 12. VERIFY LIBRARY ÇALIŞTIRMALARI (devam edebilir bit-rot taraması)
-- ─────────────────────────────────────────────────────────────────────────────
CREATE TABLE verify_runs (
  id              INTEGER PRIMARY KEY,
  library_id      INTEGER NOT NULL REFERENCES libraries (id) ON DELETE CASCADE,
  trigger         TEXT    NOT NULL CHECK (trigger IN ('manual','scheduled','remount')),
  scope           TEXT    NOT NULL DEFAULT 'library' CHECK (scope IN ('library','album','selection')),
  scope_ref       INTEGER,                     -- album_id vb.
  state           TEXT    NOT NULL DEFAULT 'running'
                  CHECK (state IN ('running','paused','done','cancelled','failed')),
  cursor_media_id INTEGER,                     -- kaldığı yer (çökme / duraklatma sonrası devam)
  total           INTEGER NOT NULL DEFAULT 0,
  checked         INTEGER NOT NULL DEFAULT 0,
  ok              INTEGER NOT NULL DEFAULT 0,
  missing         INTEGER NOT NULL DEFAULT 0,
  mismatched      INTEGER NOT NULL DEFAULT 0,
  unreadable      INTEGER NOT NULL DEFAULT 0,
  repaired        INTEGER NOT NULL DEFAULT 0,
  bytes_read      INTEGER NOT NULL DEFAULT 0,
  started_at      INTEGER NOT NULL,
  finished_at     INTEGER
) STRICT;
CREATE INDEX verify_runs_library ON verify_runs (library_id, started_at DESC);
CREATE UNIQUE INDEX verify_runs_one_active ON verify_runs (library_id)
  WHERE state IN ('running','paused');

-- Bulunan her sorun ayrı kayıt: rapor ve onarım listesi
CREATE TABLE verify_findings (
  id             INTEGER PRIMARY KEY,
  run_id         INTEGER NOT NULL REFERENCES verify_runs (id) ON DELETE CASCADE,
  media_id       INTEGER NOT NULL REFERENCES media_files (id) ON DELETE CASCADE,
  kind           TEXT    NOT NULL CHECK (kind IN ('missing','mismatch','unreadable')),
  actual_blake3  BLOB    CHECK (actual_blake3 IS NULL OR length(actual_blake3) = 32),
  quarantine_rel TEXT,                         -- .loupe/quarantine/… altındaki konum
  resolution     TEXT    NOT NULL DEFAULT 'open'
                 CHECK (resolution IN ('open','restored_cloud','restored_local','accepted','ignored')),
  found_at       INTEGER NOT NULL,
  resolved_at    INTEGER
) STRICT;
CREATE INDEX verify_findings_open ON verify_findings (resolution, found_at) WHERE resolution = 'open';
CREATE INDEX verify_findings_media ON verify_findings (media_id);

-- ─────────────────────────────────────────────────────────────────────────────
-- 13. TETİKLEYİCİLER (yalnızca ucuz ve yerel tutarlılık kuralları)
-- ─────────────────────────────────────────────────────────────────────────────

-- Çift yönlü RAW/Live eşleşmeleri: biri silinince diğerindeki işaret ON DELETE SET NULL ile temizlenir.

-- Albüm sayaçları
CREATE TRIGGER album_media_ai AFTER INSERT ON album_media BEGIN
  UPDATE albums SET media_count = media_count + 1 WHERE id = NEW.album_id;
END;
CREATE TRIGGER album_media_ad AFTER DELETE ON album_media BEGIN
  UPDATE albums SET media_count = media_count - 1 WHERE id = OLD.album_id;
END;

-- Etkinlik sayaçları (user_excluded sayılmaz)
CREATE TRIGGER event_media_ai AFTER INSERT ON event_media WHEN NEW.source <> 'user_excluded' BEGIN
  UPDATE events SET media_count = media_count + 1 WHERE id = NEW.event_id;
END;
CREATE TRIGGER event_media_ad AFTER DELETE ON event_media WHEN OLD.source <> 'user_excluded' BEGIN
  UPDATE events SET media_count = media_count - 1 WHERE id = OLD.event_id;
END;
CREATE TRIGGER event_media_au AFTER UPDATE OF source ON event_media
  WHEN (OLD.source = 'user_excluded') <> (NEW.source = 'user_excluded') BEGIN
  UPDATE events
     SET media_count = media_count + (CASE WHEN NEW.source = 'user_excluded' THEN -1 ELSE 1 END)
   WHERE id = NEW.event_id;
END;

-- Yüz kümesi sayaçları
CREATE TRIGGER faces_ai AFTER INSERT ON faces WHEN NEW.cluster_id IS NOT NULL BEGIN
  UPDATE face_clusters SET face_count = face_count + 1 WHERE id = NEW.cluster_id;
END;
CREATE TRIGGER faces_ad AFTER DELETE ON faces WHEN OLD.cluster_id IS NOT NULL BEGIN
  UPDATE face_clusters SET face_count = face_count - 1 WHERE id = OLD.cluster_id;
END;
CREATE TRIGGER faces_au_cluster AFTER UPDATE OF cluster_id ON faces
  WHEN OLD.cluster_id IS NOT NEW.cluster_id BEGIN
  UPDATE face_clusters SET face_count = face_count - 1 WHERE id = OLD.cluster_id;
  UPDATE face_clusters SET face_count = face_count + 1 WHERE id = NEW.cluster_id;
END;

-- Vektör tabloları sanal olduğu için FK desteklemez: silmeyi elle yay.
CREATE TRIGGER faces_ad_vec AFTER DELETE ON faces BEGIN
  DELETE FROM vec_faces WHERE face_id = OLD.id;
END;
CREATE TRIGGER face_clusters_ad_vec AFTER DELETE ON face_clusters BEGIN
  DELETE FROM vec_face_centroids WHERE cluster_id = OLD.id;
END;
CREATE TRIGGER media_files_ad_vec AFTER DELETE ON media_files BEGIN
  DELETE FROM vec_media_clip WHERE media_id = OLD.id;
END;

-- İçerik değişince (referans kütüphanesinde kullanıcı düzenledi) küçük resimler geçersiz
CREATE TRIGGER media_files_au_blake3 AFTER UPDATE OF blake3 ON media_files
  WHEN OLD.blake3 IS NOT NEW.blake3 BEGIN
  DELETE FROM thumbnails WHERE media_id = NEW.id;
  UPDATE sync_status
     SET state = 'queued', conflict_status = 'none', updated_at = NEW.updated_at
   WHERE media_id = NEW.id AND state IN ('synced','local_only');
END;

-- Kullanıcı alanı değişince sidecar yazılmalı
CREATE TRIGGER media_files_au_user AFTER UPDATE OF rating, color_label, is_favorite, flag ON media_files
  WHEN NEW.sidecar_dirty = 0 BEGIN
  UPDATE media_files SET sidecar_dirty = 1 WHERE id = NEW.id;
END;

-- Sürüm düzenlemesi: revision artar (küçük resim önbellek anahtarı)
CREATE TRIGGER edits_ai AFTER INSERT ON edits BEGIN
  UPDATE versions SET revision = revision + 1, updated_at = NEW.created_at WHERE id = NEW.version_id;
END;
CREATE TRIGGER edits_ad AFTER DELETE ON edits BEGIN
  UPDATE versions SET revision = revision + 1 WHERE id = OLD.version_id;
END;
CREATE TRIGGER edits_au AFTER UPDATE ON edits BEGIN
  UPDATE versions SET revision = revision + 1 WHERE id = NEW.version_id;
END;

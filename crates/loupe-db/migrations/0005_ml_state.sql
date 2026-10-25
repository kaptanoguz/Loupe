-- =============================================================================
-- Loupe — Katalog şeması, migrasyon 0005: medya başına AI işlenme durumu
-- Adım 2C (2026-10-25). "Yüz bulunamadı" geçerli bir sonuçtur; faces tablosunda
-- satır olmaması medyanın işlenmediği anlamına gelmez. Bu tablo hangi modelle,
-- ne zaman işlendiğini tutar; model değişince eski sürümle işlenenler
-- yeniden kuyruğa alınabilir (WHERE faces_model_id <> etkin model).
-- =============================================================================

PRAGMA user_version = 5;

CREATE TABLE media_ml_state (
  media_id        INTEGER PRIMARY KEY REFERENCES media_files (id) ON DELETE CASCADE,
  faces_model_id  INTEGER REFERENCES ml_models (id),   -- gömme (ArcFace) modeli
  faces_at        INTEGER,
  face_count      INTEGER CHECK (face_count IS NULL OR face_count >= 0),
  clip_model_id   INTEGER REFERENCES ml_models (id),
  clip_at         INTEGER,
  CHECK ((faces_model_id IS NULL) = (faces_at IS NULL)),
  CHECK ((clip_model_id IS NULL) = (clip_at IS NULL))
) STRICT;

CREATE INDEX media_ml_faces ON media_ml_state (faces_model_id);
CREATE INDEX media_ml_clip  ON media_ml_state (clip_model_id);

-- Küresel işler (cluster_events) koşarken yeniden istenirse istek kaybolmasın:
-- çalışan satırda rerun = 1 olur, iş bitince satır silinmek yerine yeniden
-- 'pending' olur (sondan tetiklemeli debounce, loupe-jobs).
ALTER TABLE jobs ADD COLUMN rerun INTEGER NOT NULL DEFAULT 0 CHECK (rerun IN (0,1));

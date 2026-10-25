-- =============================================================================
-- Loupe — Katalog şeması, migrasyon 0003: CLIP vektör boyutu
-- PR #2 kararı (2026-10-25): görüntü-metin modeli M-CLIP
-- "XLM-Roberta-Large-Vit-B-32" (metin: XLM-RoBERTa Large, görüntü: ViT-B/32).
-- İki kodlayıcı da ortak 512 boyutlu uzaya izdüşürür; 0001'deki 768 yanlıştı.
--
-- Rust tarafındaki tek kaynak: loupe_db::CLIP_DIM (= 512). Test, bu dosyadaki
-- boyutun sabitle aynı olduğunu ve 768'lik vektörün reddedildiğini doğrular.
-- vec_media_clip henüz hiçbir sürümde veri tutmadı (CLIP işi Adım 2'de yok);
-- yine de yeniden oluşturma veriyi atar: clip_embed işleri yeniden kuyruğa girer.
-- =============================================================================

PRAGMA user_version = 3;

DROP TABLE vec_media_clip;
CREATE VIRTUAL TABLE vec_media_clip USING vec0 (
  media_id   INTEGER PRIMARY KEY,
  embedding  float[512] distance_metric=cosine      -- M-CLIP ViT-B/32, L2 normalize
);

-- Farklı boyutlu kayıtlı CLIP modeli varsa devre dışı kalır (ml_models.dim tek kaynak).
UPDATE ml_models SET is_active = 0 WHERE task IN ('clip_image','clip_text') AND dim IS NOT NULL AND dim <> 512;

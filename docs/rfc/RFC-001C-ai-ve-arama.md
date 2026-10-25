# RFC-001C — Çevrimdışı AI, Kişi/Etkinlik Kümeleme ve Hibrit Arama

Durum: Adım 2C (2026-10-25). Crate'ler: `loupe-ai`, `loupe-search`; `loupe-jobs` ve `loupe-db` güncellemeleri.

## 1. Çıkarım katmanı

Kuyruk, kümeleme ve arama yalnızca dört trait'i bilir: `FaceDetector`, `FaceEmbedder`, `ClipImageEncoder`, `ClipTextEncoder`.

| Uygulama | Ne zaman | Notlar |
|---|---|---|
| `DeterministicMockEngine` | CI, birim/entegrasyon testleri | Model dosyası yok, milisaniyelik, her çalıştırmada aynı. Gerçek piksellere bakar: magenta çerçeveli kare = yüz, iç renk = kimlik; CLIP vektörü renk kavramlarından (gün batımı/deniz/orman/gece/kar) gelir, Türkçe ve İngilizce anahtar sözcükler aynı eksene düşer. |
| `onnx::Scrfd` | `ort` özelliği | 640×640 letterbox, adım 8/16/32, konum başına 2 çapa, NMS 0.4. Kalite = boyut (64 px) × güven. |
| `onnx::ArcFace` | `ort` özelliği | 5 noktadan en küçük kareler benzerlik dönüşümüyle 112×112 hizalama, 512 boyut, L2 normalize. |
| `onnx::ClipVision` / `onnx::MclipText` | `ort` özelliği | M-CLIP (XLM-R Large + ViT-B/32), 512 boyut = `CLIP_DIM`; metin `tokenizer.json` ile. |

ONNX Runtime derlemede indirilmez (`load-dynamic`): `ORT_DYLIB_PATH` ya da `onnx::init_runtime`. Model kimliği dosyanın BLAKE3 özetidir (`ml_models.file_blake3`); görev başına tek etkin model.

**Boşta boşaltma.** Her model `ManagedModel` içindedir: ilk çağrıda yüklenir; `idle_timeout` (varsayılan 120 sn) boyunca kullanılmazsa ve hiçbir iş `Arc` ile tutmuyorsa oturum düşürülür. `IdleReaper` iş parçacığı kayıtlı modelleri zayıf referansla yoklar. `ManagedModel<T>` sardığı modelin trait'ini uyguladığı için işleyici modelin yüklü olup olmadığını bilmez.

## 2. Yüz kümeleme

- Girdi orijinal değil `md` küçük resimdir (768 px, yönlendirmesi düzeltilmiş): RAW/HEIC bir kez çözülür, kutular ekrandaki görüntüyle aynı koordinattadır, disk takılı olmasa da AI çalışır.
- **Merkez = üye vektörlerin toplamı** (`vec_face_centroids`, normalize edilmemiş). Kosinüs ölçekten bağımsız olduğu için yön ortalamayla aynıdır; ekleme (+v), çıkarma (−v) ve birleştirme (a + b) kesindir, kayan ortalama hatası birikmez.
- Atama: en yakın 8 merkez (aynı gömme modeli), kosinüs ≥ 0.5 ise katıl, değilse yeni küme. Kalite < 0.3 yüz kayıtlı ama kümesiz kalır.
- Kullanıcı işlemleri: `merge`, `detach_face` (yanlış yüz kendi onaylı kümesine), `move_face`, `name_cluster` (aynı adlı kişi varsa ona bağlanır). Otomatik kümeleme yalnızca kümesiz yüzlere dokunur.
- `persons.face_count` her işlemin sonunda toplu güncellenir (şema kararı: tetikleyici yok).

## 3. Etkinlik algılama

- Zamana göre sırala; 24 sa boşlukta ya da 3 sa üstü boşluk + 50 km üstü yer değişiminde böl; en çok 10 gün; en az 3 fotoğraf.
- Başlık yerel saatle: `Ekim 2026 · Göreme`, `Eylül – Ekim 2026`, `Aralık 2025 – Ocak 2026`. Yer: konumlu fotoğrafların %60'ı aynı yerleşimdeyse yerleşim, değilse ortak il, değilse ortak ülke.
- **Koruma:** `is_locked`, `title_source = 'user'`, `origin = 'manual'` ya da `event_media.source <> 'auto'` olan etkinlik ve fotoğrafları (çıkarılanlar hariç) yeniden kümelemeye hiç girmez. Korumasız otomatik etkinlikler en çok ortak fotoğrafa sahip yeni grupla eşleşip yerinde güncellenir (kimlik sabit), değişmeyen satıra yazılmaz, eşleşmeyen silinir.
- Bilinen sınır: korumalı bir etkinliğin zaman aralığına sonradan eklenen fotoğraflar ona otomatik katılmaz, kendi otomatik etkinliğini oluşturur (kullanıcı `add_media` ile taşır).

## 4. Kuyruk entegrasyonu

- `md` küçük resim üretilince `MediaPipeline` ardıl işleri ekler: `face_detect` ve `clip_embed` (Background), kütüphane başına tek `cluster_events` (Idle, 30 sn debounce).
- `NewJob::with_delay`: bekleyen gecikmeli iş yeniden eklenince başlangıcı ileri kayar (sondan tetiklemeli debounce). Migrasyon 0005 `jobs.rerun`: küresel iş çalışırken yeniden istenirse bitince satır silinmez, yeniden bekler; istek kaybolmaz.
- `JobRouter` iş türünü işleyiciye bağlar; tek işçi havuzu medya hattını ve AI'ı birlikte çalıştırır.
- Migrasyon 0005 `media_ml_state`: medya hangi modelle ne zaman işlendi. "Yüz yok" geçerli sonuçtur; aynı modelle işlenmiş medya atlanır, model değişince yeniden kuyruğa alınabilir.

## 5. Hibrit arama (`loupe-search`)

`SearchQuery` (serde, `deny_unknown_fields`) tek sorguda: kişiler (hepsi/herhangi), yer (ülke/il/ilçe/yerleşim, kutu, yarıçap), zaman (UTC aralık, yerel yıl/ay/gün), etkinlik, en düşük puan, favori, renk etiketi, kamera/objektif, metin, sınır, sıralama.

1. Süzgeçler parametreli SQL'e derlenir. Kişi `v_face_person` üzerinde `EXISTS`; yer adları Türkçe küçük harfle (`tr_lower`, I→ı, İ→i); yarıçap `haversine_km` SQL fonksiyonu; yerel takvim `strftime` + `tz_offset_min`.
2. Metin varsa CLIP metin vektörü (DB kilidi dışında). Süzgeç varsa adayların vektörleriyle **kesin** kosinüs (KNN sonrası süzme sonuç kaçırabilir); yoksa `vec_media_clip` KNN.
3. Sözcük sıralayıcısı: dosya adı, yer hiyerarşisi, etkinlik başlıkları, kişi adları.
4. RRF: `Σ 1 / (60 + sıra)`.

Ölçüm (debug derleme, 3 120 fotoğraf): "Göreme + Ayşe + ≥4 yıldız + gün batımı" ≈ 2–3 ms.

**Akıllı albümler:** `smart_albums.rules` = `SearchQuery` JSON'u, `rules_version = 1`. Bilinmeyen alan ya da sürüm reddedilir; ham SQL hiçbir zaman saklanmaz. Her açılışta yeniden çalışır.

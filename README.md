# Loupe

Linux için yerel öncelikli (local-first) fotoğraf kataloglama, LAN senkronizasyon ve hibrit bulut yedekleme uygulaması.

Durum: Adım 3B — arayüz motorlara bağlandı: tam çözünürlüklü 1:1, Ctrl/⌘+K hibrit arama paleti, Kişiler ve Etkinlikler, Wi-Fi QR eşleştirme, bulut ayarları (Secret Service), age kurtarma anahtarı, Kütüphaneyi Doğrula.

- [RFC-001A — Veritabanı ve Depolama Mimarisi](docs/rfc/RFC-001A-veritabani-ve-depolama.md)
- [RFC-001B — İş Kuyruğu ve AI, LAN Sync, Bulut Yedekleme, Tasarım Sistemi](docs/rfc/RFC-001B-kuyruk-lan-bulut-tasarim.md)
- [RFC-001C — Çevrimdışı AI, Kişi/Etkinlik Kümeleme ve Hibrit Arama](docs/rfc/RFC-001C-ai-ve-arama.md)
- [RFC-001D — LAN Senkronizasyonu ve Şifreli Bulut Yedekleme](docs/rfc/RFC-001D-lan-ve-bulut.md)
- [RFC-001E — Uygulama Kabuğu ve Darkroom Katalog Arayüzü](docs/rfc/RFC-001E-uygulama-kabugu-ve-arayuz.md)
- [RFC-001F — Arayüzü Motorlara Bağlama (Adım 3B)](docs/rfc/RFC-001F-arayuz-motor-baglantisi.md)
- Şema: [`crates/loupe-db/migrations/`](crates/loupe-db/migrations/) · test: `python3 tools/schema-check/check_schema.py crates/loupe-db/migrations`
- Tasarım sistemi: [`apps/desktop/src/app.css`](apps/desktop/src/app.css) · koruma: `tools/slop-guard/slop-guard.sh`

## Crate'ler

| Crate | İçerik |
|---|---|
| `loupe-core` | Kimlikler, `Blake3Hash`, `PHash`, `RelPath` (NFC), şema enum'ları, katalog trait'leri (`ImportCatalog`, `VerifyCatalog`) |
| `loupe-db` | SQLite havuzu (WAL, NORMAL, mmap, foreign_keys, busy_timeout), gömülü migrasyon motoru + doğrulama, sqlite-vec, CRUD, 500'lük `BEGIN IMMEDIATE` toplu yazma |
| `loupe-storage` | Disk/UUID çözümleme ve yeniden konumlandırma, tek geçişte BLAKE3 + pHash, atomik importer (Kopyala/Taşı/Hardlink, `.loupe-tmp/`), debounce'lu izleyici, Verify Library |
| `loupe-media` | EXIF (JPEG, PNG, WebP, HEIC, CR2/NEF/ARW/DNG), saat dilimi ve GPS; DMS biçimleme; çevrimdışı GeoNames R-ağacı; XMP + `.loupe.json` sidecar; RAW gömülü önizleme; `sm`/`md`/`lg` WebP küçük resim (`ThumbnailBackend`, isteğe bağlı `vips`); `thumb://` işleyicisi |
| `loupe-jobs` | SQLite `jobs` üzerinde kalıcı kuyruk, viewport önceliği (`bump_viewport_priority`), işçi havuzu (ayrılmış acil işçi, panik yalıtımı), duraklat/mod/kullanıcı etkinliği kısması, üstel geri çekilme ve dead-letter; debounce'lu küresel işler; `JobRouter`; medya işleyicileri (`md` küçük resimden sonra AI işleri) |
| `loupe-ai` | `FaceDetector`/`FaceEmbedder`/`ClipImageEncoder`/`ClipTextEncoder` trait'leri; `DeterministicMockEngine` (CI); `ort` özelliğiyle SCRFD + ArcFace + M-CLIP; boşta model boşaltma; artımlı yüz kümeleme (birleştir/ayır/taşı/adlandır); `vec_media_clip`; zaman + konum etkinlik algılama (kullanıcı düzenlemesi korunur) |
| `loupe-search` | Kişi (hepsi/herhangi), yer (ülke/il/ilçe/yerleşim, kutu, yarıçap), zaman (aralık, yerel yıl/ay/gün), etkinlik, puan, favori, renk, kamera/objektif ve doğal dil metnini tek sorguda birleştiren hibrit arama (CLIP + sözcük, RRF); JSON kurallı akıllı albümler |
| `loupe-lan` | `_loupe._tcp` mDNS (`Discovery` trait'i, testte bellek içi); QR eşleştirme (60 sn tek kullanımlık jeton, `rcgen` CA, istemci sertifikası imzalama, `devices`); yalnızca mTLS (`rustls`, TLS 1.3, HTTP/2); tus.io v1.0.0 sürdürülebilir yükleme, BLAKE3 doğrulama, atomik içe aktarma; telefonda düzenlenmiş fotoğraf çakışması (KeepBoth / ReplaceWithNewer / Skip / AskUser); hız, kalan süre, doğrulanan dosya telemetrisi |
| `loupe-app` | Masaüstü çekirdeği (Tauri'siz, test edilebilir): `AppCore` havuzu, işçileri, izleyicileri, LAN/bulut servislerini başlatır; kütüphane kökleri (bağlı/çevrimdışı, UUID), içe aktarma oturumları, zaman çizelgesi dilimleri (gün/ay/etkinlik başlıkları, alaka sıralı arama), denetçi ayrıntısı + histogram, puan/etiket/favori → DB + sidecar, `thumb://` ve `thumb://orig`; palet ayrıştırıcısı, kişiler/etkinlikler komutları, AI modu (`onnx`/`mock`/`off`) ve duraklatma, QR + cihazlar, bulut hedefleri + kurtarma anahtarı, Secret Service (yoksa `0600` dosya), Kütüphaneyi Doğrula |
| `apps/desktop/src-tauri` | Tauri v2 kabuğu (`loupe-desktop`): komutlar `spawn_blocking` ile çekirdeğe, asenkron `thumb://` şeması, klasör seçici, kurtarma anahtarı kaydetme penceresi |
| `loupe-cloud` | `age` X25519 akış şifreleme (cihaz + kurtarma alıcısı), okunabilir kurtarma anahtarı; `CloudProvider` trait'i: S3 uyumlu (B2/R2/S3/Wasabi, multipart), WebDAV, yerel/NAS; `sync_queue` + `sync_status` durum makinesi, token bucket MB/sn sınırı, jitter'lı üstel geri çekilme, uzak checksum ya da geri okuma ile BLAKE3 doğrulama, geri yükleme |

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings   # üretim kodunda unwrap/expect/panic derlemeyi kırar
cargo fmt --check
# Gerçek modeller (ONNX Runtime çalışma anında: ORT_DYLIB_PATH=/usr/lib/libonnxruntime.so)
cargo clippy -p loupe-ai --features ort --all-targets -- -D warnings
cargo clippy -p loupe-app --features ort --all-targets -- -D warnings
# Modeller $XDG_DATA_HOME/loupe/models/ altında: scrfd_10g.onnx, arcface_w600k_r50.onnx,
# mclip_image.onnx, mclip_text.onnx, mclip_tokenizer.json. Yoksa AI kapalı açılır
# (etkinlikler yine çalışır). LOUPE_AI=mock deterministik deneme modelini açar,
# LOUPE_SECRETS=file Secret Service'i atlar.

# Arayüz (apps/desktop): Svelte 5 + Tailwind v4
cd apps/desktop && npm ci
npm run dev            # tarayıcıda sahte katalogla; büyük kütüphane: http://localhost:5173/?mock=100000
npm run check && npm test && npm run build
npx tauri dev          # gerçek uygulama (WebKitGTK 4.1 gerekir)
```

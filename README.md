# Loupe

Linux için yerel öncelikli (local-first) fotoğraf kataloglama, LAN senkronizasyon ve hibrit bulut yedekleme uygulaması.

Durum: Adım 2D — Wi-Fi LAN senkronizasyonu (`loupe-lan`) ve şifreli 3-2-1 bulut yedekleme (`loupe-cloud`).

- [RFC-001A — Veritabanı ve Depolama Mimarisi](docs/rfc/RFC-001A-veritabani-ve-depolama.md)
- [RFC-001B — İş Kuyruğu ve AI, LAN Sync, Bulut Yedekleme, Tasarım Sistemi](docs/rfc/RFC-001B-kuyruk-lan-bulut-tasarim.md)
- [RFC-001C — Çevrimdışı AI, Kişi/Etkinlik Kümeleme ve Hibrit Arama](docs/rfc/RFC-001C-ai-ve-arama.md)
- [RFC-001D — LAN Senkronizasyonu ve Şifreli Bulut Yedekleme](docs/rfc/RFC-001D-lan-ve-bulut.md)
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
| `loupe-cloud` | `age` X25519 akış şifreleme (cihaz + kurtarma alıcısı), okunabilir kurtarma anahtarı; `CloudProvider` trait'i: S3 uyumlu (B2/R2/S3/Wasabi, multipart), WebDAV, yerel/NAS; `sync_queue` + `sync_status` durum makinesi, token bucket MB/sn sınırı, jitter'lı üstel geri çekilme, uzak checksum ya da geri okuma ile BLAKE3 doğrulama, geri yükleme |

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings   # üretim kodunda unwrap/expect/panic derlemeyi kırar
cargo fmt --check
# Gerçek modeller (ONNX Runtime çalışma anında: ORT_DYLIB_PATH=/usr/lib/libonnxruntime.so)
cargo clippy -p loupe-ai --features ort --all-targets -- -D warnings
```

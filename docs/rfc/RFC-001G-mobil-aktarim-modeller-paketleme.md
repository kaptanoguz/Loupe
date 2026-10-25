# RFC-001G — Kurulumsuz Telefon Aktarımı, Model Yöneticisi ve Linux Paketleme (Adım 4)

Durum: uygulandı (PR #9) · Önceki: [RFC-001F](RFC-001F-arayuz-motor-baglantisi.md), [RFC-001D](RFC-001D-lan-ve-bulut.md), [RFC-001C](RFC-001C-ai-ve-arama.md)

Adım 3B'de her şey masaüstünde çalışıyordu, ama gerçek kullanım için üç eksik
kalmıştı: telefonda Loupe uygulaması olmadığı için QR'ı okuyacak bir şey
yoktu, ONNX modellerini elle bulup doğru ada koymak gerekiyordu ve
uygulamanın kurulabilir bir paketi yoktu. Bu adım üçünü kapatır.

## 1. Kurulumsuz telefon aktarımı

### 1.1 QR artık bir https adresi

`loupe://pair?…` şemasını telefon kamerası açamaz. QR şimdi şunu taşır:

```
https://192.168.1.20:47213/m#t=<43 karakter belirteç>
```

- Adres seçimi: önce özel IPv4 (telefonlar çoğunlukla oradadır), sonra döngü
  olmayan herhangi bir adres; IPv6 köşeli parantezle (`PairingPayload::web_url`).
- Belirteç **parça** (`#`) kısmındadır. Tarayıcı parçayı istek satırına
  koymaz, sayfa onu gövdede gönderir ve hemen `history.replaceState` ile
  adres çubuğundan siler. Belirteç ne erişim günlüğüne ne geçmişe girer.
- `loupe://` yükü (`to_uri`, `parse`) yerel uygulama için yerinde kalır.

### 1.2 Kimlik: oturum anahtarı

Tarayıcı istemci sertifikası sunamaz. TLS katmanında istemci sertifikası
zaten isteğe bağlıydı (RFC-001D §2.4); yalnızca uç noktalar zorunlu
tutuyordu. Tarayıcı için:

| Uç nokta | Kimlik | İş |
|---|---|---|
| `GET /m`, `/m/app.js`, `/m/core.js`, `/m/blake3.js`, `/m/app.css`, `/m/icon.svg` | yok | ikiliye gömülü sayfa (`include_str!`) |
| `POST /m/session` | QR belirteci | belirteç → 256 bit oturum anahtarı |
| `GET /m/session` | oturum | bu tarayıcı hâlâ eşleşmiş mi |
| `POST /m/precheck` | oturum | "zaten aktarıldı" ön kontrolü |
| `POST /manifest`, `/files…` (tus) | mTLS **ya da** oturum | değişmedi |

- Belirteç eşleşmeyle aynı kurallara uyar: tek kullanımlık, 60 sn, IP başına
  dakikada 5 deneme, her denemede (başarılı ya da değil) tüketilir, hata
  hangi adımın başarısız olduğunu söylemez.
- Anahtar yanıtta döner (sayfa `localStorage`'a yazar, `Authorization:
  Bearer` ile gönderir) ve `loupe_s` çerezi olarak da gelir: `Secure;
  HttpOnly; SameSite=Strict; Max-Age=1 yıl`. localStorage silinse de çerez
  oturumu taşır; başka siteden gelen istek çerezi taşımaz, tus başlıkları
  zaten CORS ön kontrolü gerektirir ve sunucu CORS'a izin vermez.
- **Anahtar saklanmaz.** `devices.cert_sha256 = SHA-256("loupe-web-session-v1\0" ‖ anahtar)`;
  alan ayrımı bir sertifika özetiyle çakışmayı imkânsız kılar. Bu parmak izi
  mTLS cihazlarıyla aynı `DeviceRegistry`'ye girer: erişimi kaldırma, son
  görülme, çakışma politikası ve telemetri hiç değişmeden çalışır.
- Migrasyon 0007: `devices.kind` (`app` | `web`). Masaüstü listesinde
  "android · tarayıcı" görünür. Platform tarayıcının kullanıcı ajanından
  (`android` | `ios` | `desktop`), ad modelden ("Pixel 7 (Chrome)").
- Sayfa sıkı CSP ile gelir: `default-src 'none'; script-src 'self'; …;
  connect-src 'self'; frame-ancestors 'none'`. Satır içi betik yok, dış
  kaynak yok, `eval` yok.

**Sertifika uyarısı.** Sunucu sertifikası bu bilgisayardaki yerel CA'dan
imzalıdır; telefon tarayıcısı ilk açılışta bir kez "bağlantı gizli değil"
der. Bu kaçınılmazdır (LAN IP'si için genel CA sertifikası alınamaz). Wi-Fi
penceresi sunucu sertifikasının SHA-256 parmak izini gösterir; kullanıcı
tarayıcının sertifika ayrıntısıyla karşılaştırıp "Gelişmiş → Devam" der.
Uyarıyı geçmek 60 saniyeyi aşarsa yeni QR'ı taramak yeterlidir: tarayıcı
sertifikayı o kökene zaten kabul etmiştir.

### 1.3 Sayfa (`crates/loupe-lan/web/`)

Bağımlılıksız düz HTML/CSS/JS, toplam ~40 KB. Darkroom belirteçleriyle aynı
renkler, daire ve gölge yok; slop-guard bu dizini de tarar.

Akış: **seç → ön kontrol → BLAKE3 → tus.**

1. `<input type="file" multiple accept="image/*,video/*">` ("Fotoğraf / Video Seç").
2. **Ön kontrol:** varlık kimliği `web:<dosya adı>` (telefon galerisinde ad
   varlığı tanımlar). `POST /m/precheck` aynı ad ve aynı `lastModified` ile
   daha önce görülenleri döndürür; bunlar özetlenmeden "zaten var" olur.
3. **BLAKE3:** WebCrypto'da BLAKE3 yok ve tus oturumu dosyanın BLAKE3'üyle
   açılır. `blake3.js` saf JS, akışlı, resmî test vektörleriyle sınanır
   (vitest); hizalı girdide sözcükler kopyalanmadan okunur (masaüstü
   Chromium'da ~170 MB/s). Dosya 4 MiB dilimlerle okunur, arayüz donmaz.
4. **tus:** `POST /files` (üst veri: ad, kimlik, blake3, değişiklik zamanı).
   Sunucu içerik zaten katalogdaysa (başka yoldan gelmiş olsa bile) `200
   already-synced` der, ağdan bayt geçmez. Değilse 8 MiB `PATCH` parçaları
   (XHR: yükleme ilerlemesi verir). Kopan bağlantıda üstel bekleme (1, 2, 4,
   8, 15, 30 sn…) ve `HEAD` ile sunucunun fsync ettiği yerden devam. Sayfa
   yeniden açılıp aynı dosya seçilirse `POST /files` yarım oturumu bulur ve
   aynı ofsetten sürer.
5. **Çakışma:** aynı ad, farklı içerik = telefonda düzenlenmiş. Cihaz
   politikası "Sor" ise sunucu `409 conflict` döner; sayfa sorar ("İkisini de
   tut / Yeniyle değiştir / Atla", "kalan çakışmalara da uygula") ve kararı
   `resolution` üst verisiyle yeniden gönderir. "Atla" da sunucuya gider: bu
   sürüm "görüldü" olur ve bir daha sorulmaz.
6. Canlı gösterge (font-mono): MB/s (3 sn kayan pencere, yalnızca bu
   oturumda ağdan geçen bayt), kalan süre, aktarılan, atlanan, hata;
   dosya başına satır. Aktarım sürerken ekran kilidi (`wakeLock`) istenir.

**Bilinen sınırlar.**
- iOS fotoğraf seçicisi HEIC'i JPEG'e çevirebilir ve `lastModified`'ı seçim
  anı yapabilir; o zaman ön kontrol atlayamaz, sunucu BLAKE3 ile ayırır.
  Dönüşüm her seferinde aynı baytı üretmezse aynı fotoğraf ikinci kopya
  olarak gelebilir. iOS'ta "Gerçek boyut / En uyumlu değil" seçimi önerilir.
- Kaynak (origin) IP:port'tur. Masaüstünün IP'si değişirse tarayıcı yeni
  kökende oturumsuzdur; QR'ı yeniden taramak yeter, eski cihaz kaydı listede
  kalır (erişimi kaldırılabilir).
- Tarayıcı arka plana alınınca mobil işletim sistemi sekmeyi durdurabilir;
  döndüğünde yükleme `HEAD` ile sürer.

### 1.4 Test

- `crates/loupe-lan/tests/mobile.rs`: QR adresi → sayfa ve CSP → veri uçları
  oturumsuz 401 → belirteç → oturum anahtarı (gövde + çerez), belirteç
  tekrarı 403 → yalnızca çerezle oturum → ön kontrol → 256 KiB parçalı tus,
  ikinci parçada kopma, `HEAD` ile devam → kütüphanede bit bit aynı dosya →
  aynı dosya ön kontrolde atlanır, farklı `lastModified` ile "already-synced"
  → düzenlenmiş dosya 409 + `keep_both` → erişim kaldırılınca 401. Ayrıca
  yanlış belirtecin QR'ı yakması, hız sınırı, uydurma ve aşırı uzun anahtar.
- Gerçek tarayıcı: `make mobile` test sunucusunu açar ve QR adresini yazar.
  Geliştirme sırasında Playwright Chromium (Pixel 7 öykünmesi) ile tam akış
  sürüldü: 3 dosya / 15,3 MB aktarıldı, sayfa yenilenince oturum sürdü,
  yeniden seçimde 3'ü atlandı, düzenlenmiş dosyada çakışma sorusu çıktı.

## 2. Masaüstü değişiklikleri

- Wi-Fi penceresi: "Telefon kamerasıyla tarayın; uygulama kurmak gerekmez",
  sertifika uyarısı açıklaması ve SHA-256 parmak izi (`PairingQrDto.fingerprint`).
- Cihaz listesinde tür (`DeviceDto.kind`).

## 3. Model yöneticisi (`loupe-ai::models`, `loupe-ai::fetch`)

### 3.1 Manifest (`crates/loupe-ai/models.json`, ikiliye gömülü)

| Dosya | Görev | Kaynak | Sabitleme |
|---|---|---|---|
| `scrfd_10g.onnx` | yüz tespiti | InsightFace `buffalo_l.zip` (GitHub v0.7) → `det_10g.onnx` | zip + üye SHA-256 |
| `arcface_w600k_r50.onnx` | yüz vektörü, 512 | aynı zip → `w600k_r50.onnx` | zip + üye SHA-256 |
| `libonnxruntime.so` | ONNX Runtime 1.22.0 (`ort` rc.10 ile aynı API) | Microsoft GitHub `onnxruntime-linux-x64-1.22.0.tgz` | tgz + üye SHA-256 |
| `mclip_image.onnx` | CLIP görüntü, 512 | Hugging Face `immich-app/XLM-Roberta-Large-Vit-B-32` `visual/model.onnx` | LFS özeti |
| `mclip_text.onnx` | CLIP metin (çok dilli), 512 | aynı depo `textual/model.onnx` | LFS özeti |
| `mclip_tokenizer.json` | XLM-R belirteçleyici | aynı depo `textual/tokenizer.json` | LFS özeti |

GitHub kaynaklarının boyutu ve SHA-256'sı bu adımda gerçekten indirilip
ölçüldü ve manifeste yazıldı. Hugging Face bu geliştirme ortamından
erişilemediği için M-CLIP dosyaları **LFS özetiyle** sabitlenir: indirmeden
hemen önce `api/models/<depo>/tree/<sürüm>/<dizin>` okunur, LFS `oid`
(içeriğin SHA-256'sı) ve boyutu `manifest.lock.json`'a yazılır, indirilen
bayt buna göre doğrulanır. Bu, bozulmaya ve yarım indirmeye karşı tam
korumadır; Hugging Face'in kendisine karşı değildir. `loupe-models pin`
çözülmüş değerleri manifest biçiminde yazdırır: bir kez çalıştırılıp
`models.json`'a işlendiğinde M-CLIP de bayt bayt sabitlenmiş olur.

Manifest yüklenirken denetlenir: yalnızca `https` (testte döngü adresine
`http`), 64 haneli hex, sıfır olmayan boyut, düz dosya adı (yol, `..`, gizli
ad yok), arşiv üyesinde `..` yok, arşiv olmayan kaynaktan tek dosya, bilinen
kaynak, tekrar yok.

### 3.2 İndirme ve doğrulama

- Zaten **doğrulanmış** dosyanın kaynağı indirilmez (tekrar çalıştırmak ucuz).
- Kaynak `.partial/<kaynak>.part`'a akar; kopan bağlantıda `Range` ile
  sürer (4 deneme, üstel bekleme). Bildirilen boyutu aşan akış anında kesilir.
- Kaynağın boyutu ve SHA-256'sı tutmazsa yarım dosya silinir, hata
  `Verify` (yeniden denenmez).
- Arşivden üye çıkarılırken yazılan baytın SHA-256'sı hesaplanır;
  tutmazsa çıkarılan dosya silinir. Zip okuyucu merkez dizine güvenir
  (`buffalo_l.zip` veri tanımlayıcısı kullanır, yerel başlıkta boyut sıfır).
- Yalnızca doğrulanan dosya `rename` ile asıl ada geçer, dizin fsync edilir.
  Yarım ya da bozuk bayt asla model dizininde görünmez.
- Bir kaynak başarısız olursa diğerleri yine kurulur; rapor her başarısız
  kaynağı nedeniyle verir.
- `SSL_CERT_FILE` verilirse içindeki kökler de güvenilir sayılır (kurumsal
  vekil sunucu).

### 3.3 Arayüzler

- `scripts/fetch-models.sh` / `make models` → `loupe-models install`
  (`status`, `verify`, `pin` da var; `verify` bozuk dosyada 1 ile çıkar).
- Ayarlar → Yapay zekâ → **Modeller:** dosya, görev, boyut, durum
  (doğrulandı / yok / bozuk / özet bilinmiyor), "Modelleri indir · <bilinen boyut>",
  ilerleme (kaynak, aşama, MB), iptal, başarısız kaynaklar. Durum yoklaması
  dosyaları yeniden özetlemez (boyut + değişiklik zamanı önbelleği).
  Tauri komutları: `models_status`, `models_install`, `models_cancel`.
- AI motoru açılışta seçildiği için modeller hazır olunca "yeniden başlatın"
  denir. Uygulama `ORT_DYLIB_PATH` verilmemişse model dizinindeki
  `libonnxruntime.so`'yu yükler.
- Lisans notu: InsightFace modelleri yalnızca ticari olmayan kullanım içindir;
  arayüz bunu gösterir.

### 3.4 Doğrulama

- `crates/loupe-ai/tests/models.rs` (yerel HTTP sunucusu + sahte Hugging Face
  API'si): zip (deflate + saklı üye) ve tar.gz'den çıkarma; ikinci
  çalıştırmada ağa çıkmama; sunucuda bir baytı değişmiş arşiv ve boyu farklı
  dosyanın reddi (asıl adla hiçbir dosya oluşmaz, yarım dosya silinir, diğer
  kaynaklar kurulur); diskte değiştirilmiş modelin "bozuk" görünüp yeniden
  indirilmesi; yanlış üye özetinin reddi; yarıda kopan indirmenin `Range`
  ile sürmesi; LFS olmayan Hugging Face dosyası; iptal; manifest
  doğrulamasının 15 hatalı biçimi reddetmesi.
- Gerçek indirme (bu ortamda): `loupe-models install` GitHub'dan 288,6 MB
  zip ve 7,8 MB tgz indirdi, SHA-256 doğruladı, üç dosyayı çıkardı.
  `tests/real_models.rs` (elle, `--features ort`): indirilen ONNX Runtime ile
  SCRFD örnek grup fotoğrafında 6 yüz buldu; ArcFace 512 boyutlu, birim
  uzunlukta vektör verdi (aynı yüz 1,000, farklı yüz 0,059 benzerlik).
- M-CLIP gerçek dosyalarla bu ortamda çalıştırılamadı (Hugging Face erişimi
  yok). Metin kodlayıcının girdi adları ve tipleri ilk gerçek indirmede
  `tests/real_models.rs` benzeri bir duman testiyle doğrulanmalıdır.

## 4. Paketleme

- `Makefile`: `dev`, `build`, `bundle`, `models`, `test` (fmt, clippy
  `-D warnings` iki özellik kümesiyle, svelte-check, slop-guard ×2,
  `cargo test --workspace`, vitest), `mobile`, `deps` (Debian/Ubuntu
  paketleri). Varsayılan özellikler `ort,vips`.
- `tauri.conf.json`: sistem başlık çubuklu (decorations) koyu tema pencere,
  ortalanmış 1440×900, en küçük 960×600; `deb` + `AppImage`; deb bağımlılıkları
  `libwebkit2gtk-4.1-0`, `libgtk-3-0`, önerilen `libvips-tools`,
  `gnome-keyring`; kategori Photography.
- İkon: 16'lık ızgarada vizör köşeleri, "L" ve kehribar odak karesi; her
  boyut (32, 128, 256, 512) ızgaranın tam katı olduğu için kenarlar keskin,
  daire yok. Telefon sayfası aynı SVG'yi kullanır.

## 5. Değişmeyenler ve sınırlar

- Yerel uygulama yolu (CSR → istemci sertifikası, mTLS) aynen çalışır.
- Sunucu yine yalnızca LAN adreslerinden bağlantı kabul eder.
- AI modu çalışma anında değişmez; model kurulumu yeniden başlatma ister.

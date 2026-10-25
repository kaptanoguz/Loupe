# RFC-001F — Arayüzü Motorlara Bağlama (Adım 3B)

Durum: uygulandı (PR #8) · Önceki: [RFC-001E](RFC-001E-uygulama-kabugu-ve-arayuz.md), [RFC-001C](RFC-001C-ai-ve-arama.md), [RFC-001D](RFC-001D-lan-ve-bulut.md)

Adım 3A'da kabuk ve katalog arayüzü kuruldu, ama AI, arama, LAN eşleştirme ve
bulut ayarları arayüze açılmamıştı (RFC-001E §2). Bu adım o sınırlamaları
kaldırır: tam çözünürlüklü 1:1, Ctrl/⌘+K paleti, Kişiler ve Etkinlikler
sayfaları, Wi-Fi QR modalı, Ayarlar (bulut, kurtarma anahtarı, doğrulama,
yapay zekâ) ve Secret Service.

## 1. Çekirdek (`loupe-app`) modülleri

| Modül | Sorumluluk |
|---|---|
| `full.rs` | `thumb://orig/<hash>`: 1:1 için tam çözünürlük |
| `search.rs` | Palet ayrıştırıcısı, öneriler, sıralı sonuç önbelleği, akıllı albüm kaydetme |
| `people.rs` | Kişi/küme özetleri, adlandırma, birleştirme, yüz taşıma/ayırma/reddetme, yoksayma |
| `moments.rs` | Etkinlik kartları, yeniden adlandırma, fotoğraf çıkarma, kilit |
| `ai.rs` | `AiMode` seçimi, iş işleyicisi, eksik analizlerin kuyruğa alınması, duraklatma ve kaynak modu |
| `cloud.rs` | Bulut hedefleri, bant genişliği, age kurtarma anahtarı akışı |
| `verify.rs` | Kütüphane başına "Kütüphaneyi Doğrula" iş parçacığı ve rapor |
| `secrets.rs` | `SecretServiceStore`, `LayeredSecretStore` (Secret Service → `0600` dosya) |
| `network.rs` | QR üretimi, cihaz listesi, erişim kaldırma, çakışma politikası, aktarım telemetrisi |

Tauri kabuğu 41 yeni komut ekler. Hepsi `spawn_blocking` ile çekirdeğe gider;
yalnızca `export_recovery_key` kaydetme penceresini kabukta açar.

## 2. `thumb://orig`

**Karar: önizleme değil, özgün piksel.** 3A'da 1:1, `lg` (1920 px) önizlemenin
1:1'iydi. Artık:

- Yönü 1 olan JPEG/PNG/WebP: dosyanın kendi baytları, yeniden kodlama yok.
- Yönü 2–8 olan JPEG: bir kez döndürülür, JPEG q92 olarak
  `cache/full/<hh>/<hash>-o<yön>.jpg`'ye yazılır. Yanıtta EXIF olmadığı için
  WebView ikinci kez döndürmez.
- RAW/DNG: gömülü tam boy önizleme (demosaic yok).
- HEIC, AVIF, JXL, TIFF, video: `415`; arayüz `lg`'de kalır ve bunu durum
  satırında söyler.
- Kütüphane çevrimdışı: `503`. ETag `"<hash>-orig"`, `immutable` önbellek.

Arayüzde 1:1'e geçilince `lg` ekranda kalır, özgün arka planda bir `Image()` ile
yüklenir. Gelince kaydırma (pan) iki görüntünün genişlik oranıyla ölçeklenir;
böylece imlecin altındaki nokta yerinde kalır.

## 3. Komut paleti

```text
deniz kenarında gün batımı person:"Ayşe Yılmaz" place:Göreme rating:4 year:2024..2026
```

- **Ayrıştırma çekirdekte.** Arayüz yalnızca ham girdiyi gönderir
  (`search_palette{input, preview}`); çekirdek belirteçleri (`person`, `place`,
  `event`, `rating`, `color`, `camera`, `lens`, `year`, `favorite` ve Türkçeleri)
  `SearchQuery`'ye çevirir, adları kimliğe çözer ve çipleri döndürür.
  Çözülemeyen belirteç kehribar çip olur ve arama yapılmaz. Kural tek yerde
  kalır: akıllı albüm, görünüm ve palet aynı `SearchQuery`'yi kullanır.
- **Öneriler son belirteç için.** `person:ay` → `person:"Ayşe Yılmaz"`; Tab ilk
  öneriyi alır.
- **Sıralı görünüm.** "Tüm sonuçları aç" `View::Search{query}` açar. Metin varsa
  sıra alakaya göredir; çekirdek RRF sırasını `RankCache`'te (4 yuva, 120 sn)
  tutar ve dilimler `json_each` ile bu sırayla okunur. Tek bölüm: "En alakalı".
- **Akıllı albüm.** Ctrl+↵ ya da görünüm başlığındaki düğme; yalnızca sorgu
  saklanır (`smart_albums.rules`), yeni fotoğraflar eşleştikçe albüme girer.
- **Hızlı komutlar:** Kütüphaneyi Doğrula (Bit-Rot), Wi-Fi Telefon Eşleştir,
  Bulut Senkronizasyonunu Başlat, Yapay Zekâyı Duraklat/Sürdür, Kişiler,
  Etkinlikler, Ayarlar. Bulut komutu hedef yoksa ya da şifreli hedefin kurtarma
  anahtarı yoksa kullanıcıyı doğru Ayarlar sekmesine götürür.
- **Görünüm:** `.palette` sınıfı: düz yüzey, `1px solid #262626`, 4 px köşe,
  gölge ve bulanıklık yok. Arka plan düz karartma (`scrim`).
- sqlite-vec KNN üst sınırı (`k ≤ 4096`) aşıldığında sorgu hata veriyordu;
  `KNN_MAX` ile sınırlandı.

## 4. Kişiler ve Etkinlikler

**Kişiler** kendi sayfasıdır (ızgara değil). Kapaklar `lg` küçük resimden yüz
kutusuna göre kırpılır (`design/face.ts`: yüz payıyla kabı doldurur, kap hep
dolu kalır). 2 px köşeli portre; daire yok.

- Tıkla: seç ve sağ panelde yüzleri göster. Ctrl/⌘+tıkla: çoklu seçim.
  Çift tıkla: kişinin fotoğrafları.
- Birleştirme planı (`mergePlan`): hedef, en çok fotoğraflı adlandırılmış kişi.
  Diğer kişiler `merge_persons` ile, isimsiz kümeler hedefin adıyla
  `name_cluster` ile katılır. Hiç adlı yoksa en büyük kümeye `merge_clusters`.
- Aynı adı vermek birleştirir (`rename_person`, `name_cluster`).
- Yüz işlemleri: başka kişiye/kümeye taşı, ayır (tek yüzlük yeni küme),
  "yüz değil" (yüz ve kendi kümesi silinir), kümeyi yoksay.

**Etkinlikler** kart ızgarasıdır: kapak, başlık, tarih aralığı, sayı, yer.
Yeniden adlandırma ve "Seçili fotoğrafı çıkar" etkinliği kilitler
(`is_locked = 1`); çıkarılan fotoğraf `user_excluded` olarak kalır ve otomatik
kümeleme kilitli etkinliğe bir daha dokunmaz. Kilit elle de açılıp kapanır.

## 5. Yapay zekâ modu

`AiMode::from_env` açılışta bir kez seçilir:

| Mod | Ne zaman | Davranış |
|---|---|---|
| `onnx` | `ort` özelliği + `$XDG_DATA_HOME/loupe/models/` altında SCRFD, ArcFace, M-CLIP görüntü/metin ve tokenizer | Gerçek yüz ve CLIP |
| `mock` | `LOUPE_AI=mock` | Deterministik motor (gösterim, uçtan uca test) |
| `off` | Model yok ya da `LOUPE_AI=off` | Etkinlik kümelemesi yine çalışır; yüz/CLIP işleri atlanır |

Kapalıyken `media_ml_state` yazılmaz; modeller gelince `backfill` eksikleri
yeniden kuyruğa koyar. `backfill` her açılışta etkinlik kümelemesini `Idle`
öncelikte kuyruğa alır. `md` küçük resimden sonraki AI ardıl işleri açıldı.

Palet CLIP yoksa yalnızca sözcük arar ve bunu söyler. Duraklatma
(`resources.paused`) ve kaynak modu (`quiet`/`balanced`/`full`) ayarlarda
saklanır; durum çubuğundaki "AI" göstergesi Ayarlar'ın yapay zekâ sekmesini açar.

## 6. Wi-Fi eşleştirme

- Sol alttaki "Telefon" düğmesi ya da palet açar. Modal açıkken QR jetonu
  üretilir (60 sn, tek kullanımlık) ve kapanınca `cancel_pairing` ile geçersiz
  kılınır.
- **QR yalnızca `<rect>` ve `<path>`:** arka plan tek dikdörtgen, koyu modüller
  tek yol, `shape-rendering="crispEdges"`. Çekirdekte üretilir (`qrcode`,
  varsayılan özellikler kapalı); arayüz `{@html}` ile basar. Kullanıcı girdisi
  içermez.
- Geri sayım `font-mono`; süre bitince kod kendiliğinden yenilenir. Yeni bir
  cihaz eşleşince (2 sn'lik cihaz yoklaması) kod yine yenilenir.
- QR'daki adresler `if-addrs` ile seçilen özel ağ adresleridir. Hiçbiri yoksa
  sunucunun bağlandığı adres kullanılır.
- Cihaz listesi: son görülme, adres, alınan dosya sayısı, çakışma politikası
  (İkisini de tut / Yeniyle değiştir / Atla / Sor), iki adımlı erişim kaldırma.
- Durum çubuğu aktarım sürerken MB/s, kalan süre ve doğrulanan dosya sayısını
  gösterir (`LanStatusDto.bytesExpected/bytesReceived/etaSecs`).

## 7. Bulut ve sırlar

**Secret Service.** `LayeredSecretStore` açılışta Secret Service'i (GNOME
Keyring, KWallet; DBus, DH oturum şifrelemesi) dener; yoksa `0600` dosyalara
düşer. Secret Service sonradan kurulursa dosyadaki değer ilk okumada ona
taşınır ve dosya silinir. `LOUPE_SECRETS=file` servisi hiç denemez; testler
(`AppConfig::in_dir`) gerçek anahtarlığa dokunmaz. `secret-service` crate'inin
`rt-tokio` özelliği tokio çalışma zamanı dışında panik riski taşıdığı için
`rt-async-io-crypto-rust` seçildi.

**Hedefler.** S3, B2, R2, Wasabi (S3 uyumlu: uç nokta, kova, bölge), WebDAV
(adres), yerel klasör (mutlak yol). Gizli olmayan ayar `cloud_targets.config`'te,
erişim anahtarı sır deposunda. Düzenlemede boş bırakılan kimlik bilgisi değişmez;
sağlayıcı değiştirilemez; şifreli hedefte şifreleme kapatılamaz. Hedef başına ve
toplam MB/s sınırı (MB = 1 000 000 bayt) token bucket'a gider. Ayar değişince
bulut işçileri yeniden başlar ve bu sürecin aldığı kiralar (`lease_owner LIKE
'<pid>/cloud-%'`) bırakılır; yoksa kesilen yüklemeler `running`'de takılırdı.

**Kurtarma anahtarı.** Yalnızca bir kez gösterilir:

1. `generate_recovery_key`: anahtar bellekte bekler, ekranda 4'lü gruplar.
2. Kullanıcı kopyalar ya da `0600` dosyaya kaydeder.
3. Rastgele 3 grubu geri yazar (`confirm_recovery_key`). Doğruysa cihaz anahtarı
   kullanılır (yoksa üretilir), kurtarma alıcısı (`age1…`) ve parmak izi
   ayarlara, şifreli hedeflerin alıcısı güncellenir; gizli anahtar bellekten
   düşer.
4. Sonra yalnızca doğrulanabilir (`check_recovery_key`). Yeni anahtar eskisinin
   yerine geçer; eski yedekleri açmak için eski anahtar yine gerekir ve arayüz
   bunu açıkça söyler.

## 8. Kütüphaneyi Doğrula

Kütüphane başına bir iş parçacığı, iptal bayrağıyla. Önceki oturumdan `running`
kalmış çalıştırmalar önce `failed` olur. İptal `cancelled`, hata `failed` olarak
kaydedilir; bozuk dosyalar karantinaya alınır. Ayarlar'ın Doğrulama sekmesi son
raporu (denetlenen, sağlam, kayıp, bozuk, okunamayan, okunan bayt, süre) ve
önceki çalıştırmaları gösterir.

## 9. Testler

| Katman | Test |
|---|---|
| `loupe-app/tests/step3b.rs` | Gerçek işçilerle mock AI: içe aktar → yüz/CLIP → kişiler, palet, etkinlikler; `thumb://orig` (ham bayt, HEAD, döndürülmüş önbellek, 304/404/405); bit-rot doğrulaması; QR + cihazlar + bulut ayarları + kurtarma anahtarı |
| `loupe-desktop` IPC | Yeni komutların adları, argümanları ve camelCase alanları |
| `loupe-search` | `k > 4096` KNN, `keyword_only` |
| Arayüz (vitest) | Palet (öneri, Tab, hatalı çip, sonuç görünümü, akıllı albüm, Bit-Rot komutu), Wi-Fi (yalnızca rect/path, `font-mono` geri sayım, politika, erişim kaldırma, jeton iptali), Kişiler (2 px kapak, adlandırma, birleştirme, yüz ayırma), Etkinlikler (adlandırma kilitler, fotoğraf çıkarma), Ayarlar (kurtarma akışı, sağlayıcı alanları ve MB/s, AI duraklat/mod), Loupe 1:1 özgün yükseltme, `faceCrop`, `mergePlan` |
| CI | `cargo clippy -p loupe-app --features ort` eklendi |

## 10. Bilinçli sınırlamalar

- RAW 1:1 gömülü önizlemedir; demosaic edilmiş tam çözünürlük yok.
- HEIC/AVIF/JXL/TIFF için 1:1 `lg`'de kalır.
- Kişiler sayfası en büyük 200 isimsiz kümeyi gösterir.

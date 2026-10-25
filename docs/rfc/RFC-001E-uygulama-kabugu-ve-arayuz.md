# RFC-001E — Uygulama Kabuğu ve Darkroom Katalog Arayüzü (Adım 3A)

Durum: uygulandı (PR #7) · Önceki: [RFC-001B](RFC-001B-kuyruk-lan-bulut-tasarim.md) §4 tasarım sistemi, [RFC-001D](RFC-001D-lan-ve-bulut.md)

Bu belge Adım 3A'da verilen kararları kaydeder: Tauri v2 kabuğu, çekirdek ile
arayüz arasındaki IPC sözleşmesi ve Svelte 5 katalog arayüzünün dört parçası
(kenar çubuğu, justified ızgara, denetçi, Loupe).

## 1. Katmanlar

```
apps/desktop/src           Svelte 5 (runes) + Tailwind v4 — yalnızca sunum ve klavye
        │  invoke / thumb://
apps/desktop/src-tauri     loupe-desktop — ince kabuk: komut → spawn_blocking → AppCore
        │
crates/loupe-app           AppCore — tüm iş mantığı; Tauri'ye bağımlı değil
        │
loupe-db · loupe-jobs · loupe-storage · loupe-media · loupe-search · loupe-lan · loupe-cloud
```

**Karar: çekirdek ayrı crate.** `loupe-app` Tauri'ye bağımlı değildir; komutların
tamamı WebView açmadan, gerçek SQLite ve gerçek dosya sistemiyle test edilir
(`crates/loupe-app/tests/app.rs`, 7 senaryo). Kabuk yalnızca argümanları çözer,
çağrıyı `spawn_blocking` ile çekirdeğe iletir ve hatayı serileştirir; kabuğun
kendi testi (`src-tauri/tests/ipc.rs`) Tauri'nin sahte çalışma zamanıyla komut
adlarını, camelCase alanları ve hata zarfını uçtan uca doğrular.

### 1.1 AppCore başlatma sırası

1. `AppConfig::from_env()` — `$XDG_DATA_HOME/loupe` (katalog, kimlik, sırlar),
   `$XDG_CACHE_HOME/loupe` (küçük resimler).
2. SQLite havuzu (WAL), migrasyonlar.
3. Kütüphane kökleri: her kök için disk/UUID çözümü; bağlıysa importer açılır.
4. İş kuyruğu + işçiler (`JobRouter`: küçük resim, tam metadata, ters coğrafi kodlama).
5. Her bağlı kütüphane için debounce'lu dosya izleyici.
6. Arka planda ağ: LAN sunucusu (mDNS duyurusu) ve bulut döngüleri.

Pencere kapanınca (`RunEvent::Exit`) servisler sırayla kapatılır.

### 1.2 IPC sözleşmesi

| Komut | Görev |
|---|---|
| `pick_or_add_library_root(path?, name?, managed?)` | Klasör seçici (yol verilmezse) → kök ekle; referans kökse yerinde tarama oturumu döner |
| `pick_import_source` | Kart/telefon klasörü seçici |
| `start_import({libraryId, source, mode})` | `copy` / `move` / `hardlink`; arka planda, oturum kimliği döner |
| `import_status(id)`, `recent_imports`, `cancel_import` | İlerleme `import_sessions` tablosundan |
| `get_timeline_slice({view, grouping, offset, limit})` | Tüm bölüm başlıkları (gün/ay/etkinlik) + istenen sayfa (en çok 5000 öğe): id, hash, en/boy, puan, etiket, favori, RAW biçimi, senkron durumu |
| `prioritize_viewport(ids)` | Görünür öğelerin küçük resim işlerini öne alır (`bump_viewport_priority`) |
| `get_media_inspector_detail(id)` | EXIF (biçimlenmiş: `f/2.8`, `1/500 s`), DMS, yer, yol, boyut, BLAKE3, bulut/age durumu, 64 kutulu histogram |
| `set_rating_label_favorite({ids, rating?, label?, favorite?})` | Tek transaction'da DB; ardından XMP/`.loupe.json` sidecar. Disk çevrimdışıysa `pending`, bağlanınca yazılır |
| `get_sidebar`, `get_status` | Kütüphaneler + görünüm sayıları; durum çubuğu (kuyruk, disk, LAN, bulut) |

Hata her zaman `{code, message}` biçimindedir (`NOT_FOUND`, `LIBRARY_OFFLINE`,
`INVALID`, …); arayüz koda göre dallanır, mesajı durum çubuğunda gösterir.

### 1.3 `thumb://`

`register_asynchronous_uri_scheme_protocol` ile kayıtlıdır: disk okuması ve
eksik boyutun kuyruğa konması WebView iş parçacığında değil, `spawn_blocking`
içinde yapılır. Arayüz `<img src="thumb://sm/{blake3}">` kullanır; JSON/base64
yoktur. Önbellekte olmayan boyut için yanıt 404'tür ve aynı anda `Immediate`
öncelikli küçük resim işi kuyruğa girer. Arayüz aynı adresi (`?r=n`, sorgu
dizesi çözümlemede yok sayılır) 400 ms'den başlayıp ikiye katlanan aralıkla en çok
5 kez yeniden dener; 404 yanıtı `no-store` olduğundan önbelleğe takılmaz.

## 2. Bu adımdaki bilinçli sınırlamalar

> Adım 3B bu sınırlamaların hepsini kaldırdı (favori yalnızca JSON'da kalır);
> bkz. [RFC-001F](RFC-001F-arayuz-motor-baglantisi.md).

- **Sırlar dosyada.** Secret Service (GNOME Keyring/KWallet) entegrasyonu 3B'ye
  kaldı. Bu arada `FileSecretStore` kullanılır: `0700` dizin, `0600` dosyalar,
  doğrulanmış anahtar adları. Bulut kimlik bilgileri ve age cihaz anahtarı yoksa
  hedef `needs_setup` durumunda bekler, hiçbir şey yüklenmez.
- **LAN eşleştirme 3B'de açılır.** Sunucu başlar ve mDNS ile duyurulur, ancak
  eşleştirme jetonu yalnızca QR modalı gösterildiğinde üretilir. QR modalı 3B'de
  geldiğinden şimdilik yeni cihaz eşleşemez; daha önce eşleşmiş cihazlar mTLS ile
  yükleyebilir.
- **AI ardıl işleri kapalı.** Yüz/CLIP modelleri yapılandırılana kadar (3B ayarlar)
  `md` küçük resimden sonra AI işi kuyruğa konmaz. Akıllı albümlerde metin (CLIP)
  kuralları zaman çizelgesinde şimdilik yok sayılır.
- **Favori yalnızca JSON sidecar'da.** XMP'de standart bir favori alanı yok;
  `.loupe.json` `favorite` alanı eklendi.
- **Loupe 1:1, `lg` önizlemenin 1:1'idir** (uzun kenar 1920 px, cihaz pikseli başına
  bir piksel). Orijinal dosyadan tam çözünürlük kırpımı sonraki adımlarda.

## 3. Arayüz

Uygulama doğrudan zaman çizelgesiyle açılır; karşılama ekranı yoktur.

```
┌──────────┬──────────────────────────────────────┬────────────┐
│ 220 px   │ #0A0A0A justified ızgara             │ 280 px (I) │
│ kenar    │ (sanal kaydırma)                     │ denetçi    │
├──────────┴──────────────────────────────────────┴────────────┤
│ 24 px durum çubuğu (font-mono text-xs)                       │
└──────────────────────────────────────────────────────────────┘
```

Kenar çubuğu 240 → **220 px**, denetçi 320 → **280 px** olarak güncellendi
(`app.css` ve `layout.ts` birlikte).

### 3.1 Justified düzen

RFC-001B §4.4 algoritması `src/lib/layout/justified.ts` içinde saf fonksiyondur
ve Web Worker'da çalışır (`justified.worker.ts`). Satır yüksekliği
`h = (W − g·(n−1)) / Σa`; kesim, `|ln(h/Ht)|` en küçük olan noktadır. Tek başına
`0.7·Ht`'den basık kalan panorama kendi satırına geçer, bölümün son satırı
esnetilmez, dolu satırlar kümülatif yuvarlamayla tam `W`'de biter. 100 000 öğe
headless Chromium'da Worker içinde ~16 ms sürer; süre durum çubuğunun sağında
görünür.

Sayfalama: ilk dilim 600 öğe (ilk ekran hızlı), sonra 5000'lik sayfalar. Henüz
gelmemiş öğeler için en/boy 1.5 varsayılır; her sayfada düzen yenilenir ve
ekrandaki öğe (seçiliyse seçili öğe) aynı ekran konumunda tutulur.

### 3.2 Sanal kaydırma (RFC-001B §4.5)

- Her rAF'ta `scrollTop` okunur, satır üst kenarlarında ikili arama yapılır,
  üstte ve altta 1.5 ekran taşma payı tutulur.
- Sabit düğüm havuzu: öğe `i` her zaman `i mod havuz` yuvasında çizilir, böylece
  görünür kaldığı sürece düğümü ve yüklenmiş görüntüsü değişmez. Konum
  `translate3d` ile verilir.
- Boyut sınıfı: `yükseklik × dpr` ≤256 → `sm`, ≤768 → `md`, değilse `lg`.
- Yuva başka öğeye geçince eski `src` bırakılır. Kaydırma 4 ekran/sn'den hızlıysa
  yeni küçük resim istenmez; kaydırma durunca (120 ms) istekler başlar.
- Görünür (taşma payı hariç) öğeler 50 ms debounce ile `prioritize_viewport`'a
  gönderilir; aynı aralık tekrar gönderilmez.

Karo mikro göstergeleri: sol üstte puan ve favori, sağ üstte RAW biçimi
(`CR2`/`NEF`/`ARW`, monospace, 1px çerçeve), sol altta 6×6 renk etiketi, sağ altta
5×5 senkron kutusu (yerel / kuyrukta / yedeklendi / hata / çevrimdışı). Seçim
2px kehribar iç çizgidir; düzen kaymaz.

### 3.3 Klavye

| Tuş | Eylem |
|---|---|
| ← → | önceki/sonraki öğe |
| ↑ ↓ | komşu satırda yatay merkezi en yakın öğe |
| 1–5, 0 | puan, sıfırla |
| 6 7 8 9 | kırmızı, sarı, yeşil, mavi etiket (aynı tuş kaldırır) |
| F | favori aç/kapa |
| I | denetçi |
| Space / Enter / çift tık | Loupe; Space ve Esc kapatır |
| Z veya tık (Loupe) | sığdır ↔ 1:1, sürükleyerek kaydırma |
| Ctrl+= / Ctrl+− | satır yüksekliği 120 / 180 / 260 / 380 |

Eşleme saf bir fonksiyondur (`state/keymap.ts`), durum değişikliği
`Catalog.dispatch` içindedir; ikisi de DOM'suz test edilir. Kürasyon iyimserdir:
karo hemen güncellenir, çekirdeğin döndürdüğü değer esas alınır, hata olursa
geri alınır.

### 3.4 Yazı tipleri

Geist, Inter ve JetBrains Mono uygulamayla paketlenir (SIL OFL 1.1, CDN yok).
Türkçe `ğ ş İ` latin-ext alt kümesinde olduğundan her aile iki `@font-face`
(`unicode-range`) ile yüklenir; dosyalar `scripts/copy-fonts.mjs` ile
`@fontsource-variable` paketlerinden kopyalanır.

### 3.5 Tasarım koruması

`tools/slop-guard/slop-guard.sh` bu adımda **daire** kuralını da denetler
(`<circle>`, `border-radius: 50%`). Lucide ikonları buna göre seçildi (ör. Kişiler
için `scan-face`, Yerler için `map`); arayüz testi DOM'da hiç `circle` olmadığını
doğrular.

## 4. Testler

| Katman | Test |
|---|---|
| `loupe-app` | 7 entegrasyon senaryosu: içe aktarma, zaman çizelgesi bölümleri, `thumb://` + histogram, kürasyon + sidecar + çevrimdışı disk, denetçi biçimleri, etkinlik/kişi/akıllı albüm, referans kütüphane + izleyici, LAN + mDNS + yerel bulut yedeği |
| `loupe-desktop` | Sahte Tauri çalışma zamanında IPC turu: kök ekle → içe aktar → dilim → öncelik → puan/etiket/favori → denetçi → kenar çubuğu → durum → hata zarfı |
| Arayüz (vitest) | justified düzen (tam W, aralık, son satır, panorama, başlıklar, 100k), sanal kaydırma (ikili arama, aralık, boyut sınıfı, ok komşuları, görünür alana kaydırma, hız), klavye eşlemesi, katalog durumu (sayfalama, iptal, iyimser kürasyon ve geri alma), bileşen (açılış, öncelik bildirimi, I/Space/Esc/3, çift tık) |
| `svelte-check` | `--fail-on-warnings` |

## 5. Adım 3B

`Ctrl+K` hibrit arama paleti, kişi/yüz birleştirme arayüzü, Wi-Fi QR eşleştirme
modalı (eşleştirme jetonu burada açılır), bulut ayarları ve Secret Service.

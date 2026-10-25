# RFC-001D — LAN Senkronizasyonu ve Şifreli Bulut Yedekleme

Durum: Adım 2D (2026-10-25). Crate'ler: `loupe-lan`, `loupe-cloud`. `loupe-db` (migrasyon 0006) ve `loupe-storage` (LAN yerleşimi) güncellendi. Tasarım çerçevesi RFC-001B §2–3'tür; bu belge uygulamada verilen kararları kaydeder.

## 1. Adlandırma eşlemesi

| İstekteki ad | Şemadaki ad | Not |
|---|---|---|
| `paired_devices` | `devices` (0002) | Sertifika SHA-256'sı, `revoked_at`, `conflict_policy` zaten burada. |
| `cloud_queue` | `sync_queue` (0002) | Hedef + medya + işlem başına tek satır; kira ve deneme sayacı. |
| `sync_status` | `sync_status` | Medya × hedef başına gerçek durum. |

`devices.conflict_policy` değerleri: `ask` = AskUser, `keep_newest` = ReplaceWithNewer, `keep_both` = KeepBoth, `skip` = Skip.

Migrasyon 0006 yalnızca `cloud_targets.provider` CHECK'ine `local` (yerel disk / NAS bağlama noktası) ekler. SQLite CHECK değiştiremediği için tablo yeniden kurulur; `DROP TABLE` alt tablolardaki satırları cascade ile silerdi, bu yüzden `sync_queue` ve `sync_status` önce TEMP tablolara kopyalanır, sonra geri yazılır. Test: `migration_0006_keeps_sync_rows_and_accepts_local_targets`.

## 2. `loupe-lan`

### 2.1 Kimlik ve keşif

- Kurulum başına P-256 CA (10 yıl, `pathlen 0`) ve sunucu sertifikası (1 yıl, SAN `loupe.local`). İstemciler sunucuyu IP'yle değil bu sabit adla doğrular; DHCP adresi değişse de eşleşme bozulmaz.
- mDNS `_loupe._tcp.local.`; TXT: `v`, kurulum kimliği, CA özetinin ilk 16 baytı. Keşif `Discovery` trait'inin arkasındadır: `MdnsDiscovery` (`mdns` özelliği, varsayılan açık) ve testler için `MemoryDiscovery`. CI `--no-default-features` ile de derler.
- Yalnızca LAN adreslerinden (RFC 1918, link-local, loopback, IPv6 ULA) gelen bağlantılar kabul edilir.

### 2.2 QR eşleştirme

```
loupe://pair?v=1&a=192.168.1.20,fd00::5&p=47321&ca=<CA SPKI SHA-256>&fp=<sunucu sertifikası SHA-256>&t=<jeton>&x=<bitiş, sn>&n=<ad>
```

- Jeton 32 rastgele bayttır (base64url, 43 karakter), 60 sn geçerlidir ve tek kullanımlıktır. Bellekte yalnızca BLAKE3 özeti tutulur, karşılaştırma sabit zamanlıdır.
- QR'da iki sabitleme vardır: `fp` (istenen sunucu sertifikası parmak izi) eşleştirme bağlantısını doğrular; `ca` (RFC-001B) kalıcı sabitlemedir, sunucu sertifikası yenilense de geçerli kalır.
- Akış: telefon `fp` sabitlemesiyle TLS açar ve `POST /pair` ile jeton + P-256 CSR gönderir. Sunucu jetonu tüketir (başarısız denemede de), `PairingApprover` onayını alır, CSR'ı CA ile imzalar (2 yıl, yalnızca ClientAuth; CSR uzantıları yok sayılır), sertifika özetini `devices`'a yazar.
- Tekrar kullanılan, süresi dolmuş ya da yanlış jeton her zaman aynı yanıtı alır: `403 PAIRING_FAILED`, boş mesaj. Saldırgan hangi durumun gerçekleştiğini öğrenemez. IP başına dakikada 5 deneme (`429`).
- `/pair` dışındaki her uç mTLS ister. `DeviceCertVerifier`, CA zincirinin yanında sertifikanın `devices` tablosunda kayıtlı ve iptal edilmemiş olmasını da denetler; iptal, açık bağlantıyı bir sonraki istekte keser.

### 2.3 tus.io v1.0.0

| Uç | İş |
|---|---|
| `OPTIONS /files` | Sürüm ve uzantılar (`creation`, `termination`, `checksum` blake3) |
| `POST /files` | `Upload-Length` + `Upload-Metadata` (`filename`, `device_asset_id`, `blake3`, `modified_at`, `taken_at`, `resolution`). Önce çakışma sınıflandırılır (§2.4). |
| `HEAD /files/:id` | `Upload-Offset` |
| `PATCH /files/:id` | `application/offset+octet-stream`, en çok 8 MiB; isteğe bağlı `Upload-Checksum: blake3 …` |
| `DELETE /files/:id` | İptal, hazırlık dosyası silinir |
| `POST /manifest` | Telefonun toplu sorusu: hangi varlıklar zaten var, hangileri yeni, hangileri çakışıyor |

- Hazırlık alanı `<kök>/.loupe/lan-staging/<upload_id>/<dosya>`; kütüphane diskinde olduğu için içe aktarma `rename` ile atomiktir (`ImportMode::Move`, `Placement::Lan`).
- **Kopan PATCH'in kısmi baytları atılır.** Ofset yalnızca `fsync`'ten sonra veritabanına yazılır; bağlantı yarıda koparsa dosya son kaydedilen ofsete kesilir (`set_len`). `HEAD` her zaman diskte kalıcı olan baytı söyler; bu yüzden "1 MiB'ten sonra kopma" testinde `HEAD` tam olarak 1048576 döner.
- Son bayt gelince dosyanın tamamı BLAKE3 ile özetlenir, metadata'daki özetle karşılaştırılır (`460` uyuşmazlık), `mtime` ayarlanır ve `loupe-storage` atomik importer'ı çağrılır. Medya satırı, `device_assets` ve aktarım durumu tek transaction'dadır.
- Tamamlanan yanıt: `204`, `loupe-sync-status: verified`, `loupe-media-id`. Tamamlanmış yüklemeye PATCH `410` alır. Yarım yüklemeler 7 gün sonra temizlenir.

### 2.4 Telefonda düzenlenmiş fotoğraf çakışması

| Durum | Yanıt |
|---|---|
| Aynı `device_asset_id`, aynı BLAKE3 (ya da kütüphanede aynı BLAKE3) | `200`, `loupe-sync-status: already-synced` |
| Aynı kimlik, farklı BLAKE3, politika `ask` | `409`, `loupe-sync-status: conflict`. Telefon `resolution` ile yeniden sorar. |
| `keep_both` | Yeni dosya `_1` ekiyle yanına alınır, ikisi aynı `near_duplicate_groups` grubuna girer |
| `keep_newest` (ReplaceWithNewer) | Yeni dosya alınır, öncekisi çöpe (soft delete) taşınır; geri alınabilir |
| `skip` | `200`, `loupe-sync-status: skipped` |

### 2.5 Telemetri

`Telemetry::subscribe()` bir `watch` kanalı verir: etkin yükleme sayısı, beklenen/alınan bayt, bayt/sn (5 sn pencereli üstel ortalama), kalan süre, doğrulanan dosya, zaten eşitli, atlanan, çakışma, hata.

## 3. `loupe-cloud`

### 3.1 Şifreleme ve kurtarma anahtarı

- Her nesne standart `age` v1 akışıdır ve iki X25519 alıcısına şifrelenir: cihaz anahtarı (Secret Service'te) ve kurtarma anahtarı (yalnızca açık anahtarı `cloud_targets.age_recipient`'ta).
- Kurtarma anahtarı `AGE-SECRET-KEY-1…` biçimindedir; yazdırırken 4'lü gruplara bölünür. Geri yazarken boşluk, tire ve küçük harf serbesttir; bech32 sağlaması tek harf hatasını yakalar. "Kaydettiğini kanıtla" adımı için `challenge(n)` / `verify_challenge`.
- Uygulama olmadan kurtarma: `age -d -i kurtarma.txt objects/9f/2c/9f2c….age > foto.jpg`.
- Şifreli anahtar yolu `önek/objects/xx/yy/<blake3>.age` (dosya adı sızmaz, yeniden adlandırma yeniden yükleme gerektirmez); şifresiz hedefte `önek/<rel_path>`.

### 3.2 Sağlayıcılar

`CloudProvider`: `upload_stream`, `download_stream`, `head_object`, `delete_object`.

| Sağlayıcı | Yükleme | Uzak checksum |
|---|---|---|
| `local` (disk / NAS) | `.part` dosyası, `fsync`, `rename`, dizin `fsync` | SHA-256 |
| S3 uyumlu (S3, B2, R2, Wasabi) | 8 MiB altı tek imzalı `PUT` (`Content-MD5`); üstü multipart, hata olursa `AbortMultipartUpload` | ETag: MD5 ya da `md5(parça md5'leri)-N` |
| WebDAV | Üst koleksiyonlar `MKCOL`, tek akışlı `PUT` | Yok, geri okunur |

### 3.3 Kuyruk, hız sınırı, geri çekilme

- Durum geçişi ve kuyruk değişikliği aynı transaction'dadır: `queued` → `uploading` (kira, 30 dk) → `synced` (kuyruk satırı silinir) / `queued` + `failed` (geri çekilme) / `error` + `dead`.
- Geri çekilme: `min(6 sa, 2^deneme × 30 sn)`, ×[0.5, 1.0] jitter, en çok 12 deneme; sunucunun `Retry-After`'ı önceliklidir. Yetki, kota ve bozuk yerel dosya yeniden denenmez.
- Hız sınırı: token bucket (MB/sn), hedef başına (`cloud_targets.bandwidth_bps`) ve isteğe bağlı küresel; şifreli akışın her 1 MiB parçası jeton bekler.

### 3.4 Doğrulama

- Yükleme sırasında dosya okunurken BLAKE3 hesaplanır; `media_files.blake3` ile uyuşmazsa akış hata ile kesilir ve bozuk dosya buluta **hiç ulaşmaz** (`LocalCorrupt`, yeniden denenmez).
- `VerifyMode::Checksum` (varsayılan): sağlayıcının `head` checksum'ı yükleme sırasında beklenen değerle karşılaştırılır. Sağlayıcı checksum vermiyorsa (WebDAV) ya da ETag uyuşmazsa, ya da `VerifyMode::ReadBack` seçildiyse nesne geri indirilir, cihaz anahtarıyla çözülür ve BLAKE3 yerel özetle karşılaştırılır (`remote_checksum_algo = blake3`).
- Başarılı doğrulama `sync_status`'a yazılır: `state = synced`, `encryption = age`, `key_fingerprint`, `remote_key`, `remote_size`, `remote_checksum`, `uploaded_at`, `last_verified_at`.
- `restore(media_id, kimlik, hedef)` nesneyi indirir, verilen kimlikle (cihaz ya da kurtarma anahtarı) çözer ve BLAKE3'ü doğrulamadan dosyayı yerine koymaz.

## 4. Testler

| Test | Kapsam |
|---|---|
| `qr_pairing_issues_client_cert_and_rejects_replay_and_expired_tokens` | Geçerli eşleştirme ve mTLS isteği; tekrar kullanılan ve süresi dolmuş jeton `403`; sertifikasız istemci reddi; iptal |
| `upload_resumes_after_disconnect_and_lands_verified_in_library` | 2 MiB fotoğraf, 1 MiB sonra kopma, `HEAD` = 1048576, devam, BLAKE3 doğrulamalı içe aktarma, telemetri, tekrar yüklemede already-synced |
| `edited_on_phone_conflict_follows_device_policy` | ask / keep_both / keep_newest / skip ve manifest |
| `encrypted_backup_round_trip_restores_with_recovery_key` | Şifrele → yükle → `sync_status` → indir → kurtarma anahtarıyla çöz → BLAKE3 |
| `corrupt_local_file_never_reaches_the_cloud` | Bozuk yerel dosya yüklenmez, `error` |
| `offline_library_is_retried_with_backoff` | Takılı olmayan disk, geri çekilme ve yeniden deneme |
| `s3_provider_single_put_multipart_and_abort` | Sahte S3 sunucusunda tek PUT, multipart, iptal |
| `webdav_target_is_verified_by_reading_back` | Checksum vermeyen hedefte geri okuma doğrulaması |

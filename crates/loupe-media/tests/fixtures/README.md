# loupe-media test fikstürleri

`tools/fixtures/make_media_fixtures.py` ile üretilir; elle düzenlemeyin.

## exif/

Pillow + piexif ile yazılmış JPEG'ler. EXIF alanları gerçek kamera
gövdelerinin yazdığı biçimdedir (Canon EOS R6, Nikon Z 6II, iPhone).
Görüntü içeriği sentetiktir; telif içermez.

| Dosya | Sınadığı |
|---|---|
| `goreme_canon_r6.jpg` | Kamera/lens, f/8, 1/250 s, ISO 100, `OffsetTimeOriginal +03:00`, GPS Göreme (K/D), irtifa |
| `buenos_aires_gps_time.jpg` | Saat farkının GPS zaman damgasından bulunması (−03:00), G/B yarıküre işareti |
| `orientation_6.jpg` | EXIF Orientation 6; saklanan piksellerde sol üst kırmızı, sağ alt mavi |

## geonames/

GeoNames döküm biçiminde (sekmeyle ayrılmış, 19 sütun) küçük bir alt küme:
Kapadokya ve İstanbul çevresindeki tüm `cities500` yerleşimleri, Ankara,
Berlin, Buenos Aires ve 180. boylam çevresindeki Fiji/Çukotka yerleşimleri.

- Kaynak: [GeoNames](https://www.geonames.org/) verisi, CC BY 4.0. Dosyalar
  `geonamescache` 3.0.2 (MIT) paketinin dağıttığı `cities500.json` ve
  `countries.json` dosyalarından türetilmiştir.
- `admin1CodesASCII.txt` yalnızca testlerin kullandığı kodları içerir; adlar
  GeoNames'teki adlardır, geonameid sütunu boş bırakılmıştır.
- Özellik sınıfı (`P`) ve kodu (`PPL`) kaynakta bulunmadığı için sabittir.

Üretimde tam döküm `download.geonames.org/export/dump/` adresinden indirilir
(`cities500.zip`, `admin1CodesASCII.txt`, `admin2Codes.txt`, `countryInfo.txt`).

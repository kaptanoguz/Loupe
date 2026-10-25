#!/usr/bin/env python3
"""loupe-media test fikstürlerini üretir.

Kullanım:
    python3 tools/fixtures/make_media_fixtures.py <geonamescache/data dizini>

- EXIF JPEG'leri: Pillow + piexif ile, gerçek kamera alanlarıyla.
- GeoNames alt kümesi: geonamescache 3.0.2 (MIT) paketindeki cities500.json
  (GeoNames, CC BY 4.0) verisinden, GeoNames döküm biçiminde (19 sütun).
Çıktı: crates/loupe-media/tests/fixtures/{exif,geonames}/
"""

import json
import sys
from fractions import Fraction
from pathlib import Path

import piexif
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[2] / "crates/loupe-media/tests/fixtures"


def rat(x, limit=10000):
    f = Fraction(x).limit_denominator(limit)
    return (f.numerator, f.denominator)


def dms(value):
    value = abs(value)
    d = int(value)
    m = int((value - d) * 60)
    s = (value - d - m / 60) * 3600
    return ((d, 1), (m, 1), (round(s * 1000), 1000))


def gps_ifd(lat, lon, alt=None, date=None, time=None):
    g = {
        piexif.GPSIFD.GPSVersionID: (2, 3, 0, 0),
        piexif.GPSIFD.GPSLatitudeRef: b"N" if lat >= 0 else b"S",
        piexif.GPSIFD.GPSLatitude: dms(lat),
        piexif.GPSIFD.GPSLongitudeRef: b"E" if lon >= 0 else b"W",
        piexif.GPSIFD.GPSLongitude: dms(lon),
    }
    if alt is not None:
        g[piexif.GPSIFD.GPSAltitudeRef] = 0 if alt >= 0 else 1
        g[piexif.GPSIFD.GPSAltitude] = rat(abs(alt))
    if date:
        g[piexif.GPSIFD.GPSDateStamp] = date.encode()
        g[piexif.GPSIFD.GPSTimeStamp] = tuple((v, 1) for v in time)
    return g


def scene(w, h):
    img = Image.new("RGB", (w, h))
    px = img.load()
    for y in range(h):
        for x in range(w):
            px[x, y] = (40 + x * 180 // w, 90 + y * 120 // h, 200 - x * 120 // w)
    d = ImageDraw.Draw(img)
    d.ellipse((w * 0.6, h * 0.15, w * 0.75, h * 0.38), fill=(230, 80, 40))
    d.polygon([(0, h), (w * 0.3, h * 0.55), (w * 0.55, h)], fill=(150, 110, 80))
    return img


def write_jpeg(path, img, exif):
    path.parent.mkdir(parents=True, exist_ok=True)
    img.save(path, "JPEG", quality=85, exif=piexif.dump(exif))
    print(path.relative_to(ROOT.parents[2]), path.stat().st_size, "bayt")


def exif_fixtures():
    out = ROOT / "exif"
    # 1. Kapadokya'da sıcak hava balonu çekimi: tam alan seti.
    write_jpeg(out / "goreme_canon_r6.jpg", scene(1200, 800), {
        "0th": {
            piexif.ImageIFD.Make: b"Canon",
            piexif.ImageIFD.Model: b"Canon EOS R6",
            piexif.ImageIFD.Orientation: 1,
            piexif.ImageIFD.Software: b"Firmware Version 1.8.1",
            piexif.ImageIFD.DateTime: b"2025:10:24 06:42:17",
        },
        "Exif": {
            piexif.ExifIFD.DateTimeOriginal: b"2025:10:24 06:42:17",
            piexif.ExifIFD.OffsetTimeOriginal: b"+03:00",
            piexif.ExifIFD.SubSecTimeOriginal: b"25",
            piexif.ExifIFD.FNumber: (8, 1),
            piexif.ExifIFD.ExposureTime: (1, 250),
            piexif.ExifIFD.ISOSpeedRatings: 100,
            piexif.ExifIFD.FocalLength: (35, 1),
            piexif.ExifIFD.LensModel: b"RF24-105mm F4 L IS USM",
            piexif.ExifIFD.PixelXDimension: 1200,
            piexif.ExifIFD.PixelYDimension: 800,
        },
        "GPS": gps_ifd(38.64312, 34.82897, alt=1104.5),
    })

    # 2. Saat farkı yalnızca GPS zaman damgasından çıkarılabilir; güney/batı yarıküre.
    write_jpeg(out / "buenos_aires_gps_time.jpg", scene(640, 480), {
        "0th": {piexif.ImageIFD.Make: b"NIKON CORPORATION", piexif.ImageIFD.Model: b"NIKON Z 6_2"},
        "Exif": {
            piexif.ExifIFD.DateTimeOriginal: b"2025:03:01 10:00:00",
            piexif.ExifIFD.FNumber: (28, 10),
            piexif.ExifIFD.ExposureTime: (1, 1000),
            piexif.ExifIFD.ISOSpeedRatings: 200,
            piexif.ExifIFD.FocalLength: (50, 1),
            piexif.ExifIFD.LensModel: b"NIKKOR Z 50mm f/1.8 S",
        },
        "GPS": gps_ifd(-34.60372, -58.38159, date="2025:03:01", time=(13, 0, 0)),
    })

    # 3. Orientation 6 (90° saat yönünde döndürülerek gösterilmeli).
    # Saklanan piksellerde sol üst köşe kırmızı, sağ alt köşe mavi.
    img = Image.new("RGB", (300, 200), (255, 255, 255))
    d = ImageDraw.Draw(img)
    d.rectangle((0, 0, 59, 59), fill=(255, 0, 0))
    d.rectangle((240, 140, 299, 199), fill=(0, 0, 255))
    write_jpeg(out / "orientation_6.jpg", img, {
        "0th": {piexif.ImageIFD.Make: b"Apple", piexif.ImageIFD.Model: b"iPhone 15", piexif.ImageIFD.Orientation: 6},
        "Exif": {piexif.ExifIFD.DateTimeOriginal: b"2025:07:14 18:20:00"},
    })


ADMIN1 = {
    "TR.34": "Istanbul",
    "TR.50": "Nevşehir",
    "TR.68": "Ankara",
    "DE.16": "Berlin",
    "AR.07": "Buenos Aires F.D.",
    "RU.15": "Chukotka",
    "FJ.01": "Central",
    "FJ.02": "Eastern",
    "FJ.03": "Northern",
}


def geonames_fixtures(data_dir):
    cities = json.load(open(data_dir / "cities500.json", encoding="utf-8"))
    countries = json.load(open(data_dir / "countries.json", encoding="utf-8"))

    def keep(v):
        lat, lon = v["latitude"], v["longitude"]
        return (
            (38.3 <= lat <= 39.0 and 34.4 <= lon <= 35.2)  # Kapadokya
            or (40.8 <= lat <= 41.3 and 28.5 <= lon <= 29.5)  # İstanbul
            or v["geonameid"] in (323786, 2950159, 3435910)  # Ankara, Berlin, Buenos Aires
            or (abs(lon) > 178.5 and v["countrycode"] in ("FJ", "RU"))  # 180. boylam
        )

    rows = sorted((v for v in cities.values() if keep(v)), key=lambda v: v["geonameid"])
    out = ROOT / "geonames"
    out.mkdir(parents=True, exist_ok=True)
    with open(out / "cities500.txt", "w", encoding="utf-8", newline="\n") as f:
        for v in rows:
            cols = [
                str(v["geonameid"]), v["name"], v["name"], "",
                f'{v["latitude"]:.5f}', f'{v["longitude"]:.5f}', "P", "PPL",
                v["countrycode"], "", v["admin1code"], "", "", "",
                str(v["population"]), "", "", v["timezone"], "2020-01-01",
            ]
            assert len(cols) == 19
            f.write("\t".join(cols) + "\n")
    with open(out / "admin1CodesASCII.txt", "w", encoding="utf-8", newline="\n") as f:
        for code, name in ADMIN1.items():
            f.write(f"{code}\t{name}\t{name}\t\n")
    used = sorted({v["countrycode"] for v in rows})
    with open(out / "countryInfo.txt", "w", encoding="utf-8", newline="\n") as f:
        f.write("#ISO\tISO3\tISO-Numeric\tfips\tCountry\tCapital\n")
        for cc in used:
            c = countries[cc]
            f.write(f'{c["iso"]}\t{c["iso3"]}\t{c["isonumeric"]:03d}\t{c["fips"]}\t{c["name"]}\t{c["capital"]}\n')
    print(f"geonames: {len(rows)} yerleşim, {len(used)} ülke")


if __name__ == "__main__":
    exif_fixtures()
    if len(sys.argv) > 1:
        geonames_fixtures(Path(sys.argv[1]))

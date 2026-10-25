//! Koordinat biçimleme ve çevrimdışı ters coğrafi kodlama.
//!
//! GeoNames döküm dosyaları (sekmeyle ayrılmış, 19 sütun) belleğe yüklenir;
//! her yerleşim birim küre üzerinde 3B noktaya çevrilip R-ağacına (`rstar`)
//! konur. Kiriş uzaklığı büyük daire uzaklığıyla aynı sırayı verdiği için en
//! yakın komşu sorgusu doğrudan doğru yerleşimi bulur; kutup ve 180. boylam
//! dikişinde de hata yapmaz.
//!
//! Beklenen dosyalar (<https://download.geonames.org/export/dump/>):
//! `cities500.txt` (veya `cities1000/5000/15000.txt`), `admin1CodesASCII.txt`,
//! `admin2Codes.txt` (isteğe bağlı) ve `countryInfo.txt`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use rstar::RTree;
use rstar::primitives::GeomWithData;

use crate::MediaError;

const EARTH_RADIUS_KM: f64 = 6371.0088;

/// Varsayılan arama yarıçapı: bunun ötesindeki en yakın yerleşim (açık deniz,
/// çöl) eşleşme sayılmaz.
pub const DEFAULT_MAX_DISTANCE_KM: f64 = 50.0;

/// `38° 38' 34.2" N, 034° 49' 43.9" E` biçimi.
///
/// Saniye onda bire yuvarlanır; yuvarlama taşması (59.95") dakikaya ve dereceye
/// aktarılır, böylece `60.0"` asla görünmez.
pub fn format_dms(lat: f64, lon: f64) -> Result<String, MediaError> {
    if !valid_coordinate(lat, lon) {
        return Err(MediaError::InvalidCoordinate { lat, lon });
    }
    Ok(format!("{}, {}", dms_axis(lat, 2, ['N', 'S']), dms_axis(lon, 3, ['E', 'W'])))
}

fn valid_coordinate(lat: f64, lon: f64) -> bool {
    lat.is_finite() && lon.is_finite() && (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)
}

fn dms_axis(value: f64, deg_width: usize, hemi: [char; 2]) -> String {
    let h = if value < 0.0 { hemi[1] } else { hemi[0] };
    // Aralık önceden doğrulandı: |değer| ≤ 180 → en çok 6 480 000 onda-saniye.
    let tenths = (value.abs() * 36_000.0).round() as u64;
    let deg = tenths / 36_000;
    let min = (tenths % 36_000) / 600;
    let sec_tenths = tenths % 600;
    format!("{deg:0deg_width$}° {min:02}' {:02}.{}\" {h}", sec_tenths / 10, sec_tenths % 10)
}

/// Çözülmüş yer. `admin1` il/eyalet, `admin2` ilçe, `name` yerleşimin kendisi.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub geoname_id: i64,
    pub name: String,
    pub country_code: String,
    pub country: String,
    pub admin1: Option<String>,
    pub admin2: Option<String>,
    pub lat: f64,
    pub lon: f64,
    pub population: u64,
    /// Sorgu noktasına büyük daire uzaklığı.
    pub distance_km: f64,
}

impl Place {
    /// Kullanıcıya gösterilecek kısa adres: `Ürgüp, Nevşehir, Türkiye`.
    #[must_use]
    pub fn display(&self) -> String {
        let mut parts: Vec<&str> = vec![&self.name];
        if let Some(a2) = self.admin2.as_deref().filter(|a| *a != self.name) {
            parts.push(a2);
        }
        if let Some(a1) = self.admin1.as_deref().filter(|a| *a != self.name) {
            parts.push(a1);
        }
        parts.push(&self.country);
        parts.join(", ")
    }
}

#[derive(Debug)]
struct City {
    geoname_id: i64,
    name: String,
    country_code: String,
    admin1_code: String,
    admin2_code: String,
    lat: f64,
    lon: f64,
    population: u64,
}

type Node = GeomWithData<[f64; 3], u32>;

/// Bellek içi GeoNames dizini. Yükleme bir kez yapılır, sorgular `&self` ile
/// eşzamanlı çalışır.
#[derive(Debug)]
pub struct GeoIndex {
    tree: RTree<Node>,
    cities: Vec<City>,
    admin1: HashMap<String, String>,
    admin2: HashMap<String, String>,
    countries: HashMap<String, String>,
    dataset_ver: String,
    max_distance_km: f64,
}

impl GeoIndex {
    /// Dizindeki standart GeoNames dosyalarını yükler. Şehir dosyası zorunlu,
    /// diğerleri yoksa ilgili adlar kod olarak kalır.
    pub fn load_dir(dir: &Path) -> Result<Self, MediaError> {
        let cities = ["cities500.txt", "cities1000.txt", "cities5000.txt", "cities15000.txt"]
            .iter()
            .map(|n| dir.join(n))
            .find(|p| p.is_file())
            .ok_or_else(|| MediaError::GeoData {
                path: dir.join("cities500.txt"),
                line: 0,
                reason: "şehir dosyası bulunamadı".into(),
            })?;
        let opt = |name: &str| Some(dir.join(name)).filter(|p| p.is_file());
        Self::from_files(
            &cities,
            opt("admin1CodesASCII.txt").as_deref(),
            opt("admin2Codes.txt").as_deref(),
            opt("countryInfo.txt").as_deref(),
        )
    }

    pub fn from_files(
        cities: &Path,
        admin1: Option<&Path>,
        admin2: Option<&Path>,
        country_info: Option<&Path>,
    ) -> Result<Self, MediaError> {
        let read = |p: &Path| fs::read_to_string(p).map_err(MediaError::io("GeoNames okuma", p));
        let cities_txt = read(cities)?;
        let hash = blake3::hash(cities_txt.as_bytes()).to_hex();
        let stem = cities.file_stem().and_then(|s| s.to_str()).unwrap_or("cities");
        let dataset_ver = format!("geonames-{stem}-{}", &hash.as_str()[..12]);

        let parsed_cities = parse_cities(&cities_txt, cities)?;
        let admin1 = admin1.map(|p| read(p).map(|t| parse_codes(&t))).transpose()?.unwrap_or_default();
        let admin2 = admin2.map(|p| read(p).map(|t| parse_codes(&t))).transpose()?.unwrap_or_default();
        let countries = country_info.map(|p| read(p).map(|t| parse_countries(&t))).transpose()?.unwrap_or_default();
        Ok(Self::build(parsed_cities, admin1, admin2, countries, dataset_ver))
    }

    fn build(
        cities: Vec<City>,
        admin1: HashMap<String, String>,
        admin2: HashMap<String, String>,
        countries: HashMap<String, String>,
        dataset_ver: String,
    ) -> Self {
        let nodes = cities
            .iter()
            .enumerate()
            .filter_map(|(i, c)| u32::try_from(i).ok().map(|i| Node::new(unit_vector(c.lat, c.lon), i)))
            .collect();
        Self {
            tree: RTree::bulk_load(nodes),
            cities,
            admin1,
            admin2,
            countries,
            dataset_ver,
            max_distance_km: DEFAULT_MAX_DISTANCE_KM,
        }
    }

    #[must_use]
    pub fn with_max_distance_km(mut self, km: f64) -> Self {
        self.max_distance_km = km;
        self
    }

    /// `locations.dataset_ver` için: dosya adı + içerik özeti.
    #[must_use]
    pub fn dataset_ver(&self) -> &str {
        &self.dataset_ver
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.cities.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cities.is_empty()
    }

    /// En yakın yerleşim; arama yarıçapında yoksa `Ok(None)`.
    pub fn reverse(&self, lat: f64, lon: f64) -> Result<Option<Place>, MediaError> {
        if !valid_coordinate(lat, lon) {
            return Err(MediaError::InvalidCoordinate { lat, lon });
        }
        let Some(node) = self.tree.nearest_neighbor(&unit_vector(lat, lon)) else {
            return Ok(None);
        };
        let Some(city) = usize::try_from(node.data).ok().and_then(|i| self.cities.get(i)) else {
            return Ok(None);
        };
        let distance_km = haversine_km(lat, lon, city.lat, city.lon);
        if distance_km > self.max_distance_km {
            return Ok(None);
        }
        let a1_key = format!("{}.{}", city.country_code, city.admin1_code);
        let a2_key = format!("{a1_key}.{}", city.admin2_code);
        Ok(Some(Place {
            geoname_id: city.geoname_id,
            name: city.name.clone(),
            country_code: city.country_code.clone(),
            country: self.countries.get(&city.country_code).cloned().unwrap_or_else(|| city.country_code.clone()),
            admin1: self.admin1.get(&a1_key).cloned(),
            admin2: (!city.admin2_code.is_empty()).then(|| self.admin2.get(&a2_key).cloned()).flatten(),
            lat: city.lat,
            lon: city.lon,
            population: city.population,
            distance_km,
        }))
    }
}

fn unit_vector(lat: f64, lon: f64) -> [f64; 3] {
    let (la, lo) = (lat.to_radians(), lon.to_radians());
    [la.cos() * lo.cos(), la.cos() * lo.sin(), la.sin()]
}

#[must_use]
pub fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = p2 - p1;
    let dl = (lon2 - lon1).to_radians();
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_KM * a.sqrt().min(1.0).asin()
}

fn geo_err(path: &Path, line: usize, reason: impl Into<String>) -> MediaError {
    MediaError::GeoData { path: PathBuf::from(path), line, reason: reason.into() }
}

/// GeoNames ana tablo satırları: 0 id, 1 ad, 4 enlem, 5 boylam, 6 özellik sınıfı,
/// 8 ülke, 10 admin1, 11 admin2, 14 nüfus. Yalnızca `P` (yerleşim) sınıfı alınır.
fn parse_cities(text: &str, path: &Path) -> Result<Vec<City>, MediaError> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let lineno = i + 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 15 {
            return Err(geo_err(path, lineno, format!("{} sütun, en az 15 bekleniyor", cols.len())));
        }
        if cols[6] != "P" {
            continue;
        }
        let num = |idx: usize, what: &str| -> Result<f64, MediaError> {
            cols[idx].parse::<f64>().map_err(|_| geo_err(path, lineno, format!("geçersiz {what}: {:?}", cols[idx])))
        };
        let (lat, lon) = (num(4, "enlem")?, num(5, "boylam")?);
        if !valid_coordinate(lat, lon) {
            return Err(geo_err(path, lineno, format!("aralık dışı koordinat {lat},{lon}")));
        }
        out.push(City {
            geoname_id: cols[0].parse().map_err(|_| geo_err(path, lineno, "geçersiz geonameid"))?,
            name: cols[1].to_owned(),
            country_code: cols[8].to_owned(),
            admin1_code: cols[10].to_owned(),
            admin2_code: cols[11].to_owned(),
            lat,
            lon,
            population: cols[14].parse().unwrap_or(0),
        });
    }
    Ok(out)
}

/// `admin1CodesASCII.txt` / `admin2Codes.txt`: `TR.34<TAB>Istanbul<TAB>…`.
fn parse_codes(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split('\t');
            Some((it.next()?.to_owned(), it.next()?.to_owned()))
        })
        .collect()
}

/// `countryInfo.txt`: `#` yorum satırları; sütun 0 ISO kodu, 4 ülke adı.
fn parse_countries(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let cols: Vec<&str> = l.split('\t').collect();
            Some((cols.first()?.to_string(), cols.get(4)?.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dms_format_and_carry() {
        assert_eq!(format_dms(41.01384, 28.94966).unwrap(), "41° 00' 49.8\" N, 028° 56' 58.8\" E");
        assert_eq!(format_dms(-33.8688, -151.2093).unwrap(), "33° 52' 07.7\" S, 151° 12' 33.5\" W");
        // 59.99" → bir sonraki dakikaya taşar.
        assert_eq!(
            format_dms(10.0 + 59.0 / 60.0 + 59.99 / 3600.0, 0.0).unwrap(),
            "11° 00' 00.0\" N, 000° 00' 00.0\" E"
        );
        assert!(format_dms(91.0, 0.0).is_err());
        assert!(format_dms(f64::NAN, 0.0).is_err());
    }

    #[test]
    fn haversine_known_distance() {
        // İstanbul – Ankara kuş uçuşu ≈ 350 km.
        let d = haversine_km(41.01384, 28.94966, 39.91987, 32.85427);
        assert!((345.0..356.0).contains(&d), "{d}");
    }
}

//! Ayarlar → Yapay Zekâ → Yer verisi: çevrimdışı ters coğrafi kodlama için
//! GeoNames dökümlerinin indirilmesi ve GPS'li fotoğrafların yerlere atanması.
//!
//! GeoNames dökümleri her gün yeniden üretilir; sabit bir özetle
//! sabitlenemez. Bu yüzden doğrulama içeriğe göredir: dosyalar `.partial/`
//! altında ayrıştırılır, şehir sayısı alt sınırı geçmezse hiçbiri yerine
//! konmaz. Ancak tam ve okunabilir bir set `geonames/` dizinine taşınır.
//!
//! İndirme bitince yeni dizin çalışan medya hattına verilir ([`GeoSlot`]) ve
//! GPS'i olup yeri olmayan her fotoğraf aynı anda yerine atanır. Aynı atama her
//! açılışta da çalışır: veri yokken içe aktarılan fotoğraflar böylece kalmaz.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use loupe_ai::models::{Archive, extract_unverified};
use loupe_core::MediaId;
use loupe_db::Pool;
use loupe_db::repo::location;
use loupe_jobs::GeoSlot;
use loupe_jobs::handlers::new_location;
use loupe_media::GeoIndex;
use serde::Serialize;

use crate::{AppError, AppResult};

pub const BASE_URL: &str = "https://download.geonames.org/export/dump";
/// Nüfusu 500'ün üstündeki yerleşimler (Göreme, Uçhisar gibi kasabalar dahil).
const CITIES_ZIP: &str = "cities500.zip";
const CITIES_TXT: &str = "cities500.txt";
/// Adlar için; biri eksikse ilgili düzey kod olarak kalır.
const NAME_FILES: [&str; 3] = ["admin1CodesASCII.txt", "admin2Codes.txt", "countryInfo.txt"];
/// Gerçek döküm ~200 000 yerleşim içerir; bunun çok altı yarım ya da bozuk indirmedir.
const MIN_CITIES: usize = 100_000;
const PARTIAL_DIR: &str = ".partial";
const ATTEMPTS: u32 = 3;
/// Atama satırları: içe aktarma ile aynı büyüklükte yazma işlemleri.
const ASSIGN_BATCH: usize = 500;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeoJobDto {
    pub running: bool,
    /// `download` | `verify` | `assign` | `done`
    pub phase: Option<&'static str>,
    pub done: u64,
    pub total: u64,
    /// Bu indirmeden sonra yere atanan fotoğraf.
    pub assigned: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeoDataDto {
    pub dir: String,
    /// Yer dizini yüklü: yeni fotoğraflar içe aktarılırken yere atanır.
    pub ready: bool,
    pub places: usize,
    pub dataset: Option<String>,
    /// GPS'i olan fotoğraflar ve bunlardan bir yere bağlanmış olanlar.
    pub with_gps: u64,
    pub located: u64,
    pub job: GeoJobDto,
}

#[derive(Debug)]
pub struct GeoData {
    dir: PathBuf,
    base_url: String,
    min_cities: usize,
    pool: Arc<Pool>,
    slot: GeoSlot,
    job: Arc<Mutex<GeoJobDto>>,
}

impl GeoData {
    #[must_use]
    pub fn new(dir: PathBuf, pool: Arc<Pool>, slot: GeoSlot) -> Self {
        Self {
            dir,
            base_url: BASE_URL.to_owned(),
            min_cities: MIN_CITIES,
            pool,
            slot,
            job: Arc::new(Mutex::new(GeoJobDto::default())),
        }
    }

    /// Testler: yerel sunucu ve küçük veri seti.
    #[must_use]
    pub fn with_source(mut self, base_url: &str, min_cities: usize) -> Self {
        self.base_url = base_url.trim_end_matches('/').to_owned();
        self.min_cities = min_cities;
        self
    }

    /// Diskteki veriyi yükler (varsa) ve hatta verir. Yoksa `Ok(None)`.
    pub fn load(&self) -> AppResult<Option<Arc<GeoIndex>>> {
        if !self.dir.join(CITIES_TXT).is_file() {
            return Ok(None);
        }
        let geo = Arc::new(GeoIndex::load_dir(&self.dir)?);
        *self.slot.write().map_err(|_| AppError::Poisoned)? = Some(geo.clone());
        Ok(Some(geo))
    }

    pub fn status(&self) -> AppResult<GeoDataDto> {
        let geo = self.slot.read().map_err(|_| AppError::Poisoned)?.clone();
        let (with_gps, located) = self.pool.read(|c| {
            Ok(c.query_row(
                "SELECT count(*), count(location_id) FROM media_files
                  WHERE gps_lat IS NOT NULL AND soft_deleted_at IS NULL",
                [],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
            )?)
        })?;
        Ok(GeoDataDto {
            dir: self.dir.display().to_string(),
            ready: geo.is_some(),
            places: geo.as_ref().map_or(0, |g| g.len()),
            dataset: geo.as_ref().map(|g| g.dataset_ver().to_owned()),
            with_gps: u64::try_from(with_gps).unwrap_or(0),
            located: u64::try_from(located).unwrap_or(0),
            job: self.job.lock().map_err(|_| AppError::Poisoned)?.clone(),
        })
    }

    /// Arka planda indirir, doğrular, yerine koyar ve eski fotoğrafları atar.
    pub fn install(&self) -> AppResult<()> {
        {
            let mut j = self.job.lock().map_err(|_| AppError::Poisoned)?;
            if j.running {
                return Ok(());
            }
            *j = GeoJobDto { running: true, phase: Some("download"), ..GeoJobDto::default() };
        }
        let client = loupe_ai::fetch::http_client()?;
        let (dir, base, min, pool, slot, job) = (
            self.dir.clone(),
            self.base_url.clone(),
            self.min_cities,
            self.pool.clone(),
            self.slot.clone(),
            self.job.clone(),
        );
        tokio::spawn(async move {
            let result = run_install(&client, &base, &dir, min, &pool, &slot, &job).await;
            if let Ok(mut j) = job.lock() {
                j.running = false;
                match result {
                    Ok(()) => j.phase = Some("done"),
                    Err(e) => j.error = Some(e.to_string()),
                }
            }
        });
        Ok(())
    }
}

fn set_job(job: &Mutex<GeoJobDto>, f: impl FnOnce(&mut GeoJobDto)) {
    if let Ok(mut j) = job.lock() {
        f(&mut j);
    }
}

async fn run_install(
    client: &reqwest::Client,
    base: &str,
    dir: &Path,
    min_cities: usize,
    pool: &Arc<Pool>,
    slot: &GeoSlot,
    job: &Arc<Mutex<GeoJobDto>>,
) -> AppResult<()> {
    let partial = dir.join(PARTIAL_DIR);
    tokio::fs::create_dir_all(&partial).await.map_err(AppError::io(partial.display().to_string()))?;
    let names: Vec<&str> = std::iter::once(CITIES_ZIP).chain(NAME_FILES).collect();
    set_job(job, |j| j.total = names.len() as u64);
    for (i, name) in names.iter().enumerate() {
        let bytes = fetch(client, &format!("{base}/{name}")).await?;
        let out = partial.join(name);
        tokio::fs::write(&out, &bytes).await.map_err(AppError::io(out.display().to_string()))?;
        set_job(job, |j| j.done = i as u64 + 1);
    }

    set_job(job, |j| j.phase = Some("verify"));
    let (d, p) = (dir.to_path_buf(), partial.clone());
    let geo = tokio::task::spawn_blocking(move || verify_and_commit(&d, &p, min_cities))
        .await
        .map_err(|e| AppError::Join(e.to_string()))??;
    *slot.write().map_err(|_| AppError::Poisoned)? = Some(geo.clone());

    set_job(job, |j| j.phase = Some("assign"));
    let (pool, job2) = (pool.clone(), job.clone());
    let assigned =
        tokio::task::spawn_blocking(move || assign_missing(&pool, &geo, |n| set_job(&job2, |j| j.assigned = n)))
            .await
            .map_err(|e| AppError::Join(e.to_string()))??;
    set_job(job, |j| j.assigned = assigned);
    Ok(())
}

/// Biraz bekleyip yeniden dener; GeoNames sunucusu ara sıra bağlantı keser.
async fn fetch(client: &reqwest::Client, url: &str) -> AppResult<Vec<u8>> {
    let mut last = String::new();
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(2u64 << attempt)).await;
        }
        match client.get(url).send().await {
            Ok(r) if r.status().is_success() => match r.bytes().await {
                Ok(b) => return Ok(b.to_vec()),
                Err(e) => last = e.to_string(),
            },
            Ok(r) => last = format!("HTTP {}", r.status()),
            Err(e) => last = e.to_string(),
        }
    }
    Err(AppError::Invalid(format!("{url}: {last}")))
}

/// Arşivi açar, seti ayrıştırır; ancak tamamı okunursa dosyaları yerine koyar.
fn verify_and_commit(dir: &Path, partial: &Path, min_cities: usize) -> AppResult<Arc<GeoIndex>> {
    let txt = partial.join(CITIES_TXT);
    extract_unverified(&partial.join(CITIES_ZIP), Archive::Zip, CITIES_TXT, &txt)?;
    let opt = |n: &str| Some(partial.join(n)).filter(|p| p.is_file());
    let geo = GeoIndex::from_files(
        &txt,
        opt(NAME_FILES[0]).as_deref(),
        opt(NAME_FILES[1]).as_deref(),
        opt(NAME_FILES[2]).as_deref(),
    )?;
    if geo.len() < min_cities {
        return Err(AppError::Invalid(format!(
            "{CITIES_TXT}: {} yerleşim, en az {min_cities} bekleniyordu",
            geo.len()
        )));
    }
    for name in std::iter::once(CITIES_TXT).chain(NAME_FILES) {
        let (from, to) = (partial.join(name), dir.join(name));
        std::fs::rename(&from, &to).map_err(AppError::io(to.display().to_string()))?;
    }
    let _ignored = std::fs::remove_dir_all(partial);
    // Yerine konan dosyalardan yeniden yüklenir: `dataset_ver` aynı içerikten çıkar.
    Ok(Arc::new(GeoIndex::load_dir(dir)?))
}

/// GPS'i olup yeri olmayan fotoğrafları atar; atananların sayısını döndürür.
/// Açık deniz gibi arama yarıçapı dışındaki noktalar yersiz kalır.
pub fn assign_missing(pool: &Pool, geo: &GeoIndex, progress: impl Fn(u64)) -> AppResult<u64> {
    let rows: Vec<(i64, f64, f64)> = pool.read(|c| {
        Ok(c.prepare(
            "SELECT id, gps_lat, gps_lon FROM media_files
              WHERE gps_lat IS NOT NULL AND location_id IS NULL AND soft_deleted_at IS NULL",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?)
    })?;
    let mut assigned = 0u64;
    for chunk in rows.chunks(ASSIGN_BATCH) {
        let mut found = Vec::with_capacity(chunk.len());
        for &(id, lat, lon) in chunk {
            if let Some(p) = geo.reverse(lat, lon)? {
                found.push((MediaId(id), new_location(&p, geo.dataset_ver())));
            }
        }
        pool.write(|c| {
            let tx = c.transaction()?;
            for (id, loc) in &found {
                let lid = location::upsert(&tx, loc)?;
                location::assign_to_media(&tx, *id, lid)?;
            }
            tx.commit()?;
            Ok(())
        })?;
        assigned += found.len() as u64;
        progress(assigned);
    }
    Ok(assigned)
}

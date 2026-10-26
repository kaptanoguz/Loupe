//! SQLite `jobs` tablosu üzerinde kalıcı iş kuyruğu.
//!
//! - **Kalıcılık:** her durum geçişi tek bir SQL ifadesidir; süreç çökse de
//!   kuyruk tutarlı kalır. Açılışta süresi dolmuş kiralar geri alınır.
//! - **Alma:** `UPDATE … WHERE id = (SELECT … ORDER BY base_priority, run_after, id
//!   LIMIT 1) RETURNING` tek ifadede atomiktir; `jobs_claim` kısmi indeksini
//!   kullanır, 50 000 bekleyen işte de tek indeks araması kadar sürer.
//! - **Viewport önceliği:** [`JobQueue::bump_viewport_priority`] görünen medyanın
//!   işlerini `Immediate` yapar; viewport değişince eski yükseltmeler geri alınır.
//! - **Yeniden deneme:** geçici hata → `failed` + üstel geri çekilme;
//!   `max_attempts` dolunca veya kalıcı hatada → `dead` (dead-letter).
//!   Önkoşul bekleyen iş ([`JobError::waiting`]) deneme harcamadan `failed`
//!   olarak [`RetryPolicy::wait`] sonra yeniden alınır.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use loupe_core::{MediaId, now_ms};
use loupe_db::{DbError, Pool};
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params, params_from_iter};

use crate::JobsError;
use crate::model::{Job, JobError, JobId, JobKind, JobRecord, JobState, NewJob, Priority, QueueStats};

/// Toplu eklemede transaction başına satır (loupe-db toplu import ile aynı).
pub const ENQUEUE_BATCH: usize = 500;

/// Üstel geri çekilme: `base · 2^(deneme−1)`, en çok `max`. Önkoşul bekleyen
/// iş ([`JobError::waiting`]) bunun yerine sabit `wait` aralığıyla denenir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    pub base: Duration,
    pub max: Duration,
    pub wait: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self { base: Duration::from_secs(5), max: Duration::from_secs(3600), wait: Duration::from_secs(30) }
    }
}

impl RetryPolicy {
    #[must_use]
    pub fn delay(&self, attempts: u32) -> Duration {
        let exp = attempts.saturating_sub(1).min(30);
        self.base.saturating_mul(1u32 << exp).min(self.max)
    }
}

/// Bekleyen işçileri uyandırmak için: her yeni iş, yükseltme veya devam
/// komutunda sayaç artar ve tüm bekleyenler uyanır.
#[derive(Debug, Default)]
pub(crate) struct Signal {
    epoch: Mutex<u64>,
    cv: Condvar,
}

impl Signal {
    pub(crate) fn notify(&self) {
        if let Ok(mut e) = self.epoch.lock() {
            *e = e.wrapping_add(1);
        }
        self.cv.notify_all();
    }

    pub(crate) fn current(&self) -> u64 {
        self.epoch.lock().map(|e| *e).unwrap_or(0)
    }

    /// `seen`'den sonra sinyal gelene ya da süre dolana kadar bekler.
    pub(crate) fn wait(&self, seen: u64, timeout: Duration) {
        if let Ok(guard) = self.epoch.lock() {
            let _ignored = self.cv.wait_timeout_while(guard, timeout, |e| *e == seen);
        }
    }
}

#[derive(Debug)]
pub struct JobQueue {
    pool: Arc<Pool>,
    retry: RetryPolicy,
    /// Viewport yükseltmesiyle önceliği değişen işlerin eski öncelikleri.
    boosted: Mutex<HashMap<i64, u8>>,
    pub(crate) signal: Signal,
}

const JOB_COLS: &str = "id, kind, media_id, variant, base_priority, attempts, max_attempts";

fn job_from_row(r: &Row<'_>) -> rusqlite::Result<Job> {
    let kind: String = r.get(1)?;
    Ok(Job {
        id: JobId(r.get(0)?),
        kind: kind
            .parse()
            .map_err(|e: String| rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, e.into()))?,
        media_id: r.get::<_, Option<i64>>(2)?.map(MediaId),
        variant: r.get(3)?,
        priority: r.get(4)?,
        attempts: r.get(5)?,
        max_attempts: r.get(6)?,
    })
}

impl JobQueue {
    #[must_use]
    pub fn new(pool: Arc<Pool>) -> Self {
        Self::with_retry(pool, RetryPolicy::default())
    }

    #[must_use]
    pub fn with_retry(pool: Arc<Pool>, retry: RetryPolicy) -> Self {
        Self { pool, retry, boosted: Mutex::new(HashMap::new()), signal: Signal::default() }
    }

    #[must_use]
    pub fn pool(&self) -> &Arc<Pool> {
        &self.pool
    }

    /// Tek iş ekler (ya da var olanın önceliğini yükseltir).
    pub fn enqueue(&self, job: &NewJob) -> Result<JobId, JobsError> {
        let id = self.pool.write(|conn| upsert(conn, job, now_ms()))?;
        self.signal.notify();
        Ok(id)
    }

    /// Toplu ekleme: 500'lük `BEGIN IMMEDIATE` transaction'ları.
    pub fn enqueue_batch(&self, jobs: &[NewJob]) -> Result<usize, JobsError> {
        let mut n = 0;
        for chunk in jobs.chunks(ENQUEUE_BATCH) {
            n += self.pool.write(|conn| {
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let now = now_ms();
                for j in chunk {
                    upsert(&tx, j, now)?;
                }
                tx.commit()?;
                Ok(chunk.len())
            })?;
        }
        self.signal.notify();
        Ok(n)
    }

    /// Görünen medyanın bekleyen işlerini `Immediate` yapar. Önceki çağrıda
    /// yükseltilip artık görünmeyen işler eski önceliklerine döner. Yükseltilen
    /// iş sayısını döndürür.
    ///
    /// Yükseltme veritabanına yazılır (tek ifade), böylece işçiler ayrı bir
    /// bellek kanalına bakmadan aynı `ORDER BY` ile acil işi önce alır.
    pub fn bump_viewport_priority(&self, media_ids: &[MediaId]) -> Result<usize, JobsError> {
        let mut boosted = self.boosted.lock().map_err(|_| JobsError::Poisoned)?;
        let wanted: std::collections::HashSet<i64> = media_ids.iter().map(|m| m.0).collect();
        let level = Priority::Immediate.level();
        let (restored, newly) = self.pool.write(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            // 1. Görünmez olan eski yükseltmeleri geri al (iş hâlâ bekliyorsa).
            let mut restored = Vec::new();
            {
                let mut owner = tx.prepare_cached("SELECT media_id FROM jobs WHERE id = ?1")?;
                let mut demote = tx.prepare_cached(
                    "UPDATE jobs SET base_priority = ?2, updated_at = ?3
                      WHERE id = ?1 AND state IN ('pending','failed') AND base_priority = ?4",
                )?;
                for (&job, &orig) in boosted.iter() {
                    let media: Option<Option<i64>> = owner.query_row([job], |r| r.get(0)).optional()?;
                    if media.flatten().is_none_or(|m| !wanted.contains(&m)) {
                        demote.execute(params![job, orig, now_ms(), level])?;
                        restored.push(job);
                    }
                }
            }
            // 2. Görünenleri yükselt; eski önceliklerini hatırla.
            let mut newly = Vec::new();
            for chunk in media_ids.chunks(ENQUEUE_BATCH) {
                let marks = vec!["?"; chunk.len()].join(",");
                let filter =
                    format!("media_id IN ({marks}) AND state IN ('pending','failed') AND base_priority > {level}");
                let ids = || chunk.iter().map(|m| m.0);
                // Eski öncelikler önce okunur (aynı transaction içinde tutarlı).
                let rows: Vec<(i64, u8)> = tx
                    .prepare(&format!("SELECT id, base_priority FROM jobs WHERE {filter}"))?
                    .query_map(params_from_iter(ids()), |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<Result<_, _>>()?;
                tx.execute(
                    &format!("UPDATE jobs SET base_priority = {level}, updated_at = {} WHERE {filter}", now_ms()),
                    params_from_iter(ids()),
                )?;
                newly.extend(rows);
            }
            tx.commit()?;
            Ok((restored, newly))
        })?;
        for job in restored {
            boosted.remove(&job);
        }
        let count = newly.len();
        for (job, orig) in newly {
            boosted.entry(job).or_insert(orig);
        }
        drop(boosted);
        self.signal.notify();
        Ok(count)
    }

    /// En acil hazır işi kiralar. `max_priority` verilirse yalnızca o seviyeye
    /// kadar (dahil) işler alınır (kısma: kullanıcı etkinken arka plan işleri yok).
    pub fn claim(&self, owner: &str, max_priority: u8, lease: Duration) -> Result<Option<Job>, JobsError> {
        let lease_ms = i64::try_from(lease.as_millis()).unwrap_or(i64::MAX / 2);
        let job = self.pool.write(|conn| {
            let now = now_ms();
            Ok(conn
                .prepare_cached(&format!(
                    "UPDATE jobs SET state = 'running', attempts = attempts + 1,
                            lease_owner = ?1, lease_until = ?2, updated_at = ?3
                      WHERE id = (SELECT id FROM jobs
                                   WHERE state IN ('pending','failed') AND run_after <= ?3 AND base_priority <= ?4
                                   ORDER BY base_priority, run_after, id LIMIT 1)
                      RETURNING {JOB_COLS}"
                ))?
                .query_row(params![owner, now.saturating_add(lease_ms), now, max_priority], job_from_row)
                .optional()?)
        })?;
        if let Some(j) = &job
            && let Ok(mut b) = self.boosted.lock()
        {
            b.remove(&j.id.0);
        }
        Ok(job)
    }

    /// Uzun süren iş kirasını uzatır. Kira başka işçiye geçmişse `false`.
    pub fn renew(&self, id: JobId, owner: &str, lease: Duration) -> Result<bool, JobsError> {
        let lease_ms = i64::try_from(lease.as_millis()).unwrap_or(i64::MAX / 2);
        let n = self.pool.write(|conn| {
            Ok(conn.execute(
                "UPDATE jobs SET lease_until = ?3, updated_at = ?4
                  WHERE id = ?1 AND state = 'running' AND lease_owner = ?2",
                params![id.0, owner, now_ms().saturating_add(lease_ms), now_ms()],
            )?)
        })?;
        Ok(n == 1)
    }

    /// Başarılı iş: satır silinir. İş sürerken yeniden istendiyse (`rerun`)
    /// satır sıfır denemeyle yeniden bekler.
    pub fn complete(&self, id: JobId, owner: &str) -> Result<(), JobsError> {
        let again = self.pool.write(|conn| {
            let again = conn.execute(
                "UPDATE jobs SET state = 'pending', rerun = 0, attempts = 0, lease_owner = NULL, lease_until = NULL,
                        updated_at = ?3
                  WHERE id = ?1 AND lease_owner = ?2 AND rerun = 1",
                params![id.0, owner, now_ms()],
            )?;
            if again == 0 {
                conn.execute("DELETE FROM jobs WHERE id = ?1 AND lease_owner = ?2", params![id.0, owner])?;
            }
            Ok(again > 0)
        })?;
        if again {
            self.signal.notify();
        }
        Ok(())
    }

    /// Başarısız iş: geçici hatada geri çekilerek `failed`, kalıcı hatada veya
    /// deneme hakkı bitince `dead`. Yeni durumu döndürür.
    pub fn fail(&self, id: JobId, owner: &str, err: &JobError) -> Result<JobState, JobsError> {
        let retry = self.retry;
        let state = self.pool.write(|conn| {
            let row: Option<(u32, u32)> = conn
                .prepare_cached("SELECT attempts, max_attempts FROM jobs WHERE id = ?1 AND lease_owner = ?2")?
                .query_row(params![id.0, owner], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            let Some((attempts, max_attempts)) = row else {
                return Ok(None);
            };
            let now = now_ms();
            // Bekleyen iş: claim'in artırdığı deneme geri alınır.
            let attempts = if err.waiting { attempts.saturating_sub(1) } else { attempts };
            let dead = !err.retryable || (!err.waiting && attempts >= max_attempts);
            let delay = if err.waiting { retry.wait } else { retry.delay(attempts) };
            let run_after = now.saturating_add(i64::try_from(delay.as_millis()).unwrap_or(i64::MAX / 2));
            let state = if dead { JobState::Dead } else { JobState::Failed };
            conn.execute(
                "UPDATE jobs SET state = ?2, run_after = ?3, attempts = ?7, lease_owner = NULL, lease_until = NULL,
                        last_error_code = ?4, last_error_msg = ?5, updated_at = ?6
                  WHERE id = ?1",
                params![
                    id.0,
                    state.as_str(),
                    if dead { now } else { run_after },
                    err.code,
                    truncate(&err.message),
                    now,
                    attempts
                ],
            )?;
            Ok(Some(state))
        })?;
        state.ok_or(JobsError::LeaseLost(id))
    }

    /// Kirası dolmuş (çökmüş ya da donmuş işçi) işleri yeniden kuyruğa koyar.
    /// Deneme sayacı korunur: çökme her seferinde tekrarlıyorsa iş sonunda dead olur.
    pub fn reclaim_expired(&self) -> Result<usize, JobsError> {
        let n = self.pool.write(|conn| {
            let now = now_ms();
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let dead = tx.execute(
                "UPDATE jobs SET state = 'dead', lease_owner = NULL, lease_until = NULL,
                        last_error_code = 'LEASE_EXPIRED', last_error_msg = 'işçi yanıt vermedi', updated_at = ?1
                  WHERE state = 'running' AND lease_until < ?1 AND attempts >= max_attempts",
                [now],
            )?;
            let again = tx.execute(
                "UPDATE jobs SET state = 'pending', lease_owner = NULL, lease_until = NULL,
                        last_error_code = 'LEASE_EXPIRED', updated_at = ?1
                  WHERE state = 'running' AND lease_until < ?1",
                [now],
            )?;
            tx.commit()?;
            Ok(dead + again)
        })?;
        if n > 0 {
            self.signal.notify();
        }
        Ok(n)
    }

    /// Dead-letter'daki işi sıfır denemeyle yeniden kuyruğa koyar.
    pub fn retry_dead(&self, id: JobId) -> Result<bool, JobsError> {
        let n = self.pool.write(|conn| {
            Ok(conn.execute(
                "UPDATE jobs SET state = 'pending', attempts = 0, run_after = ?2, updated_at = ?2
                  WHERE id = ?1 AND state = 'dead'",
                params![id.0, now_ms()],
            )?)
        })?;
        self.signal.notify();
        Ok(n == 1)
    }

    /// Belirli dosya türündeki medyanın dead-letter işlerini yeniden kuyruğa
    /// koyar (ör. ffmpeg kurulunca daha önce başarısız olan video işleri).
    pub fn retry_dead_for_file_type(&self, kinds: &[JobKind], file_type: &str) -> Result<usize, JobsError> {
        if kinds.is_empty() {
            return Ok(0);
        }
        let marks = (3..3 + kinds.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
        let sql = format!(
            "UPDATE jobs SET state = 'pending', attempts = 0, run_after = ?2, updated_at = ?2
              WHERE state = 'dead' AND kind IN ({marks})
                AND media_id IN (SELECT id FROM media_files WHERE file_type = ?1)"
        );
        let mut values: Vec<rusqlite::types::Value> = vec![file_type.to_owned().into(), now_ms().into()];
        values.extend(kinds.iter().map(|k| rusqlite::types::Value::from(k.as_str().to_owned())));
        let n = self.pool.write(|conn| Ok(conn.execute(&sql, params_from_iter(values.iter()))?))?;
        if n > 0 {
            self.signal.notify();
        }
        Ok(n)
    }

    /// Belirli hata koduyla dead-letter'a düşmüş işleri sıfır denemeyle yeniden
    /// kuyruğa koyar (ör. eski sürümlerde küçük resmi beklerken tükenen AI işleri).
    pub fn retry_dead_with_code(&self, kinds: &[JobKind], code: &str) -> Result<usize, JobsError> {
        if kinds.is_empty() {
            return Ok(0);
        }
        let marks = (3..3 + kinds.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
        let sql = format!(
            "UPDATE jobs SET state = 'pending', attempts = 0, run_after = ?2, updated_at = ?2
              WHERE state = 'dead' AND kind IN ({marks}) AND last_error_code = ?1"
        );
        let mut values: Vec<rusqlite::types::Value> = vec![code.to_owned().into(), now_ms().into()];
        values.extend(kinds.iter().map(|k| rusqlite::types::Value::from(k.as_str().to_owned())));
        let n = self.pool.write(|conn| Ok(conn.execute(&sql, params_from_iter(values.iter()))?))?;
        if n > 0 {
            self.signal.notify();
        }
        Ok(n)
    }

    pub fn get(&self, id: JobId) -> Result<Option<JobRecord>, JobsError> {
        Ok(self.pool.read(|conn| {
            Ok(conn
                .prepare_cached(
                    "SELECT id, kind, media_id, variant, base_priority, state, attempts, max_attempts, run_after,
                            last_error_code, last_error_msg FROM jobs WHERE id = ?1",
                )?
                .query_row([id.0], record_from_row)
                .optional()?)
        })?)
    }

    pub fn find(&self, kind: JobKind, media: Option<MediaId>, variant: &str) -> Result<Option<JobRecord>, JobsError> {
        Ok(self.pool.read(|conn| {
            Ok(conn
                .prepare_cached(
                    "SELECT id, kind, media_id, variant, base_priority, state, attempts, max_attempts, run_after,
                            last_error_code, last_error_msg
                       FROM jobs WHERE kind = ?1 AND media_id IS ?2 AND variant = ?3",
                )?
                .query_row(params![kind.as_str(), media.map(|m| m.0), variant], record_from_row)
                .optional()?)
        })?)
    }

    pub fn dead_letters(&self, limit: u32) -> Result<Vec<JobRecord>, JobsError> {
        Ok(self.pool.read(|conn| {
            Ok(conn
                .prepare_cached(
                    "SELECT id, kind, media_id, variant, base_priority, state, attempts, max_attempts, run_after,
                            last_error_code, last_error_msg
                       FROM jobs WHERE state = 'dead' ORDER BY updated_at DESC LIMIT ?1",
                )?
                .query_map([limit], record_from_row)?
                .collect::<Result<_, _>>()?)
        })?)
    }

    pub fn stats(&self) -> Result<QueueStats, JobsError> {
        Ok(self.pool.read(|conn| {
            let mut s = QueueStats::default();
            let mut stmt = conn.prepare_cached("SELECT state, count(*) FROM jobs GROUP BY state")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            for row in rows {
                let (state, n) = row?;
                let n = u64::try_from(n).unwrap_or(0);
                match state.as_str() {
                    "pending" => s.pending = n,
                    "running" => s.running = n,
                    "failed" => s.failed = n,
                    _ => s.dead = n,
                }
            }
            Ok(s)
        })?)
    }
}

fn record_from_row(r: &Row<'_>) -> rusqlite::Result<JobRecord> {
    let conv =
        |i: usize, e: String| rusqlite::Error::FromSqlConversionFailure(i, rusqlite::types::Type::Text, e.into());
    Ok(JobRecord {
        id: JobId(r.get(0)?),
        kind: r.get::<_, String>(1)?.parse().map_err(|e| conv(1, e))?,
        media_id: r.get::<_, Option<i64>>(2)?.map(MediaId),
        variant: r.get(3)?,
        priority: r.get(4)?,
        state: r.get::<_, String>(5)?.parse().map_err(|e| conv(5, e))?,
        attempts: r.get(6)?,
        max_attempts: r.get(7)?,
        run_after: r.get(8)?,
        last_error_code: r.get(9)?,
        last_error_msg: r.get(10)?,
    })
}

fn truncate(msg: &str) -> &str {
    match msg.char_indices().nth(1000) {
        Some((i, _)) => &msg[..i],
        None => msg,
    }
}

/// Ekle ya da var olan işin önceliğini yükselt (dead işlere dokunulmaz:
/// tarama her seferinde bozuk dosyayı yeniden kuyruğa sokmasın).
///
/// Gecikmeli iş bekliyorsa başlangıcı ileri kayar (debounce). Çalışan küresel
/// işte `rerun` işaretlenir: [`JobQueue::complete`] satırı silmek yerine yeniden
/// bekletir, böylece iş sürerken gelen istek kaybolmaz.
fn upsert(conn: &Connection, j: &NewJob, now: i64) -> Result<JobId, DbError> {
    let conflict = if j.media_id.is_some() {
        "ON CONFLICT (kind, media_id, variant)"
    } else {
        "ON CONFLICT (kind, variant) WHERE media_id IS NULL"
    };
    let sql = format!(
        "INSERT INTO jobs (kind, media_id, variant, base_priority, max_attempts, run_after, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, COALESCE(?5, 5), ?6, ?7, ?7)
         {conflict} DO UPDATE SET
           base_priority = min(base_priority, excluded.base_priority),
           run_after = CASE
             WHEN state = 'running' AND media_id IS NULL THEN excluded.run_after
             WHEN ?8 AND state = 'pending' THEN max(run_after, excluded.run_after)
             ELSE run_after END,
           rerun = CASE WHEN state = 'running' AND media_id IS NULL THEN 1 ELSE rerun END,
           updated_at = excluded.updated_at
           WHERE state <> 'dead'
         RETURNING id"
    );
    let delay_ms = j.delay.map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX / 2));
    let run_after = now.saturating_add(delay_ms);
    let media = j.media_id.map(|m| m.0);
    let id: Option<i64> = conn
        .prepare_cached(&sql)?
        .query_row(
            params![
                j.kind.as_str(),
                media,
                j.variant,
                j.priority.level(),
                j.max_attempts,
                run_after,
                now,
                j.delay.is_some()
            ],
            |r| r.get(0),
        )
        .optional()?;
    match id {
        Some(id) => Ok(JobId(id)),
        None => Ok(JobId(
            conn.prepare_cached("SELECT id FROM jobs WHERE kind = ?1 AND media_id IS ?2 AND variant = ?3")?
                .query_row(params![j.kind.as_str(), media, j.variant], |r| r.get(0))?,
        )),
    }
}

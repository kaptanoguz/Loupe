//! İşçi havuzu: işletim sistemi iş parçacıkları, `catch_unwind` ile panik
//! yalıtımı ve sinyal ile uyanma (boşta yoklama yok, yalnızca güvenlik zaman aşımı).
//!
//! İlk `reserved_immediate` işçi yalnızca `Immediate` iş alır: arka planda
//! 50 000 iş beklerken görünen bir küçük resim en çok bir iş süresi bekler.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::JobsError;
use crate::model::{Job, JobError};
use crate::queue::JobQueue;
use crate::throttle::{ResourceMode, Throttle};

/// Bir iş türünü (veya hepsini) çalıştıran kod. Panik işçiyi düşürmez; iş
/// `PANIC` koduyla dead-letter'a gider.
pub trait JobHandler: Send + Sync {
    fn run(&self, job: &Job, ctx: &JobContext<'_>) -> Result<(), JobError>;
}

/// İşleyiciye verilen bağlam: kapanış bildirimi ve kira uzatma.
#[derive(Debug)]
pub struct JobContext<'a> {
    queue: &'a JobQueue,
    owner: &'a str,
    lease: Duration,
    shutdown: &'a AtomicBool,
}

impl JobContext<'_> {
    /// Havuz kapanıyorsa uzun işler aşama aralarında bırakmalı.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    /// Uzun iş kirasını uzatır; kira kaybedildiyse `false` (iş başkasına geçti).
    pub fn renew_lease(&self, job: &Job) -> Result<bool, JobsError> {
        self.queue.renew(job.id, self.owner, self.lease)
    }
}

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Toplam işçi sayısı (ayrılmışlar dahil).
    pub workers: usize,
    /// Yalnızca `Immediate` iş alan işçi sayısı.
    pub reserved_immediate: usize,
    pub lease: Duration,
    /// Sinyal kaçarsa (ör. `run_after` dolan geri çekilmiş iş) en geç bu kadar sonra bakılır.
    pub idle_wait: Duration,
    pub mode: ResourceMode,
    /// İşçi adlarının öneki (`lease_owner`).
    pub name: String,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        let cpus = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
        Self {
            workers: cpus.clamp(2, 8),
            reserved_immediate: 1,
            lease: Duration::from_secs(300),
            idle_wait: Duration::from_millis(500),
            mode: ResourceMode::Balanced,
            name: "louped".to_owned(),
        }
    }
}

#[derive(Debug)]
struct Shared {
    queue: Arc<JobQueue>,
    throttle: Throttle,
    shutdown: AtomicBool,
    cfg: WorkerConfig,
}

/// Çalışan işçi havuzu. Düşürülünce kapanır ve iş parçacıklarını bekler.
#[derive(Debug)]
pub struct WorkerPool {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl WorkerPool {
    /// Önce süresi dolmuş kiraları geri alır, sonra işçileri başlatır.
    pub fn start(queue: Arc<JobQueue>, handler: Arc<dyn JobHandler>, cfg: WorkerConfig) -> Result<Self, JobsError> {
        queue.reclaim_expired()?;
        let workers = cfg.workers.max(1);
        let reserved = cfg.reserved_immediate.min(workers.saturating_sub(1));
        let shared = Arc::new(Shared {
            queue,
            throttle: Throttle::new(workers - reserved, cfg.mode),
            shutdown: AtomicBool::new(false),
            cfg,
        });
        let mut threads = Vec::with_capacity(workers);
        for i in 0..workers {
            let general = i.checked_sub(reserved);
            let owner = format!("{}-{}-{i}", shared.cfg.name, std::process::id());
            let (s, h) = (Arc::clone(&shared), Arc::clone(&handler));
            let t = std::thread::Builder::new()
                .name(format!("loupe-job-{i}"))
                .spawn(move || worker_loop(&s, h.as_ref(), &owner, general))
                .map_err(JobsError::Spawn)?;
            threads.push(t);
        }
        Ok(Self { shared, threads })
    }

    #[must_use]
    pub fn throttle(&self) -> &Throttle {
        &self.shared.throttle
    }

    /// Yalnızca `Immediate` işler çalışmaya devam eder.
    pub fn pause(&self) {
        self.shared.throttle.set_paused(true);
    }

    pub fn resume(&self) {
        self.shared.throttle.set_paused(false);
        self.shared.queue.signal.notify();
    }

    pub fn set_mode(&self, mode: ResourceMode) {
        self.shared.throttle.set_mode(mode);
        self.shared.queue.signal.notify();
    }

    /// Kullanıcı arayüzde etkinken toplu işler (öncelik ≥ Normal) bekler.
    pub fn set_user_active(&self, active: bool) {
        self.shared.throttle.set_user_active(active);
        self.shared.queue.signal.notify();
    }

    /// Yeni iş almayı bırakır, süren işlerin bitmesini bekler.
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        self.shared.queue.signal.notify();
        for t in self.threads.drain(..) {
            let _ignored = t.join();
        }
    }
}

impl Drop for WorkerPool {
    fn drop(&mut self) {
        self.stop();
    }
}

fn panic_message(p: &(dyn Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "bilinmeyen panik".to_owned())
}

fn worker_loop(s: &Shared, handler: &dyn JobHandler, owner: &str, general: Option<usize>) {
    let mut db_errors = 0u32;
    while !s.shutdown.load(Ordering::SeqCst) {
        let seen = s.queue.signal.current();
        let max = s.throttle.max_priority(general);
        let job = match s.queue.claim(owner, max, s.cfg.lease) {
            Ok(j) => {
                db_errors = 0;
                j
            }
            Err(_) => {
                // Veritabanı geçici olarak kullanılamıyor: artan bekleme ile tekrar dene.
                db_errors = db_errors.saturating_add(1);
                std::thread::sleep(s.cfg.idle_wait.saturating_mul(db_errors.min(10)));
                continue;
            }
        };
        let Some(job) = job else {
            s.queue.signal.wait(seen, s.cfg.idle_wait);
            continue;
        };
        let ctx = JobContext { queue: &s.queue, owner, lease: s.cfg.lease, shutdown: &s.shutdown };
        let outcome = catch_unwind(AssertUnwindSafe(|| handler.run(&job, &ctx)))
            .unwrap_or_else(|p| Err(JobError::permanent("PANIC", panic_message(p.as_ref()))));
        // Sonuç yazılamazsa kira dolunca iş yeniden kuyruğa döner.
        let _ignored = match outcome {
            Ok(()) => s.queue.complete(job.id, owner),
            Err(e) => s.queue.fail(job.id, owner, &e).map(|_| ()),
        };
    }
}

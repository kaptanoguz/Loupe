//! "Kütüphaneyi Doğrula (Bit-Rot)": her dosyanın BLAKE3 özeti diskteki
//! içerikle karşılaştırılır. Kütüphane başına bir iş parçacığı; sonuçlar
//! `verify_runs` / `verify_findings` tablolarında kalır, arayüz son
//! çalıştırmanın raporunu oradan okur.
//!
//! Disk taramanın ortasında çıkarılırsa çalıştırma `failed` olur ve hiçbir
//! dosya "kayıp" diye işaretlenmez (`verify_library` her partide işareti okur).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use loupe_core::{LibraryId, VerifyTrigger};
use loupe_db::{DbVerifyCatalog, Pool};
use loupe_storage::verify::{VerifyOptions, verify_library};
use serde::Serialize;

use crate::library::Libraries;
use crate::{AppError, AppResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyRunDto {
    pub id: i64,
    pub library_id: i64,
    pub library: String,
    /// `manual` | `scheduled` | `remount`
    pub trigger: String,
    /// `running` | `paused` | `done` | `cancelled` | `failed`
    pub state: String,
    pub total: u64,
    pub checked: u64,
    pub ok: u64,
    pub missing: u64,
    pub mismatched: u64,
    pub unreadable: u64,
    pub bytes_read: u64,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Default)]
pub struct Verifier {
    running: Mutex<HashMap<i64, Arc<AtomicBool>>>,
}

impl Verifier {
    /// Çevrimiçi her kütüphane (ya da yalnızca `library`) için doğrulama başlatır.
    /// Zaten çalışan kütüphane atlanır. Başlatılan kütüphane kimlikleri döner.
    pub fn start(
        self: &Arc<Self>,
        pool: &Arc<Pool>,
        libraries: &Arc<Libraries>,
        library: Option<i64>,
    ) -> AppResult<Vec<i64>> {
        let roots = libraries.roots()?;
        let mut started = Vec::new();
        let mut running = self.running.lock().map_err(|_| AppError::Poisoned)?;
        for (lib, root) in roots {
            if library.is_some_and(|l| l != lib.0) || running.contains_key(&lib.0) {
                continue;
            }
            let cancel = Arc::new(AtomicBool::new(false));
            running.insert(lib.0, cancel.clone());
            let (pool, me) = (pool.clone(), Arc::clone(self));
            let spawned = std::thread::Builder::new().name(format!("loupe-verify-{}", lib.0)).spawn(move || {
                let _outcome = run(&pool, lib, &root, &cancel);
                if let Ok(mut r) = me.running.lock() {
                    r.remove(&lib.0);
                }
            });
            if let Err(e) = spawned {
                running.remove(&lib.0);
                return Err(AppError::io("doğrulama iş parçacığı")(e));
            }
            started.push(lib.0);
        }
        if let Some(l) = library
            && started.is_empty()
            && !running.contains_key(&l)
        {
            return Err(AppError::LibraryOffline(format!("kütüphane {l} çevrimdışı")));
        }
        Ok(started)
    }

    pub fn cancel(&self, library: Option<i64>) -> AppResult<usize> {
        let running = self.running.lock().map_err(|_| AppError::Poisoned)?;
        let mut n = 0;
        for (lib, c) in running.iter() {
            if library.is_none_or(|l| l == *lib) {
                c.store(true, Ordering::SeqCst);
                n += 1;
            }
        }
        Ok(n)
    }

    pub fn active(&self) -> usize {
        self.running.lock().map_or(0, |r| r.len())
    }
}

fn run(pool: &Pool, lib: LibraryId, root: &std::path::Path, cancel: &AtomicBool) -> AppResult<()> {
    // Bellekte çalışmayan ama katalogda `running` kalan çalıştırma: önceki oturum çöktü.
    pool.write(|c| {
        c.execute(
            "UPDATE verify_runs SET state = 'failed', finished_at = ?2 WHERE library_id = ?1 AND state IN ('running','paused')",
            rusqlite::params![lib.0, loupe_core::now_ms()],
        )?;
        Ok(())
    })?;
    let mut cat = DbVerifyCatalog::start(pool, lib, VerifyTrigger::Manual)?;
    let run_id = cat.run_id();
    let opts = VerifyOptions { quarantine: true, ..VerifyOptions::default() };
    match verify_library(&mut cat, root, &opts, Some(cancel)) {
        Ok(report) if report.cancelled => {
            pool.write(|c| loupe_db::repo::verify::finish_run(c, run_id, "cancelled"))?;
        }
        Ok(_) => {
            cat.finish()?;
        }
        Err(e) => {
            pool.write(|c| loupe_db::repo::verify::finish_run(c, run_id, "failed"))?;
            return Err(e.into());
        }
    }
    Ok(())
}

/// Son çalıştırmalar (en yeni önce).
pub fn recent(pool: &Pool, limit: u32) -> AppResult<Vec<VerifyRunDto>> {
    Ok(pool.read(|c| {
        Ok(c.prepare_cached(
            "SELECT r.id, r.library_id, l.name, r.trigger, r.state, r.total, r.checked, r.ok, r.missing,
                    r.mismatched, r.unreadable, r.bytes_read, r.started_at, r.finished_at
               FROM verify_runs r JOIN libraries l ON l.id = r.library_id
              ORDER BY r.started_at DESC, r.id DESC LIMIT ?1",
        )?
        .query_map([limit.clamp(1, 100)], |r| {
            let u = |i: usize| r.get::<_, i64>(i).map(|v| u64::try_from(v).unwrap_or(0));
            Ok(VerifyRunDto {
                id: r.get(0)?,
                library_id: r.get(1)?,
                library: r.get(2)?,
                trigger: r.get(3)?,
                state: r.get(4)?,
                total: u(5)?,
                checked: u(6)?,
                ok: u(7)?,
                missing: u(8)?,
                mismatched: u(9)?,
                unreadable: u(10)?,
                bytes_read: u(11)?,
                started_at: r.get(12)?,
                finished_at: r.get(13)?,
            })
        })?
        .collect::<Result<_, _>>()?)
    })?)
}

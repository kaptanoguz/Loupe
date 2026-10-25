//! Verify Library: bit-rot taraması (RFC-001A §3.7).
//!
//! Her dosya diskten (sayfa önbelleği atlanarak) okunur, BLAKE3'ü katalogdakiyle
//! karşılaştırılır. Sonuç kataloğa dosya dosya yazılır; tarama yarıda kalırsa
//! kaldığı yerden sürer. Kütüphane işareti bulunamazsa tarama başlamaz: bağlı
//! olmayan bir disk "bütün dosyalar kayıp" diye raporlanmaz.

use std::fs::{self, File};
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use loupe_core::{RelPath, VerifyCatalog, VerifyEntry, VerifyOutcome};

use crate::fsutil::{create_dir_all_durable, drop_page_cache};
use crate::hash::hash_reader;
use crate::volume::LibraryMarker;
use crate::{LOUPE_DIR, StorageError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifyOptions {
    pub batch_size: usize,
    pub buffer_size: usize,
    /// Bozuk dosyanın bir kopyasını `.loupe/quarantine/` altına al (kanıt; buluttan
    /// geri yükleme orijinalin üzerine yazmadan önce).
    pub quarantine: bool,
}

impl Default for VerifyOptions {
    fn default() -> Self {
        Self { batch_size: 256, buffer_size: 1024 * 1024, quarantine: false }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub entry: VerifyEntry,
    pub outcome: VerifyOutcome,
    pub quarantine: Option<RelPath>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VerifyReport {
    pub checked: u64,
    pub ok: u64,
    pub bytes_read: u64,
    pub findings: Vec<Finding>,
    pub cancelled: bool,
}

impl VerifyReport {
    #[must_use]
    pub fn count(&self, f: impl Fn(&VerifyOutcome) -> bool) -> usize {
        self.findings.iter().filter(|x| f(&x.outcome)).count()
    }
}

/// Tek dosyayı doğrular. Okunan bayt sayısını da döndürür.
#[must_use]
pub fn verify_entry(root: &Path, entry: &VerifyEntry, buffer_size: usize) -> (VerifyOutcome, u64) {
    let path = entry.rel_path.to_path(root);
    let mut f = match File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return (VerifyOutcome::Missing, 0),
        Err(e) => return (VerifyOutcome::Unreadable { reason: e.to_string() }, 0),
    };
    drop_page_cache(&f);
    let res = hash_reader(&mut f, buffer_size, None);
    drop_page_cache(&f);
    match res {
        Ok((actual, n)) if actual == entry.blake3 => (VerifyOutcome::Ok, n),
        Ok((actual, n)) => (VerifyOutcome::Mismatch { actual }, n),
        Err(e) => (VerifyOutcome::Unreadable { reason: e.to_string() }, 0),
    }
}

fn quarantine_copy(root: &Path, entry: &VerifyEntry) -> Result<RelPath, StorageError> {
    let rel = RelPath::new(&format!("{LOUPE_DIR}/quarantine/{}-{}", entry.media_id, entry.rel_path.file_name()))?;
    let dst = rel.to_path(root);
    if let Some(dir) = dst.parent() {
        create_dir_all_durable(dir)?;
    }
    fs::copy(entry.rel_path.to_path(root), &dst).map_err(StorageError::io("karantina kopyası", &dst))?;
    Ok(rel)
}

pub fn verify_library<C: VerifyCatalog>(
    catalog: &mut C,
    root: &Path,
    opts: &VerifyOptions,
    cancel: Option<&AtomicBool>,
) -> Result<VerifyReport, StorageError> {
    let mut report = VerifyReport::default();
    let mut after = None;
    loop {
        // Her partide: disk hâlâ bağlı mı? Değilse dur, kayıp diye işaretleme.
        LibraryMarker::read(root)?;
        let batch = catalog.next_batch(after, opts.batch_size.max(1)).map_err(StorageError::catalog)?;
        if batch.is_empty() {
            break;
        }
        for entry in batch {
            if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                report.cancelled = true;
                return Ok(report);
            }
            let (outcome, n) = verify_entry(root, &entry, opts.buffer_size);
            let quarantine = match outcome {
                VerifyOutcome::Mismatch { .. } if opts.quarantine => quarantine_copy(root, &entry).ok(),
                _ => None,
            };
            catalog.record(&entry, &outcome, n, quarantine.as_ref()).map_err(StorageError::catalog)?;
            report.checked += 1;
            report.bytes_read += n;
            after = Some(entry.media_id);
            if outcome.is_problem() {
                report.findings.push(Finding { entry, outcome, quarantine });
            } else {
                report.ok += 1;
            }
        }
    }
    Ok(report)
}

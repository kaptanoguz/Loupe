//! `loupe-core` katalog trait'lerinin SQLite uygulaması.

use loupe_core::{
    Blake3Hash, CommitOutcome, ImportCatalog, ImportSessionId, LibraryId, MediaId, NewMediaFile, RelPath,
    VerifyCatalog, VerifyEntry, VerifyOutcome, VerifyRunId, VerifyTrigger,
};
use rusqlite::TransactionBehavior;

use crate::models::VerifyRun;
use crate::repo::{import, media, verify};
use crate::{DbError, Pool};

/// Import sırasında katalog: kopya kontrolü okuyucudan, yazma yazardan.
/// Oturum verilirse satır oturuma bağlanır ve sayaçlar aynı transaction'da artar.
#[derive(Debug)]
pub struct DbImportCatalog<'p> {
    pool: &'p Pool,
    session: Option<ImportSessionId>,
}

impl<'p> DbImportCatalog<'p> {
    #[must_use]
    pub const fn new(pool: &'p Pool, session: Option<ImportSessionId>) -> Self {
        Self { pool, session }
    }
}

impl ImportCatalog for DbImportCatalog<'_> {
    type Error = DbError;

    fn find_by_blake3(&mut self, hash: &Blake3Hash) -> Result<Option<MediaId>, DbError> {
        self.pool.read(|c| media::find_by_blake3(c, hash))
    }

    fn commit_media(&mut self, m: &NewMediaFile) -> Result<CommitOutcome, DbError> {
        let session = self.session;
        self.pool.write(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut row = m.clone();
            if row.import_session_id.is_none() {
                row.import_session_id = session;
            }
            let out = media::insert(&tx, &row)?;
            if let Some(s) = session {
                let p = match out {
                    CommitOutcome::Inserted(_) => {
                        import::Progress { done_files: 1, done_bytes: m.size_bytes, ..Default::default() }
                    }
                    CommitOutcome::Duplicate(_) => import::Progress { skipped_dup: 1, ..Default::default() },
                };
                import::add_progress(&tx, s, p)?;
            }
            tx.commit()?;
            Ok(out)
        })
    }

    fn note_duplicate(&mut self, _existing: MediaId) -> Result<(), DbError> {
        match self.session {
            Some(s) => self
                .pool
                .write(|c| import::add_progress(c, s, import::Progress { skipped_dup: 1, ..Default::default() })),
            None => Ok(()),
        }
    }
}

/// Bir Verify Library çalıştırması. Her sonuç kendi küçük transaction'ında
/// yazılır; tarama yarıda kesilirse `cursor_media_id`'den devam edilir.
#[derive(Debug)]
pub struct DbVerifyCatalog<'p> {
    pool: &'p Pool,
    library: LibraryId,
    run: VerifyRunId,
}

impl<'p> DbVerifyCatalog<'p> {
    pub fn start(pool: &'p Pool, library: LibraryId, trigger: VerifyTrigger) -> Result<Self, DbError> {
        let run = pool.write(|c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let id = verify::start_run(&tx, library, trigger)?;
            tx.commit()?;
            Ok(id)
        })?;
        Ok(Self { pool, library, run })
    }

    #[must_use]
    pub const fn run_id(&self) -> VerifyRunId {
        self.run
    }

    /// Çalıştırmayı kapatır ve son sayaçları döndürür.
    pub fn finish(self) -> Result<VerifyRun, DbError> {
        self.pool.write(|c| verify::finish_run(c, self.run, "done"))?;
        self.pool
            .read(|c| verify::get_run(c, self.run))?
            .ok_or(DbError::NotFound { entity: "doğrulama çalıştırması", id: self.run.0 })
    }
}

impl VerifyCatalog for DbVerifyCatalog<'_> {
    type Error = DbError;

    fn next_batch(&mut self, after: Option<MediaId>, limit: usize) -> Result<Vec<VerifyEntry>, DbError> {
        self.pool.read(|c| verify::entries_after(c, self.library, after, limit))
    }

    fn record(
        &mut self,
        entry: &VerifyEntry,
        outcome: &VerifyOutcome,
        bytes_read: u64,
        quarantine: Option<&RelPath>,
    ) -> Result<(), DbError> {
        let run = self.run;
        self.pool.write(|c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            verify::record(&tx, run, entry, outcome, bytes_read, quarantine)?;
            tx.commit()?;
            Ok(())
        })
    }
}

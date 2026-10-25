//! Toplu yazma: 500 satırlık `BEGIN IMMEDIATE` transaction'lar.
//!
//! İlk kütüphane taraması on binlerce satır üretir. Satır başına transaction
//! her satırda WAL fsync'i ve 16 tetikleyicinin ayrı ayrı çalışması demektir;
//! tek dev transaction ise okuyucuları uzun süre eski anlık görüntüde bırakır ve
//! WAL'ı şişirir. 500'lük parçalar ikisinin arasıdır (PR #1 onay notu).
//! `IMMEDIATE`: yazma kilidi baştan alınır, yarıda `SQLITE_BUSY` ile geri alınma olmaz.

use loupe_core::{CommitOutcome, ImportItemId, ImportSessionId, NewMediaFile, now_ms};
use rusqlite::{Connection, TransactionBehavior, params};

use crate::DbError;
use crate::convert::to_i64;
use crate::models::NewImportItem;
use crate::repo::media::{ID_BY_BLAKE3_SQL, INSERT_SQL, insert_with};

pub const BATCH_SIZE: usize = 500;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchReport {
    /// Girdiyle aynı sırada, her satırın sonucu.
    pub outcomes: Vec<CommitOutcome>,
    /// Açılan (ve commit edilen) transaction sayısı.
    pub transactions: usize,
}

impl BatchReport {
    #[must_use]
    pub fn inserted(&self) -> usize {
        self.outcomes.iter().filter(|o| matches!(o, CommitOutcome::Inserted(_))).count()
    }

    #[must_use]
    pub fn duplicates(&self) -> usize {
        self.outcomes.len() - self.inserted()
    }
}

/// `media_files`'a toplu ekleme. Aynı BLAKE3'e sahip satırlar (katalogda veya
/// aynı girdide) yazılmaz, `Duplicate` olarak raporlanır. Bir parçada hata
/// olursa o parça tamamen geri alınır, önceki parçalar kalıcıdır.
pub fn insert_media_batch(conn: &mut Connection, rows: &[NewMediaFile]) -> Result<BatchReport, DbError> {
    let mut report = BatchReport { outcomes: Vec::with_capacity(rows.len()), transactions: 0 };
    for chunk in rows.chunks(BATCH_SIZE) {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut ins = tx.prepare_cached(INSERT_SQL)?;
            let mut look = tx.prepare_cached(ID_BY_BLAKE3_SQL)?;
            for m in chunk {
                report.outcomes.push(insert_with(&mut ins, &mut look, m)?);
            }
        }
        tx.commit()?;
        report.transactions += 1;
    }
    Ok(report)
}

/// Import öğelerini (tarama sonucu) toplu ekler. Aynı oturumda aynı kaynak yolu
/// ikinci kez eklenmez (yeniden tarama güvenli). Dönen kimlikler yeni eklenenlerdir.
pub fn insert_import_items_batch(
    conn: &mut Connection,
    session: ImportSessionId,
    items: &[NewImportItem],
) -> Result<Vec<ImportItemId>, DbError> {
    let mut ids = Vec::with_capacity(items.len());
    let now = now_ms();
    for chunk in items.chunks(BATCH_SIZE) {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO import_items (session_id, source_path, size_bytes, source_mtime_ns, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (session_id, source_path) DO NOTHING
                 RETURNING id",
            )?;
            for it in chunk {
                let mut rows = stmt.query(params![
                    session.0,
                    it.source_path,
                    to_i64("size_bytes", it.size_bytes)?,
                    it.source_mtime_ns,
                    now
                ])?;
                if let Some(r) = rows.next()? {
                    ids.push(ImportItemId(r.get(0)?));
                }
            }
        }
        tx.commit()?;
    }
    Ok(ids)
}

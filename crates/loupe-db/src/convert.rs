//! Satır okuma yardımcıları. `loupe-core` tipleri için `FromSql` yazılamadığından
//! (yetim kuralı) dönüşümler burada toplanır.

use std::str::FromStr;

use loupe_core::{Blake3Hash, CoreError};
use rusqlite::Row;
use rusqlite::types::Type;

use crate::DbError;

pub(crate) fn enum_col<T: FromStr<Err = CoreError>>(row: &Row<'_>, idx: usize) -> rusqlite::Result<T> {
    let s: String = row.get(idx)?;
    s.parse().map_err(|e: CoreError| rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e)))
}

pub(crate) fn hash_col(row: &Row<'_>, idx: usize) -> rusqlite::Result<Blake3Hash> {
    let b: Vec<u8> = row.get(idx)?;
    Blake3Hash::from_slice(&b).map_err(|e| rusqlite::Error::FromSqlConversionFailure(idx, Type::Blob, Box::new(e)))
}

pub(crate) fn rel_path_col(row: &Row<'_>, idx: usize) -> rusqlite::Result<loupe_core::RelPath> {
    let s: String = row.get(idx)?;
    loupe_core::RelPath::new(&s).map_err(|e| rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e)))
}

pub(crate) fn bool_col(row: &Row<'_>, idx: usize) -> rusqlite::Result<bool> {
    Ok(row.get::<_, i64>(idx)? != 0)
}

/// SQLite INTEGER işaretlidir; boyut gibi doğal olarak negatif olmayan değerler
/// sığmıyorsa sessizce sarmak yerine hata verilir.
pub(crate) fn to_i64(column: &'static str, v: u64) -> Result<i64, DbError> {
    i64::try_from(v).map_err(|_| DbError::OutOfRange { column, value: v })
}

pub(crate) fn from_i64(v: i64) -> u64 {
    u64::try_from(v).unwrap_or(0)
}

/// inode ve st_dev tam 64 bit kullanabilir; bit deseni korunarak saklanır.
pub(crate) const fn bits_to_i64(v: u64) -> i64 {
    i64::from_ne_bytes(v.to_ne_bytes())
}

pub(crate) const fn bits_from_i64(v: i64) -> u64 {
    u64::from_ne_bytes(v.to_ne_bytes())
}

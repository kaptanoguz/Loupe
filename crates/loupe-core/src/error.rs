use thiserror::Error;

/// Ortak tiplerin doğrulama hataları.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CoreError {
    #[error("geçersiz göreli yol {path:?}: {reason}")]
    InvalidRelPath { path: String, reason: &'static str },

    #[error("BLAKE3 hash'i 32 bayt olmalı, {0} bayt geldi")]
    InvalidHashLength(usize),

    #[error("geçersiz hex hash: {0:?}")]
    InvalidHashHex(String),

    #[error("{kind} için bilinmeyen değer: {value:?}")]
    UnknownVariant { kind: &'static str, value: String },
}

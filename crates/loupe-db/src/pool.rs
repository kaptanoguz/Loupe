//! Bağlantı havuzu: tek yazar bağlantısı + yeniden kullanılan okuyucular.
//!
//! SQLite'ta aynı anda tek yazar olabilir; birden çok yazar bağlantısı yalnızca
//! `SQLITE_BUSY` beklemesi üretir. Bu yüzden yazar bir `Mutex` arkasındadır ve
//! yazma işleri sıraya girer. WAL sayesinde okuyucular yazarı hiç beklemez.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use rusqlite::Connection;

use crate::DbError;
use crate::connection::{ConnectionConfig, Role, open_connection};
use crate::migrate::{MigrationReport, migrate, validate_schema};

#[derive(Debug, Clone)]
pub struct PoolConfig {
    pub connection: ConnectionConfig,
    /// Boşta tutulan en fazla okuyucu bağlantısı. Fazlası iş bitince kapanır.
    pub max_idle_readers: usize,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self { connection: ConnectionConfig::default(), max_idle_readers: 4 }
    }
}

#[derive(Debug)]
pub struct Pool {
    path: PathBuf,
    cfg: PoolConfig,
    writer: Mutex<Connection>,
    readers: Mutex<Vec<Connection>>,
    migration: MigrationReport,
}

impl Pool {
    /// Veritabanını açar (yoksa oluşturur), migrasyonları uygular ve şemayı doğrular.
    pub fn open(path: &Path, cfg: PoolConfig) -> Result<Self, DbError> {
        let mut writer = open_connection(path, Role::Writer, &cfg.connection)?;
        let migration = migrate(&mut writer)?;
        validate_schema(&writer)?;
        Ok(Self {
            path: path.to_path_buf(),
            cfg,
            writer: Mutex::new(writer),
            readers: Mutex::new(Vec::new()),
            migration,
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Açılışta hangi migrasyonların uygulandığı.
    #[must_use]
    pub const fn migration_report(&self) -> &MigrationReport {
        &self.migration
    }

    /// Yazar bağlantısıyla iş yapar. Transaction'ı iş kendisi açar
    /// (`conn.transaction_with_behavior(Immediate)`), böylece kapsamı açıktır.
    pub fn write<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T, DbError>) -> Result<T, DbError> {
        let mut guard = self.writer.lock().map_err(|_| DbError::PoolPoisoned)?;
        f(&mut guard)
    }

    /// Bir okuyucu bağlantısıyla iş yapar. Bağlantı boştaysa yeniden kullanılır,
    /// değilse açılır; iş bitince havuza döner (iş paniklerse yalnızca kapanır).
    pub fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T, DbError>) -> Result<T, DbError> {
        let idle = self.lock_readers()?.pop();
        let conn = match idle {
            Some(c) => c,
            None => open_connection(&self.path, Role::Reader, &self.cfg.connection)?,
        };
        let out = f(&conn);
        self.give_back(conn);
        out
    }

    fn lock_readers(&self) -> Result<MutexGuard<'_, Vec<Connection>>, DbError> {
        self.readers.lock().map_err(|_| DbError::PoolPoisoned)
    }

    fn give_back(&self, conn: Connection) {
        // Kilit zehirliyse veya havuz doluysa bağlantı kapanır; hata yaymaya gerek yok.
        if let Ok(mut idle) = self.readers.lock() {
            if idle.len() < self.cfg.max_idle_readers {
                idle.push(conn);
            }
        }
    }
}

//! sqlite-vec'in statik kaydı. Crate'teki tek `unsafe` buradadır.
//!
//! `sqlite3_auto_extension` süreç başına bir kez çağrılır; sonra açılan her
//! bağlantıda `vec0` sanal tablosu ve `vec_*` fonksiyonları hazırdır. Dinamik
//! `load_extension` kullanılmaz (dağıtımda .so araması yok, saldırı yüzeyi yok).

use std::sync::OnceLock;

use crate::DbError;

static REGISTERED: OnceLock<i32> = OnceLock::new();

pub(crate) fn ensure_registered() -> Result<(), DbError> {
    let rc = *REGISTERED.get_or_init(register);
    if rc == rusqlite::ffi::SQLITE_OK { Ok(()) } else { Err(DbError::VecRegister(rc)) }
}

#[allow(unsafe_code)]
fn register() -> i32 {
    type EntryPoint = unsafe extern "C" fn(
        *mut rusqlite::ffi::sqlite3,
        *mut *mut std::os::raw::c_char,
        *const rusqlite::ffi::sqlite3_api_routines,
    ) -> std::os::raw::c_int;
    // SAFETY: sqlite3_vec_init, SQLite eklenti giriş noktası imzasına sahip bir C
    // fonksiyonudur; sqlite-vec crate'i onu argümansız bildirir. İşaretçiyi gerçek
    // imzaya dönüştürmek, SQLite'ın beklediği çağrı biçimiyle birebir aynıdır.
    // auto_extension kaydı iş parçacığı güvenlidir ve OnceLock ile bir kez yapılır.
    unsafe {
        let entry: EntryPoint = std::mem::transmute::<*const (), EntryPoint>(sqlite_vec::sqlite3_vec_init as *const ());
        rusqlite::ffi::sqlite3_auto_extension(Some(entry))
    }
}

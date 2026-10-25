//! Tablo bazında CRUD. Fonksiyonlar `&Connection` alır; bir `Transaction` da
//! `Connection`'a dönüştüğü için çağıran taraf işleri tek transaction'da birleştirebilir.

pub mod import;
pub mod library;
pub mod location;
pub mod media;
pub mod thumbnail;
pub mod verify;

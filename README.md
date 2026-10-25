# Loupe

Linux için yerel öncelikli (local-first) fotoğraf kataloglama, LAN senkronizasyon ve hibrit bulut yedekleme uygulaması.

Durum: Adım 2A — çekirdek Rust crate'leri (`loupe-core`, `loupe-db`, `loupe-storage`).

- [RFC-001A — Veritabanı ve Depolama Mimarisi](docs/rfc/RFC-001A-veritabani-ve-depolama.md)
- [RFC-001B — İş Kuyruğu ve AI, LAN Sync, Bulut Yedekleme, Tasarım Sistemi](docs/rfc/RFC-001B-kuyruk-lan-bulut-tasarim.md)
- Şema: [`crates/loupe-db/migrations/`](crates/loupe-db/migrations/) · test: `python3 tools/schema-check/check_schema.py crates/loupe-db/migrations`
- Tasarım sistemi: [`apps/desktop/src/app.css`](apps/desktop/src/app.css) · koruma: `tools/slop-guard/slop-guard.sh`

## Crate'ler

| Crate | İçerik |
|---|---|
| `loupe-core` | Kimlikler, `Blake3Hash`, `PHash`, `RelPath` (NFC), şema enum'ları, katalog trait'leri (`ImportCatalog`, `VerifyCatalog`) |
| `loupe-db` | SQLite havuzu (WAL, NORMAL, mmap, foreign_keys, busy_timeout), gömülü migrasyon motoru + doğrulama, sqlite-vec, CRUD, 500'lük `BEGIN IMMEDIATE` toplu yazma |
| `loupe-storage` | Disk/UUID çözümleme ve yeniden konumlandırma, tek geçişte BLAKE3 + pHash, atomik importer (Kopyala/Taşı/Hardlink, `.loupe-tmp/`), debounce'lu izleyici, Verify Library |

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings   # üretim kodunda unwrap/expect/panic derlemeyi kırar
cargo fmt --check
```

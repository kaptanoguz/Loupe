# Loupe

Linux için yerel öncelikli (local-first) fotoğraf kataloglama, LAN senkronizasyon ve hibrit bulut yedekleme uygulaması.

Durum: mimari tasarım aşaması.

- [RFC-001A — Veritabanı ve Depolama Mimarisi](docs/rfc/RFC-001A-veritabani-ve-depolama.md)
- [RFC-001B — İş Kuyruğu ve AI, LAN Sync, Bulut Yedekleme, Tasarım Sistemi](docs/rfc/RFC-001B-kuyruk-lan-bulut-tasarim.md)
- Şema: [`crates/loupe-db/migrations/`](crates/loupe-db/migrations/) · test: `python3 tools/schema-check/check_schema.py crates/loupe-db/migrations`
- Tasarım sistemi: [`apps/desktop/src/app.css`](apps/desktop/src/app.css) · koruma: `tools/slop-guard/slop-guard.sh`

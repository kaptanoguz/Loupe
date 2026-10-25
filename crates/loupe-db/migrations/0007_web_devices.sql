-- =============================================================================
-- Loupe — Katalog şeması, migrasyon 0007: tarayıcıdan eşleşen telefonlar
-- Adım 4 (RFC-001G): telefon kurulum gerektirmeden, QR'daki https adresini
-- tarayıcıda açarak eşleşir. Tarayıcı istemci sertifikası sunamadığı için
-- kimlik, sunucunun verdiği rastgele oturum anahtarıdır.
--
-- kind = 'app': mTLS istemci sertifikası; cert_sha256 sertifikanın özeti.
-- kind = 'web': oturum anahtarı; cert_sha256 = SHA-256("loupe-web-session-v1\0"
--               ‖ anahtar). Anahtarın kendisi DB'de tutulmaz. cert_not_after
--               oturumun bitişidir; iptal ve son görülme aynı sütunlarla çalışır.
-- =============================================================================

PRAGMA user_version = 7;

ALTER TABLE devices ADD COLUMN kind TEXT NOT NULL DEFAULT 'app' CHECK (kind IN ('app','web'));

//! Yerel CA, sunucu sertifikası ve istemci sertifikası imzalama (RFC-001B §2.1).
//!
//! - CA: ECDSA P-256, 10 yıl. Telefon eşleşmeden sonra yalnızca bu CA'ya güvenir;
//!   sunucu sertifikası yenilense de yeniden eşleşme gerekmez.
//! - Sunucu sertifikası: CA imzalı, 1 yıl, SAN = [`SERVER_NAME`]. IP adresi
//!   sertifikaya yazılmaz (DHCP değiştirir); istemci bu sabit adı doğrular.
//! - İstemci sertifikası: telefonun CSR'ından, 2 yıl. Telefonun özel anahtarı
//!   telefonu hiç terk etmez; CSR'daki uzantılar yok sayılır, sertifikayı
//!   yalnızca sunucunun kuralları belirler.

use std::fmt;

use rand::RngCore;
use rcgen::{
    BasicConstraints, CertificateParams, CertificateSigningRequestParams, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, PublicKeyData,
    SerialNumber,
};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use crate::LanError;

/// Sunucu sertifikasındaki tek DNS adı. İstemciler bağlandıkları IP yerine bu
/// adı doğrular (bkz. [`crate::tls::ServerPin::Ca`]).
pub const SERVER_NAME: &str = "loupe.local";

const CA_DAYS: i64 = 3650;
const SERVER_DAYS: i64 = 365;
const CLIENT_DAYS: i64 = 730;
/// Saat kayması payı: sertifikalar bir saat öncesinden geçerli başlar.
const SKEW_SECS: i64 = 3600;

#[must_use]
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn datetime(ms: i64) -> Result<OffsetDateTime, LanError> {
    OffsetDateTime::from_unix_timestamp(ms.div_euclid(1000))
        .map_err(|e| LanError::BadRequest(format!("geçersiz zaman: {e}")))
}

pub(crate) fn random_serial() -> SerialNumber {
    let mut b = [0u8; 16];
    rand::rng().fill_bytes(&mut b);
    // DER INTEGER pozitif kalsın.
    b[0] &= 0x7f;
    b[0] |= 0x01;
    SerialNumber::from_slice(&b)
}

pub(crate) fn validity(params: &mut CertificateParams, now_ms: i64, days: i64) -> Result<i64, LanError> {
    let now = datetime(now_ms)?;
    params.not_before = now - time::Duration::seconds(SKEW_SECS);
    params.not_after = now + time::Duration::days(days);
    Ok(params.not_after.unix_timestamp().saturating_mul(1000))
}

pub(crate) fn dn(common_name: &str) -> DistinguishedName {
    let mut d = DistinguishedName::new();
    d.push(DnType::OrganizationName, "Loupe");
    d.push(DnType::CommonName, common_name);
    d
}

/// Kalıcı saklama biçimi. Daemon özel anahtarları Secret Service'e yazar;
/// bu yapı yalnızca taşıma içindir ve diske düz yazılmamalıdır.
#[derive(Clone, PartialEq, Eq)]
pub struct IdentityPem {
    pub install_id: String,
    pub ca_cert_pem: String,
    pub ca_key_pem: String,
    pub server_cert_pem: String,
    pub server_key_pem: String,
}

impl fmt::Debug for IdentityPem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IdentityPem").field("install_id", &self.install_id).finish_non_exhaustive()
    }
}

/// Telefona verilen istemci sertifikası ve `devices` satırına yazılacak alanlar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedClientCert {
    pub cert_der: CertificateDer<'static>,
    pub cert_pem: String,
    pub cert_sha256: [u8; 32],
    pub spki_sha256: [u8; 32],
    pub serial: Vec<u8>,
    pub not_after_ms: i64,
}

pub struct ServerIdentity {
    install_id: String,
    ca_cert: CertificateDer<'static>,
    ca_cert_pem: String,
    ca_issuer: Issuer<'static, KeyPair>,
    ca_spki_sha256: [u8; 32],
    server_cert: CertificateDer<'static>,
    server_cert_pem: String,
    server_key: KeyPair,
}

impl fmt::Debug for ServerIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerIdentity")
            .field("install_id", &self.install_id)
            .field("ca_spki_sha256", &hex(&self.ca_spki_sha256))
            .field("server_cert_sha256", &hex(&self.server_cert_sha256()))
            .finish_non_exhaustive()
    }
}

pub(crate) fn hex(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter().fold(String::with_capacity(b.len() * 2), |mut s, x| {
        let _infallible = write!(s, "{x:02x}");
        s
    })
}

impl ServerIdentity {
    /// Yeni kurulum kimliği, CA ve sunucu sertifikası üretir.
    pub fn generate(display_name: &str, now_ms: i64) -> Result<Self, LanError> {
        let mut id = [0u8; 8];
        rand::rng().fill_bytes(&mut id);
        let install_id = hex(&id);

        let ca_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
        let mut ca = CertificateParams::default();
        ca.distinguished_name = dn(&format!("Loupe yerel CA {install_id}"));
        ca.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        ca.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign, KeyUsagePurpose::DigitalSignature];
        ca.serial_number = Some(random_serial());
        validity(&mut ca, now_ms, CA_DAYS)?;
        let ca_cert = ca.self_signed(&ca_key)?;
        let ca_spki_sha256 = sha256(&ca_key.subject_public_key_info());
        let ca_issuer = Issuer::new(ca, ca_key);

        let (server_cert, server_key) = Self::issue_server_cert(&ca_issuer, display_name, now_ms)?;
        Ok(Self {
            install_id,
            ca_cert_pem: ca_cert.pem(),
            ca_cert: ca_cert.der().clone(),
            ca_issuer,
            ca_spki_sha256,
            server_cert_pem: server_cert.pem(),
            server_cert: server_cert.der().clone(),
            server_key,
        })
    }

    fn issue_server_cert(
        ca: &Issuer<'static, KeyPair>,
        display_name: &str,
        now_ms: i64,
    ) -> Result<(rcgen::Certificate, KeyPair), LanError> {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
        let mut p = CertificateParams::new(vec![SERVER_NAME.to_owned()])?;
        p.distinguished_name = dn(display_name);
        p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        p.serial_number = Some(random_serial());
        validity(&mut p, now_ms, SERVER_DAYS)?;
        let cert = p.signed_by(&key, ca)?;
        Ok((cert, key))
    }

    /// Sunucu sertifikasını aynı CA ile yeniler (yıllık). Eşleşmiş telefonlar
    /// CA'ya güvendiği için etkilenmez.
    pub fn renew_server_cert(&mut self, display_name: &str, now_ms: i64) -> Result<(), LanError> {
        let (cert, key) = Self::issue_server_cert(&self.ca_issuer, display_name, now_ms)?;
        self.server_cert_pem = cert.pem();
        self.server_cert = cert.der().clone();
        self.server_key = key;
        Ok(())
    }

    pub fn from_pem(p: &IdentityPem) -> Result<Self, LanError> {
        let ca_key = KeyPair::from_pem(&p.ca_key_pem)?;
        let ca_spki_sha256 = sha256(&ca_key.subject_public_key_info());
        let ca_cert = pem_cert(&p.ca_cert_pem)?;
        let ca_issuer = Issuer::from_ca_cert_der(&ca_cert, ca_key)?;
        Ok(Self {
            install_id: p.install_id.clone(),
            ca_cert,
            ca_cert_pem: p.ca_cert_pem.clone(),
            ca_issuer,
            ca_spki_sha256,
            server_cert: pem_cert(&p.server_cert_pem)?,
            server_cert_pem: p.server_cert_pem.clone(),
            server_key: KeyPair::from_pem(&p.server_key_pem)?,
        })
    }

    #[must_use]
    pub fn to_pem(&self) -> IdentityPem {
        IdentityPem {
            install_id: self.install_id.clone(),
            ca_cert_pem: self.ca_cert_pem.clone(),
            ca_key_pem: self.ca_issuer.key().serialize_pem(),
            server_cert_pem: self.server_cert_pem.clone(),
            server_key_pem: self.server_key.serialize_pem(),
        }
    }

    /// Telefonun CSR'ını doğrular (imza) ve 2 yıllık istemci sertifikası imzalar.
    /// Yalnızca P-256 anahtar kabul edilir (iOS Secure Enclave ile uyumlu).
    pub fn sign_client_csr(&self, csr_pem: &str, device_name: &str, now_ms: i64) -> Result<IssuedClientCert, LanError> {
        let csr = CertificateSigningRequestParams::from_pem(csr_pem)?;
        if csr.public_key.algorithm() != &PKCS_ECDSA_P256_SHA256 {
            return Err(LanError::BadRequest("istemci anahtarı ECDSA P-256 olmalı".into()));
        }
        let mut p = CertificateParams::default();
        p.distinguished_name = dn(device_name);
        p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let serial = random_serial();
        p.serial_number = Some(serial.clone());
        let not_after_ms = validity(&mut p, now_ms, CLIENT_DAYS)?;
        let cert = p.signed_by(&csr.public_key, &self.ca_issuer)?;
        let der = cert.der().clone();
        Ok(IssuedClientCert {
            cert_sha256: sha256(&der),
            spki_sha256: sha256(&csr.public_key.subject_public_key_info()),
            cert_pem: cert.pem(),
            cert_der: der,
            serial: serial.to_bytes(),
            not_after_ms,
        })
    }

    #[must_use]
    pub fn install_id(&self) -> &str {
        &self.install_id
    }

    #[must_use]
    pub const fn ca_cert(&self) -> &CertificateDer<'static> {
        &self.ca_cert
    }

    #[must_use]
    pub fn ca_cert_pem(&self) -> &str {
        &self.ca_cert_pem
    }

    /// QR'daki `ca`: telefonun sabitlediği CA açık anahtarı (SPKI) özeti.
    #[must_use]
    pub const fn ca_spki_sha256(&self) -> [u8; 32] {
        self.ca_spki_sha256
    }

    #[must_use]
    pub const fn server_cert(&self) -> &CertificateDer<'static> {
        &self.server_cert
    }

    /// QR'daki `fp`: sunucu sertifikasının SHA-256 parmak izi. Telefon ilk
    /// (sertifikasız) bağlantıda sunucuyu bununla tanır.
    #[must_use]
    pub fn server_cert_sha256(&self) -> [u8; 32] {
        sha256(&self.server_cert)
    }

    pub(crate) fn server_key_der(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.server_key.serialize_der()))
    }
}

pub(crate) fn pem_cert(pem: &str) -> Result<CertificateDer<'static>, LanError> {
    use rustls_pki_types::pem::PemObject;
    CertificateDer::from_pem_slice(pem.as_bytes()).map_err(|e| LanError::BadRequest(format!("PEM sertifika: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_761_390_000_000;

    #[test]
    fn pem_round_trip_keeps_fingerprints_and_can_sign() {
        let id = ServerIdentity::generate("Masaüstü", NOW).unwrap();
        let back = ServerIdentity::from_pem(&id.to_pem()).unwrap();
        assert_eq!(back.ca_spki_sha256(), id.ca_spki_sha256());
        assert_eq!(back.server_cert_sha256(), id.server_cert_sha256());

        let phone = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let csr = CertificateParams::default().serialize_request(&phone).unwrap().pem().unwrap();
        let issued = back.sign_client_csr(&csr, "Pixel 9", NOW).unwrap();
        assert_eq!(issued.spki_sha256, sha256(&phone.subject_public_key_info()));
        assert_eq!(issued.not_after_ms, NOW + CLIENT_DAYS * 86_400_000);
    }

    #[test]
    fn non_p256_csr_is_rejected() {
        let id = ServerIdentity::generate("Masaüstü", NOW).unwrap();
        let ed = KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
        let csr = CertificateParams::default().serialize_request(&ed).unwrap().pem().unwrap();
        assert!(matches!(id.sign_client_csr(&csr, "x", NOW), Err(LanError::BadRequest(_))));
        assert!(id.sign_client_csr("-----BEGIN CERTIFICATE REQUEST-----\nAAAA\n", "x", NOW).is_err());
    }
}

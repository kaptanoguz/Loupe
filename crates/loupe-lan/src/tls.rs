//! TLS 1.3 yapılandırmaları (RFC-001B §2.4–2.5).
//!
//! **Sunucu:** istemci sertifikası *istenir* ama zorunlu değildir; yalnızca
//! `POST /pair` sertifikasız çalışır, diğer her uç nokta eşleşmiş cihaz ister.
//! Gönderilen sertifika (1) yerel CA'ya zincirlenmeli, (2) parmak izi
//! [`DeviceRegistry`]'de olmalı. İptal edilen cihaz kayıttan düşer ve bir sonraki
//! el sıkışmada reddedilir; yeniden başlatma gerekmez.
//!
//! **İstemci (telefon ve testler):** sunucu iki yoldan biriyle doğrulanır:
//! eşleşmeden önce QR'daki sertifika parmak izi ([`ServerPin::Leaf`]), sonra
//! eşleşmede alınan CA ([`ServerPin::Ca`]).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::server::WebPkiClientVerifier;
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, DistinguishedName, Error, RootCertStore, ServerConfig,
    SignatureScheme,
};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};

use crate::identity::sha256;
use crate::{LanError, SERVER_NAME, ServerIdentity};

const ALPN: [&[u8]; 2] = [b"h2", b"http/1.1"];

#[must_use]
pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn builder_err(e: impl std::fmt::Display) -> LanError {
    LanError::Tls(Error::General(e.to_string()))
}

/// Eşleşmiş ve iptal edilmemiş cihazların sertifika parmak izleri → `devices.id`.
#[derive(Debug, Default)]
pub struct DeviceRegistry {
    by_cert: RwLock<HashMap<[u8; 32], i64>>,
}

impl DeviceRegistry {
    pub fn insert(&self, cert_sha256: [u8; 32], device_id: i64) {
        if let Ok(mut m) = self.by_cert.write() {
            m.insert(cert_sha256, device_id);
        }
    }

    /// Cihaz iptal edildi.
    pub fn remove_device(&self, device_id: i64) {
        if let Ok(mut m) = self.by_cert.write() {
            m.retain(|_, d| *d != device_id);
        }
    }

    pub fn replace_all(&self, rows: impl IntoIterator<Item = ([u8; 32], i64)>) {
        if let Ok(mut m) = self.by_cert.write() {
            *m = rows.into_iter().collect();
        }
    }

    #[must_use]
    pub fn lookup(&self, cert_sha256: &[u8; 32]) -> Option<i64> {
        self.by_cert.read().ok().and_then(|m| m.get(cert_sha256).copied())
    }
}

#[derive(Debug)]
struct DeviceCertVerifier {
    chain: Arc<dyn ClientCertVerifier>,
    registry: Arc<DeviceRegistry>,
}

impl ClientCertVerifier for DeviceCertVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        false
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        self.chain.root_hint_subjects()
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<ClientCertVerified, Error> {
        let ok = self.chain.verify_client_cert(end_entity, intermediates, now)?;
        if self.registry.lookup(&sha256(end_entity)).is_none() {
            return Err(Error::InvalidCertificate(CertificateError::Revoked));
        }
        Ok(ok)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.chain.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.chain.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.chain.supported_verify_schemes()
    }
}

/// Sunucu: TLS 1.3, ALPN h2 + http/1.1, zincir = sunucu sertifikası + CA.
pub fn server_config(identity: &ServerIdentity, registry: Arc<DeviceRegistry>) -> Result<Arc<ServerConfig>, LanError> {
    let provider = provider();
    let mut roots = RootCertStore::empty();
    roots.add(identity.ca_cert().clone())?;
    let chain = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider.clone())
        .allow_unauthenticated()
        .build()
        .map_err(builder_err)?;
    let mut cfg = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(Arc::new(DeviceCertVerifier { chain, registry }))
        .with_single_cert(
            vec![identity.server_cert().clone(), identity.ca_cert().clone()],
            identity.server_key_der(),
        )?;
    cfg.alpn_protocols = ALPN.iter().map(|p| p.to_vec()).collect();
    Ok(Arc::new(cfg))
}

/// İstemcinin sunucuyu tanıma yolu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerPin {
    /// Eşleşmeden önce: QR'daki sunucu sertifikası SHA-256'sı.
    Leaf([u8; 32]),
    /// Eşleşmeden sonra: yerel CA. Sertifika [`SERVER_NAME`] için doğrulanır,
    /// bağlanılan IP'ye bakılmaz.
    Ca(CertificateDer<'static>),
}

#[derive(Debug)]
struct PinnedServerVerifier {
    pin: ServerPin,
    provider: Arc<CryptoProvider>,
    webpki: Option<Arc<WebPkiServerVerifier>>,
}

impl ServerCertVerifier for PinnedServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        match (&self.pin, &self.webpki) {
            (ServerPin::Leaf(fp), _) => {
                if &sha256(end_entity) == fp {
                    Ok(ServerCertVerified::assertion())
                } else {
                    Err(Error::InvalidCertificate(CertificateError::ApplicationVerificationFailure))
                }
            }
            (ServerPin::Ca(_), Some(v)) => {
                let name =
                    ServerName::try_from(SERVER_NAME).map_err(|_| Error::General("sunucu adı geçersiz".into()))?;
                v.verify_server_cert(end_entity, intermediates, &name, ocsp_response, now)
            }
            (ServerPin::Ca(_), None) => Err(Error::General("CA doğrulayıcısı kurulmamış".into())),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// İstemci yapılandırması. `client_auth` verilirse mTLS (zincir + özel anahtar).
pub fn client_config(
    pin: ServerPin,
    client_auth: Option<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)>,
) -> Result<ClientConfig, LanError> {
    let provider = provider();
    let webpki = match &pin {
        ServerPin::Ca(ca) => {
            let mut roots = RootCertStore::empty();
            roots.add(ca.clone())?;
            Some(
                WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone())
                    .build()
                    .map_err(builder_err)?,
            )
        }
        ServerPin::Leaf(_) => None,
    };
    let verifier = Arc::new(PinnedServerVerifier { pin, provider: provider.clone(), webpki });
    let builder = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(verifier);
    let mut cfg = match client_auth {
        Some((chain, key)) => builder.with_client_auth_cert(chain, key)?,
        None => builder.with_no_client_auth(),
    };
    cfg.alpn_protocols = ALPN.iter().map(|p| p.to_vec()).collect();
    Ok(cfg)
}

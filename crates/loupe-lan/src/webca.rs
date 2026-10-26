//! Telefonun kendi uygulamaları (Safari, Kestirmeler) için güvenilir TLS.
//!
//! Eşleşme CA'sı ([`crate::ServerIdentity`]) telefona kök olarak kurulmaz:
//! sınırsız bir kök, anahtarı sızarsa telefonun tüm trafiğini açar. Bunun
//! yerine ayrı bir **ad kısıtlı** CA vardır; yalnızca `*.local` adları ve özel
//! ağ adresleri için sertifika imzalayabilir (RFC 5280 §4.2.1.10). Telefon bu
//! CA'yı bir yapılandırma profiliyle (`/m/loupe.mobileconfig`) bir kez kurar;
//! sonra Kestirmeler `https://<makine>.local:<port>/s/upload` adresine uyarısız
//! bağlanır (SNI'ye göre sertifika seçimi: [`crate::tls`]).
//!
//! Yaprak sertifika her sunucu açılışında o anki LAN adresleri için yeniden
//! imzalanır (DHCP adresi değiştirebilir); yalnızca CA saklanır.

use std::fmt;
use std::net::IpAddr;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rand::RngCore;
use rcgen::{
    BasicConstraints, CertificateParams, CidrSubnet, DnType, ExtendedKeyUsagePurpose, GeneralSubtree, IsCa, Issuer,
    KeyPair, KeyUsagePurpose, NameConstraints, PKCS_ECDSA_P256_SHA256, SanType,
};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use crate::identity::{dn, hex, pem_cert, random_serial, validity};
use crate::{LanError, SERVER_NAME};

const CA_DAYS: i64 = 3650;
/// iOS kullanıcı köklerinden gelen sunucu sertifikasında 825 günü aşmayı kabul etmez.
const LEAF_DAYS: i64 = 365;

/// Yaprak sertifikanın imzalanabileceği özel ağlar.
const PRIVATE_NETS: [(&str, u8); 6] =
    [("10.0.0.0", 8), ("172.16.0.0", 12), ("192.168.0.0", 16), ("169.254.0.0", 16), ("fc00::", 7), ("fe80::", 10)];

/// Saklama biçimi (gizli depoya JSON olarak).
#[derive(Clone, PartialEq, Eq)]
pub struct WebCaPem {
    pub ca_cert_pem: String,
    pub ca_key_pem: String,
}

impl fmt::Debug for WebCaPem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebCaPem").finish_non_exhaustive()
    }
}

pub struct WebCa {
    cert: CertificateDer<'static>,
    cert_pem: String,
    issuer: Issuer<'static, KeyPair>,
}

impl fmt::Debug for WebCa {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebCa").finish_non_exhaustive()
    }
}

/// TLS'te sunulacak yaprak sertifika ve anahtarı.
#[derive(Debug)]
pub struct WebLeaf {
    pub chain: Vec<CertificateDer<'static>>,
    pub key: PrivateKeyDer<'static>,
}

fn constraints() -> Result<NameConstraints, LanError> {
    let mut permitted = vec![GeneralSubtree::DnsName("local".to_owned())];
    for (net, prefix) in PRIVATE_NETS {
        let ip: IpAddr = net.parse().map_err(|_| LanError::BadRequest(format!("ağ: {net}")))?;
        permitted.push(GeneralSubtree::IpAddress(CidrSubnet::from_addr_prefix(ip, prefix)));
    }
    Ok(NameConstraints { permitted_subtrees: permitted, excluded_subtrees: Vec::new() })
}

impl WebCa {
    pub fn generate(now_ms: i64) -> Result<Self, LanError> {
        let mut id = [0u8; 4];
        rand::rng().fill_bytes(&mut id);
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
        let mut p = CertificateParams::default();
        p.distinguished_name = dn(&format!("Loupe yerel ağ {}", hex(&id)));
        p.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign, KeyUsagePurpose::DigitalSignature];
        p.name_constraints = Some(constraints()?);
        p.serial_number = Some(random_serial());
        validity(&mut p, now_ms, CA_DAYS)?;
        let cert = p.self_signed(&key)?;
        Ok(Self { cert_pem: cert.pem(), cert: cert.der().clone(), issuer: Issuer::new(p, key) })
    }

    pub fn from_pem(p: &WebCaPem) -> Result<Self, LanError> {
        let cert = pem_cert(&p.ca_cert_pem)?;
        let issuer = Issuer::from_ca_cert_der(&cert, KeyPair::from_pem(&p.ca_key_pem)?)?;
        Ok(Self { cert, cert_pem: p.ca_cert_pem.clone(), issuer })
    }

    #[must_use]
    pub fn to_pem(&self) -> WebCaPem {
        WebCaPem { ca_cert_pem: self.cert_pem.clone(), ca_key_pem: self.issuer.key().serialize_pem() }
    }

    #[must_use]
    pub const fn cert(&self) -> &CertificateDer<'static> {
        &self.cert
    }

    /// Verilen adlar ve adresler için sunucu sertifikası. Kısıt dışı adlar
    /// (ör. genel IP) atlanır; telefon zaten onlara güvenmezdi.
    pub fn leaf(&self, hosts: &[String], ips: &[IpAddr], now_ms: i64) -> Result<WebLeaf, LanError> {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
        let mut p = CertificateParams::default();
        p.distinguished_name = dn(SERVER_NAME);
        p.distinguished_name.remove(DnType::OrganizationName);
        let mut sans = vec![SanType::DnsName(SERVER_NAME.try_into()?)];
        for h in hosts.iter().filter(|h| h.ends_with(".local") && h.as_str() != SERVER_NAME) {
            sans.push(SanType::DnsName(h.as_str().try_into()?));
        }
        sans.extend(
            ips.iter().filter(|ip| crate::server::is_lan(**ip) && !ip.is_loopback()).map(|ip| SanType::IpAddress(*ip)),
        );
        p.subject_alt_names = sans;
        p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        p.serial_number = Some(random_serial());
        validity(&mut p, now_ms, LEAF_DAYS)?;
        let cert = p.signed_by(&key, &self.issuer)?;
        Ok(WebLeaf {
            chain: vec![cert.der().clone()],
            key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        })
    }

    /// iOS/macOS yapılandırma profili: CA'yı kök sertifika olarak kurar.
    /// Kurulduktan sonra Ayarlar → Genel → Hakkında → Sertifika Güven
    /// Ayarları'ndan tam güven açılmalıdır (iOS bunu otomatik yapmaz).
    #[must_use]
    pub fn mobileconfig(&self, install_id: &str, display_name: &str) -> String {
        let der = STANDARD.encode(self.cert.as_ref());
        let name = xml_escape(display_name);
        // Kurulumun kimliğinden türeyen sabit UUID'ler: profil yeniden kurulunca eskisinin yerine geçer.
        let uuid = |salt: &str| {
            let h = crate::identity::sha256(format!("{salt}:{install_id}").as_bytes());
            format!("{}-{}-{}-{}-{}", hex(&h[0..4]), hex(&h[4..6]), hex(&h[6..8]), hex(&h[8..10]), hex(&h[10..16]))
                .to_uppercase()
        };
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>PayloadContent</key>
  <array>
    <dict>
      <key>PayloadCertificateFileName</key><string>loupe-yerel-ag.cer</string>
      <key>PayloadContent</key><data>{der}</data>
      <key>PayloadDescription</key><string>Yalnızca .local adları ve özel ağ adresleri için geçerlidir.</string>
      <key>PayloadDisplayName</key><string>Loupe yerel ağ sertifikası</string>
      <key>PayloadIdentifier</key><string>app.loupe.lan.{install_id}.ca</string>
      <key>PayloadType</key><string>com.apple.security.root</string>
      <key>PayloadUUID</key><string>{ca_uuid}</string>
      <key>PayloadVersion</key><integer>1</integer>
    </dict>
  </array>
  <key>PayloadDescription</key><string>{name} ile güvenli aktarım (Kestirmeler ve Safari).</string>
  <key>PayloadDisplayName</key><string>Loupe · {name}</string>
  <key>PayloadIdentifier</key><string>app.loupe.lan.{install_id}</string>
  <key>PayloadRemovalDisallowed</key><false/>
  <key>PayloadType</key><string>Configuration</string>
  <key>PayloadUUID</key><string>{profile_uuid}</string>
  <key>PayloadVersion</key><integer>1</integer>
</dict>
</plist>
"#,
            ca_uuid = uuid("ca"),
            profile_uuid = uuid("profile"),
        )
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use rustls::client::danger::ServerCertVerifier;

    use super::*;

    const NOW: i64 = 1_761_390_000_000;

    #[test]
    fn leaf_chains_to_constrained_ca_for_lan_names_only() {
        let ca = WebCa::generate(NOW).unwrap();
        let back = WebCa::from_pem(&ca.to_pem()).unwrap();
        let ips: Vec<IpAddr> = vec!["192.168.18.130".parse().unwrap(), "8.8.8.8".parse().unwrap()];
        let leaf = back.leaf(&["oguz-mint.local".to_owned(), "example.com".to_owned()], &ips, NOW).unwrap();

        let mut roots = rustls::RootCertStore::empty();
        roots.add(ca.cert().clone()).unwrap();
        let verifier =
            rustls::client::WebPkiServerVerifier::builder_with_provider(roots.into(), crate::tls::provider())
                .build()
                .unwrap();
        let now = rustls_pki_types::UnixTime::since_unix_epoch(std::time::Duration::from_millis(NOW as u64));
        let ok = |name: &str| {
            let sn = rustls_pki_types::ServerName::try_from(name.to_owned()).unwrap();
            verifier.verify_server_cert(&leaf.chain[0], &[], &sn, &[], now).is_ok()
        };
        assert!(ok("192.168.18.130"));
        assert!(ok("oguz-mint.local"));
        assert!(ok(SERVER_NAME));
        assert!(!ok("8.8.8.8"), "genel adres sertifikaya girmez");
        assert!(!ok("example.com"));
    }

    #[test]
    fn profile_carries_ca_and_stable_ids() {
        let ca = WebCa::generate(NOW).unwrap();
        let a = ca.mobileconfig("abcd", "Oğuz <Mint>");
        assert!(a.contains("com.apple.security.root"));
        assert!(a.contains(&STANDARD.encode(ca.cert().as_ref())));
        assert!(a.contains("Oğuz &lt;Mint&gt;"));
        assert_eq!(a, ca.mobileconfig("abcd", "Oğuz <Mint>"));
        assert_ne!(a, ca.mobileconfig("efgh", "Oğuz <Mint>"));
    }
}

//! `_loupe._tcp.local.` keşfi (RFC-001B §2.2).
//!
//! TXT kayıtları: `v=1`, `id=<kurulum kimliği>`, `ca=<CA SPKI SHA-256'nın ilk 16
//! baytı, base64url>`. Telefon sabitlediği CA ile eşleşen servisi seçer; aynı
//! ağdaki iki Loupe kurulumu karışmaz. TXT'de kişisel bilgi yoktur.
//!
//! [`Discovery`] trait'i gerçek ağı (mDNS, `mdns` özelliği) ve testlerdeki
//! bellek içi kaydı aynı arayüzle sunar.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use crate::{LanError, ServerIdentity};

pub const SERVICE_TYPE: &str = "_loupe._tcp.local.";

/// Bir Loupe kurulumunun duyurusu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceAd {
    /// Servis örnek adı ("Oğuz-Masaüstü"); ayarlardan anonimleştirilebilir.
    pub instance: String,
    pub port: u16,
    pub addrs: Vec<IpAddr>,
    pub txt: BTreeMap<String, String>,
}

impl ServiceAd {
    #[must_use]
    pub fn for_server(identity: &ServerIdentity, instance: &str, port: u16, addrs: Vec<IpAddr>) -> Self {
        let mut txt = BTreeMap::new();
        txt.insert("v".to_owned(), "1".to_owned());
        txt.insert("id".to_owned(), identity.install_id().to_owned());
        txt.insert("ca".to_owned(), ca_tag(&identity.ca_spki_sha256()));
        Self { instance: instance.to_owned(), port, addrs, txt }
    }

    /// Telefonun sabitlediği CA'ya ait mi?
    #[must_use]
    pub fn matches_ca(&self, ca_spki_sha256: &[u8; 32]) -> bool {
        self.txt.get("ca").is_some_and(|c| *c == ca_tag(ca_spki_sha256))
    }
}

/// TXT `ca` değeri: SPKI özetinin ilk 16 baytı (22 karakter).
#[must_use]
pub fn ca_tag(ca_spki_sha256: &[u8; 32]) -> String {
    URL_SAFE_NO_PAD.encode(&ca_spki_sha256[..16])
}

pub trait Discovery: Send + Sync + std::fmt::Debug {
    fn advertise(&self, ad: &ServiceAd) -> Result<(), LanError>;
    fn withdraw(&self, instance: &str) -> Result<(), LanError>;
    /// Ağdaki Loupe servislerini `timeout` boyunca toplar.
    fn browse(&self, timeout: Duration) -> Result<Vec<ServiceAd>, LanError>;
}

/// Testler için paylaşılan "ağ": klonlar aynı kaydı görür.
#[derive(Debug, Clone, Default)]
pub struct MemoryDiscovery {
    registry: Arc<Mutex<BTreeMap<String, ServiceAd>>>,
}

impl MemoryDiscovery {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, ServiceAd>>, LanError> {
        self.registry.lock().map_err(|_| LanError::Discovery("kayıt kilidi zehirlendi".into()))
    }
}

impl Discovery for MemoryDiscovery {
    fn advertise(&self, ad: &ServiceAd) -> Result<(), LanError> {
        self.lock()?.insert(ad.instance.clone(), ad.clone());
        Ok(())
    }

    fn withdraw(&self, instance: &str) -> Result<(), LanError> {
        self.lock()?.remove(instance);
        Ok(())
    }

    fn browse(&self, _timeout: Duration) -> Result<Vec<ServiceAd>, LanError> {
        Ok(self.lock()?.values().cloned().collect())
    }
}

#[cfg(feature = "mdns")]
pub use mdns::MdnsDiscovery;

#[cfg(feature = "mdns")]
mod mdns {
    use std::collections::BTreeMap;
    use std::time::{Duration, Instant};

    use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

    use super::{Discovery, SERVICE_TYPE, ServiceAd};
    use crate::LanError;

    fn err(e: impl std::fmt::Display) -> LanError {
        LanError::Discovery(e.to_string())
    }

    /// Gerçek ağda mDNS/DNS-SD (`mdns-sd`, saf Rust; Avahi gerekmez).
    pub struct MdnsDiscovery {
        daemon: ServiceDaemon,
        host_name: String,
    }

    impl std::fmt::Debug for MdnsDiscovery {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("MdnsDiscovery").field("host_name", &self.host_name).finish_non_exhaustive()
        }
    }

    impl MdnsDiscovery {
        /// `host_name`: `<ad>.local.` biçiminde makine adı.
        pub fn new(host_name: &str) -> Result<Self, LanError> {
            let mut host = host_name.trim_end_matches('.').to_owned();
            if !host.ends_with(".local") {
                host.push_str(".local");
            }
            host.push('.');
            Ok(Self { daemon: ServiceDaemon::new().map_err(err)?, host_name: host })
        }

        fn fullname(instance: &str) -> String {
            format!("{instance}.{SERVICE_TYPE}")
        }
    }

    impl Discovery for MdnsDiscovery {
        fn advertise(&self, ad: &ServiceAd) -> Result<(), LanError> {
            let props: Vec<(&str, &str)> = ad.txt.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            let info = ServiceInfo::new(
                SERVICE_TYPE,
                &ad.instance,
                &self.host_name,
                ad.addrs.as_slice(),
                ad.port,
                props.as_slice(),
            )
            .map_err(err)?;
            self.daemon.register(info).map_err(err)
        }

        fn withdraw(&self, instance: &str) -> Result<(), LanError> {
            self.daemon.unregister(&Self::fullname(instance)).map(drop).map_err(err)
        }

        fn browse(&self, timeout: Duration) -> Result<Vec<ServiceAd>, LanError> {
            let rx = self.daemon.browse(SERVICE_TYPE).map_err(err)?;
            let deadline = Instant::now() + timeout;
            let mut found: BTreeMap<String, ServiceAd> = BTreeMap::new();
            while let Some(left) = deadline.checked_duration_since(Instant::now()) {
                let Ok(event) = rx.recv_timeout(left) else { break };
                if let ServiceEvent::ServiceResolved(info) = event {
                    let instance = info
                        .get_fullname()
                        .strip_suffix(&format!(".{SERVICE_TYPE}"))
                        .unwrap_or_else(|| info.get_fullname())
                        .to_owned();
                    let txt =
                        info.get_properties().iter().map(|p| (p.key().to_owned(), p.val_str().to_owned())).collect();
                    let mut addrs: Vec<_> = info.get_addresses().iter().copied().collect();
                    addrs.sort();
                    found.insert(instance.clone(), ServiceAd { instance, port: info.get_port(), addrs, txt });
                }
            }
            let _stopped = self.daemon.stop_browse(SERVICE_TYPE);
            Ok(found.into_values().collect())
        }
    }

    impl Drop for MdnsDiscovery {
        fn drop(&mut self) {
            let _stopped = self.daemon.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_discovery_finds_the_install_with_the_pinned_ca() {
        let net = MemoryDiscovery::default();
        let ours = ServerIdentity::generate("Masaüstü", 1_761_390_000_000).unwrap();
        let other = ServerIdentity::generate("Dizüstü", 1_761_390_000_000).unwrap();
        let addr: IpAddr = "192.168.1.20".parse().unwrap();
        net.advertise(&ServiceAd::for_server(&ours, "Masaüstü", 47123, vec![addr])).unwrap();
        net.clone().advertise(&ServiceAd::for_server(&other, "Dizüstü", 47123, vec![addr])).unwrap();

        let seen = net.browse(Duration::from_millis(10)).unwrap();
        assert_eq!(seen.len(), 2);
        let mine: Vec<_> = seen.iter().filter(|s| s.matches_ca(&ours.ca_spki_sha256())).collect();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].instance, "Masaüstü");
        assert_eq!(mine[0].txt.get("id").map(String::as_str), Some(ours.install_id()));
        assert_eq!(mine[0].txt.get("ca").map(String::len), Some(22));

        net.withdraw("Masaüstü").unwrap();
        assert!(net.browse(Duration::ZERO).unwrap().iter().all(|s| !s.matches_ca(&ours.ca_spki_sha256())));
    }
}

//! İstemci tarafı `age` şifrelemesi (RFC-001B §3.2–3.3).
//!
//! Her nesne iki X25519 alıcısına şifrelenir:
//! - **cihaz anahtarı**: bu bilgisayarda (Secret Service), günlük şifreleme ve
//!   doğrulama okuması için;
//! - **kurtarma anahtarı**: bir kez gösterilir, kâğıda yazılır, bilgisayarda
//!   yalnızca açık anahtarı (`age1…`) kalır.
//!
//! Bilgisayar kaybolsa bile arşiv uygulama olmadan da açılır:
//! `age -d -i kurtarma.txt objects/9f/2c/9f2c….age > DSC_0412.NEF`.
//!
//! Akış biçimi standart age v1'dir: ChaCha20-Poly1305, 64 KiB parçalar; her
//! parça ayrı doğrulanır, kesilmiş ya da değiştirilmiş nesne çözülemez.

use std::fmt;
use std::io::{Read, Write};

use age::secrecy::ExposeSecret;
use age::x25519::{Identity, Recipient};
use rand::Rng;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::CloudError;

const SECRET_PREFIX: &str = "AGE-SECRET-KEY-1";
/// Kurtarma anahtarının yazdırılan gruplarının boyu.
pub const GROUP_LEN: usize = 4;

fn crypto_err(e: impl fmt::Display) -> CloudError {
    CloudError::Crypto(e.to_string())
}

/// Şifreleme anahtarları: cihaz kimliği + kurtarma alıcısı.
#[derive(Clone)]
pub struct Keyring {
    device: Identity,
    recovery: Recipient,
}

impl fmt::Debug for Keyring {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Keyring")
            .field("device", &self.device.to_public().to_string())
            .field("recovery", &self.recovery.to_string())
            .finish()
    }
}

impl Keyring {
    #[must_use]
    pub const fn new(device: Identity, recovery: Recipient) -> Self {
        Self { device, recovery }
    }

    /// Yeni cihaz anahtarı; kurtarma alıcısı (`age1…`) verilir.
    pub fn generate(recovery_recipient: &str) -> Result<Self, CloudError> {
        Ok(Self { device: Identity::generate(), recovery: parse_recipient(recovery_recipient)? })
    }

    /// Secret Service'ten okunan `AGE-SECRET-KEY-1…` ile.
    pub fn from_device_secret(device_secret: &str, recovery_recipient: &str) -> Result<Self, CloudError> {
        let device = device_secret.trim().parse::<Identity>().map_err(|e| CloudError::InvalidKey(e.to_string()))?;
        Ok(Self { device, recovery: parse_recipient(recovery_recipient)? })
    }

    /// Secret Service'e yazılacak cihaz anahtarı.
    #[must_use]
    pub fn device_secret(&self) -> Zeroizing<String> {
        Zeroizing::new(self.device.to_string().expose_secret().to_owned())
    }

    #[must_use]
    pub const fn device_identity(&self) -> &Identity {
        &self.device
    }

    #[must_use]
    pub fn recovery_recipient(&self) -> String {
        self.recovery.to_string()
    }

    #[must_use]
    pub fn recipients(&self) -> [Recipient; 2] {
        [self.device.to_public(), self.recovery.clone()]
    }

    /// `sync_status.key_fingerprint`: hangi alıcı çiftiyle şifrelendi (rotasyon).
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let mut h = Sha256::new();
        h.update(self.device.to_public().to_string().as_bytes());
        h.update(b"\n");
        h.update(self.recovery.to_string().as_bytes());
        let d = h.finalize();
        d[..8].iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Çıkışı iki alıcıya şifreleyen akış yazıcısı. `finish()` çağrılmalı.
    pub fn encrypt_to<W: Write>(&self, out: W) -> Result<age::stream::StreamWriter<W>, CloudError> {
        let r = self.recipients();
        let enc = age::Encryptor::with_recipients(r.iter().map(|r| r as &dyn age::Recipient)).map_err(crypto_err)?;
        enc.wrap_output(out).map_err(crypto_err)
    }
}

fn parse_recipient(s: &str) -> Result<Recipient, CloudError> {
    s.trim().parse::<Recipient>().map_err(|e| CloudError::InvalidKey(format!("age alıcısı: {e}")))
}

/// age akışını verilen kimlikle çözen okuyucu.
pub fn decrypt_from<R: Read>(identity: &Identity, input: R) -> Result<age::stream::StreamReader<R>, CloudError> {
    let dec = age::Decryptor::new(input).map_err(crypto_err)?;
    dec.decrypt(std::iter::once(identity as &dyn age::Identity)).map_err(crypto_err)
}

/// İnsan tarafından okunabilir kurtarma anahtarı.
///
/// Biçim standart `AGE-SECRET-KEY-1…` (bech32, sağlama toplamlı): tek harf
/// hatası ayrıştırmada yakalanır. Yazdırırken önek ayrı satırda, gövde 4'lü
/// gruplarda gösterilir; geri yazarken boşluk, tire ve küçük harf yok sayılır.
#[derive(Clone)]
pub struct RecoveryKey {
    identity: Identity,
}

impl fmt::Debug for RecoveryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecoveryKey").field("public", &self.public().to_string()).finish_non_exhaustive()
    }
}

impl RecoveryKey {
    #[must_use]
    pub fn generate() -> Self {
        Self { identity: Identity::generate() }
    }

    /// Kullanıcının yazdığı/okuttuğu anahtar. Bech32 sağlaması doğrulanır.
    pub fn parse(input: &str) -> Result<Self, CloudError> {
        let compact: Zeroizing<String> = Zeroizing::new(
            input.chars().filter(|c| !c.is_whitespace() && *c != '-').flat_map(char::to_uppercase).collect(),
        );
        let body = compact
            .strip_prefix("AGESECRETKEY1")
            .ok_or_else(|| CloudError::InvalidKey("kurtarma anahtarı AGE-SECRET-KEY-1 ile başlamalı".into()))?;
        let full = Zeroizing::new(format!("{SECRET_PREFIX}{body}"));
        let identity =
            full.parse::<Identity>().map_err(|_| CloudError::InvalidKey("kurtarma anahtarı hatalı yazılmış".into()))?;
        Ok(Self { identity })
    }

    #[must_use]
    pub fn public(&self) -> Recipient {
        self.identity.to_public()
    }

    #[must_use]
    pub const fn identity(&self) -> &Identity {
        &self.identity
    }

    /// Standart tek satır (age aracına verilecek dosyanın içeriği).
    #[must_use]
    pub fn secret(&self) -> Zeroizing<String> {
        Zeroizing::new(self.identity.to_string().expose_secret().to_owned())
    }

    /// Önekten sonraki gövde, 4'lü gruplar.
    #[must_use]
    pub fn groups(&self) -> Vec<Zeroizing<String>> {
        let s = self.secret();
        let body = s.strip_prefix(SECRET_PREFIX).unwrap_or(&s);
        body.as_bytes().chunks(GROUP_LEN).map(|c| Zeroizing::new(String::from_utf8_lossy(c).into_owned())).collect()
    }

    /// Yazdırılabilir biçim: `AGE-SECRET-KEY-1` ve altında 4'lü gruplar.
    #[must_use]
    pub fn display(&self) -> Zeroizing<String> {
        let groups: Vec<String> = self.groups().iter().map(|g| g.to_string()).collect();
        Zeroizing::new(format!("{SECRET_PREFIX}\n{}", groups.join(" ")))
    }

    /// "Kaydettiğini kanıtla" adımı: sorulacak `n` farklı grup numarası (0'dan).
    #[must_use]
    pub fn challenge(&self, n: usize) -> Vec<usize> {
        let total = self.groups().len();
        let mut picked: Vec<usize> = Vec::with_capacity(n.min(total));
        let mut rng = rand::rng();
        while picked.len() < n.min(total) {
            let i = rng.random_range(0..total);
            if !picked.contains(&i) {
                picked.push(i);
            }
        }
        picked.sort_unstable();
        picked
    }

    /// Kullanıcının geri yazdığı gruplar doğru mu? Büyük/küçük harf ve boşluk önemsiz.
    #[must_use]
    pub fn verify_challenge(&self, answers: &[(usize, &str)]) -> bool {
        let groups = self.groups();
        !answers.is_empty()
            && answers.iter().all(|(i, a)| {
                let a: String = a.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_uppercase).collect();
                groups.get(*i).is_some_and(|g| g.as_str() == a)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_round_trip_with_device_and_recovery_keys() {
        let recovery = RecoveryKey::generate();
        let keys = Keyring::generate(&recovery.public().to_string()).unwrap();
        let plain: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();

        let mut cipher = Vec::new();
        let mut w = keys.encrypt_to(&mut cipher).unwrap();
        w.write_all(&plain).unwrap();
        w.finish().unwrap();
        assert!(cipher.starts_with(b"age-encryption.org/v1\n"));
        assert!(!cipher.windows(64).any(|w| w == &plain[1000..1064]));

        for id in [keys.device_identity(), recovery.identity()] {
            let mut out = Vec::new();
            decrypt_from(id, cipher.as_slice()).unwrap().read_to_end(&mut out).unwrap();
            assert_eq!(out, plain);
        }
        // Başka anahtar çözemez; kesilmiş nesne çözülmez.
        assert!(decrypt_from(&Identity::generate(), cipher.as_slice()).is_err());
        let mut out = Vec::new();
        let cut = &cipher[..cipher.len() - 100];
        assert!(
            decrypt_from(recovery.identity(), cut)
                .and_then(|mut r| r.read_to_end(&mut out).map_err(crypto_err))
                .is_err()
        );

        let again = Keyring::from_device_secret(&keys.device_secret(), &keys.recovery_recipient()).unwrap();
        assert_eq!(again.fingerprint(), keys.fingerprint());
        assert_eq!(keys.fingerprint().len(), 16);
    }

    #[test]
    fn recovery_key_is_readable_tolerant_and_checksummed() {
        let k = RecoveryKey::generate();
        let shown = k.display();
        assert!(shown.starts_with("AGE-SECRET-KEY-1\n"));
        assert!(k.groups().iter().all(|g| !g.is_empty() && g.len() <= GROUP_LEN));

        // Kâğıttan geri yazma: küçük harf, boşluk, tire serbest.
        let typed = shown.to_lowercase().replace('\n', " - ");
        let back = RecoveryKey::parse(&typed).unwrap();
        assert_eq!(back.public().to_string(), k.public().to_string());

        // Tek karakter hatası bech32 sağlamasına takılır.
        let mut chars: Vec<char> = k.secret().chars().collect();
        let i = chars.len() - 10;
        chars[i] = if chars[i] == 'Q' { 'P' } else { 'Q' };
        let typo: String = chars.into_iter().collect();
        assert!(matches!(RecoveryKey::parse(&typo), Err(CloudError::InvalidKey(_))));
        assert!(RecoveryKey::parse("age1abc").is_err());

        let ask = k.challenge(3);
        assert_eq!(ask.len(), 3);
        let groups = k.groups();
        let right: Vec<(usize, String)> = ask.iter().map(|&i| (i, groups[i].to_lowercase())).collect();
        let right: Vec<(usize, &str)> = right.iter().map(|(i, s)| (*i, s.as_str())).collect();
        assert!(k.verify_challenge(&right));
        assert!(!k.verify_challenge(&[(ask[0], "ZZZZ")]));
        assert!(!k.verify_challenge(&[]));
    }
}

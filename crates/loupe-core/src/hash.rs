use std::fmt;

use crate::CoreError;

/// BLAKE3 içerik hash'i (32 bayt). `media_files.blake3` ile birebir.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Blake3Hash(pub [u8; 32]);

impl Blake3Hash {
    pub fn from_slice(bytes: &[u8]) -> Result<Self, CoreError> {
        <[u8; 32]>::try_from(bytes).map(Self).map_err(|_| CoreError::InvalidHashLength(bytes.len()))
    }

    pub fn from_hex(hex: &str) -> Result<Self, CoreError> {
        let raw = hex.as_bytes();
        if raw.len() != 64 {
            return Err(CoreError::InvalidHashHex(hex.to_owned()));
        }
        let mut out = [0u8; 32];
        for (dst, pair) in out.iter_mut().zip(raw.as_chunks::<2>().0) {
            let hi = hex_digit(pair[0]).ok_or_else(|| CoreError::InvalidHashHex(hex.to_owned()))?;
            let lo = hex_digit(pair[1]).ok_or_else(|| CoreError::InvalidHashHex(hex.to_owned()))?;
            *dst = (hi << 4) | lo;
        }
        Ok(Self(out))
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    #[must_use]
    pub fn to_hex(&self) -> String {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut s = String::with_capacity(64);
        for b in self.0 {
            s.push(char::from(DIGITS[usize::from(b >> 4)]));
            s.push(char::from(DIGITS[usize::from(b & 0x0f)]));
        }
        s
    }
}

fn hex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

impl fmt::Debug for Blake3Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Blake3Hash({})", self.to_hex())
    }
}

impl fmt::Display for Blake3Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// 64-bit DCT algısal hash. SQLite'ta işaretli INTEGER olarak saklanır;
/// bit deseni korunur (`to_db` / `from_db`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PHash(pub u64);

impl PHash {
    #[must_use]
    pub const fn hamming(self, other: Self) -> u32 {
        (self.0 ^ other.0).count_ones()
    }

    #[must_use]
    pub const fn to_db(self) -> i64 {
        i64::from_ne_bytes(self.0.to_ne_bytes())
    }

    #[must_use]
    pub const fn from_db(v: i64) -> Self {
        Self(u64::from_ne_bytes(v.to_ne_bytes()))
    }

    /// Bant indeksleri için 16 bitlik dilim (0..=3). Şemadaki ifadelerle aynı.
    #[must_use]
    pub const fn band(self, i: u32) -> u16 {
        ((self.0 >> (16 * (i & 3))) & 0xffff) as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let h = Blake3Hash([0xab; 32]);
        assert_eq!(Blake3Hash::from_hex(&h.to_hex()), Ok(h));
        assert!(Blake3Hash::from_hex("zz").is_err());
        assert_eq!(Blake3Hash::from_slice(&[0; 31]), Err(CoreError::InvalidHashLength(31)));
    }

    #[test]
    fn phash_db_round_trip_keeps_bits() {
        let p = PHash(0xffff_0000_0000_0001);
        assert!(p.to_db() < 0);
        assert_eq!(PHash::from_db(p.to_db()), p);
        assert_eq!(p.band(3), 0xffff);
        assert_eq!(p.band(0), 1);
        assert_eq!(PHash(0b1011).hamming(PHash(0b0001)), 2);
    }
}

//! L2 normalize gömme vektörleri ve sqlite-vec ikili biçimi (küçük sonlu f32).

use crate::AiError;

/// Birim uzunlukta vektör. Kosinüs benzerliği nokta çarpımıdır.
#[derive(Debug, Clone, PartialEq)]
pub struct Embedding(Vec<f32>);

impl Embedding {
    /// Vektörü L2 normalize eder. Sıfır ya da sonlu olmayan vektör reddedilir.
    pub fn normalized(mut v: Vec<f32>) -> Result<Self, AiError> {
        let norm = v.iter().map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>().sqrt();
        if !norm.is_finite() || norm < 1e-12 {
            return Err(AiError::InvalidInput("sıfır ya da sonlu olmayan vektör".into()));
        }
        #[allow(clippy::cast_possible_truncation)]
        let inv = (1.0 / norm) as f32;
        for x in &mut v {
            *x *= inv;
        }
        Ok(Self(v))
    }

    pub fn expect_dim(self, dim: usize) -> Result<Self, AiError> {
        if self.0.len() == dim { Ok(self) } else { Err(AiError::Dimension { expected: dim, found: self.0.len() }) }
    }

    #[must_use]
    pub fn as_slice(&self) -> &[f32] {
        &self.0
    }

    #[must_use]
    pub fn dim(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn cosine(&self, other: &Self) -> f32 {
        dot(&self.0, &other.0)
    }

    #[must_use]
    pub fn to_blob(&self) -> Vec<u8> {
        to_blob(&self.0)
    }
}

#[must_use]
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Normalize edilmemiş vektörün kosinüs benzerliği (küme merkezi toplam vektördür).
#[must_use]
pub fn cosine_raw(a: &[f32], b: &[f32]) -> f32 {
    let na = dot(a, a).sqrt();
    let nb = dot(b, b).sqrt();
    if na <= f32::EPSILON || nb <= f32::EPSILON { 0.0 } else { dot(a, b) / (na * nb) }
}

#[must_use]
pub fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub fn from_blob(b: &[u8]) -> Result<Vec<f32>, AiError> {
    let (chunks, rest) = b.as_chunks::<4>();
    if !rest.is_empty() {
        return Err(AiError::InvalidInput(format!("vektör blob uzunluğu 4'ün katı değil: {}", b.len())));
    }
    Ok(chunks.iter().map(|c| f32::from_le_bytes(*c)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_and_blob_round_trip() {
        let e = Embedding::normalized(vec![3.0, 4.0]).unwrap();
        assert_eq!(e.as_slice(), &[0.6, 0.8]);
        assert!((e.cosine(&e) - 1.0).abs() < 1e-6);
        assert_eq!(from_blob(&e.to_blob()).unwrap(), e.as_slice());
        assert!(Embedding::normalized(vec![0.0; 4]).is_err());
        assert!(Embedding::normalized(vec![f32::NAN]).is_err());
        assert!(from_blob(&[0, 1, 2]).is_err());
        assert!((cosine_raw(&[2.0, 0.0], &[5.0, 0.0]) - 1.0).abs() < 1e-6);
    }
}

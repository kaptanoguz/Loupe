//! Tek geçişte içerik özeti: dosya bir kez okunur, her parça BLAKE3'e verilir;
//! dosya pHash'i hesaplanabilen bir görüntüyse aynı parçalar belleğe de
//! kopyalanır ve okuma bitince pHash bu tampondan çıkarılır (ikinci disk okuması yok).
//!
//! İçe aktarmada pHash varsayılan olarak kapalıdır: tam çözünürlükte çözmek
//! dosya başına ~200 ms sürer. Boyut başlıktan okunur, pHash `md` küçük resmi
//! üretilince ondan hesaplanır.

use crate::meta;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use image::DynamicImage;
use image::imageops::FilterType;
use loupe_core::{Blake3Hash, PHash, classify};

use crate::StorageError;
use crate::fsutil::drop_page_cache;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HashOptions {
    /// Görüntü dosyalarında pHash hesaplansın mı?
    pub phash: bool,
    /// Bu boyuttan büyük görüntülerin pHash'i atlanır (tampon belleği sınırı).
    pub phash_max_bytes: u64,
    pub buffer_size: usize,
    /// Okuma bitince dosyayı sayfa önbelleğinden at (import kaynakları için).
    pub drop_cache: bool,
}

impl Default for HashOptions {
    fn default() -> Self {
        Self { phash: false, phash_max_bytes: 64 * 1024 * 1024, buffer_size: 1024 * 1024, drop_cache: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileDigest {
    pub blake3: Blake3Hash,
    pub size: u64,
    pub phash: Option<PHash>,
    /// pHash için görüntü çözüldüyse boyutları (EXIF yönlendirmesi uygulanmamış).
    pub dimensions: Option<(u32, u32)>,
    pub mtime_ns: i64,
    pub inode: u64,
    pub device: u64,
}

/// Dosyayı tek geçişte okur. Okuma sırasında dosya değişirse (boyut, mtime veya
/// okunan bayt sayısı tutmazsa) `SourceChanged` döner: yarım yazılmış bir
/// dosyanın hash'i kataloğa girmez.
pub fn hash_file(path: &Path, opts: &HashOptions) -> Result<FileDigest, StorageError> {
    let mut file = File::open(path).map_err(StorageError::io("okuma için açma", path))?;
    let before = file.metadata().map_err(StorageError::io("stat", path))?;
    if !before.is_file() {
        return Err(StorageError::NotAFile(path.to_path_buf()));
    }
    let size = before.len();
    let want_phash = opts.phash && size <= opts.phash_max_bytes && classify(path).decodable_for_phash();
    let mut tee = want_phash.then(|| Vec::with_capacity(usize::try_from(size).unwrap_or(0)));

    let (blake3, read) =
        hash_reader(&mut file, opts.buffer_size, tee.as_mut()).map_err(StorageError::io("okuma", path))?;
    if opts.drop_cache {
        drop_page_cache(&file);
    }
    let after = std::fs::metadata(path).map_err(StorageError::io("stat", path))?;
    if read != size || after.len() != size || meta::mtime_ns(&after) != meta::mtime_ns(&before) {
        return Err(StorageError::SourceChanged(path.to_path_buf()));
    }

    let (phash, dimensions) = match tee.as_deref().and_then(phash_from_bytes) {
        Some((p, d)) => (Some(p), Some(d)),
        None if classify(path).decodable_for_phash() => (None, header_dimensions(path)),
        None => (None, None),
    };
    Ok(FileDigest {
        blake3,
        size,
        phash,
        dimensions,
        mtime_ns: meta::mtime_ns(&before),
        inode: meta::inode(&before),
        device: meta::device(&before),
    })
}

/// Görüntünün boyutu, pikselleri çözmeden (dosya az önce okunduğu için
/// sayfa önbelleğinden gelir).
fn header_dimensions(path: &Path) -> Option<(u32, u32)> {
    image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.into_dimensions().ok()
}

/// Okuyucuyu sonuna kadar BLAKE3'ler; `tee` verilirse baytları ona da ekler.
pub fn hash_reader<R: Read>(
    r: &mut R,
    buffer_size: usize,
    mut tee: Option<&mut Vec<u8>>,
) -> io::Result<(Blake3Hash, u64)> {
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; buffer_size.max(4096)];
    let mut total = 0u64;
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        let chunk = &buf[..n];
        hasher.update(chunk);
        if let Some(t) = tee.as_deref_mut() {
            t.extend_from_slice(chunk);
        }
        total += n as u64;
    }
    Ok((Blake3Hash(*hasher.finalize().as_bytes()), total))
}

/// Bellekteki görüntüden pHash. Çözülemiyorsa `None` (import bunu hata saymaz).
#[must_use]
pub fn phash_from_bytes(bytes: &[u8]) -> Option<(PHash, (u32, u32))> {
    let img = image::load_from_memory(bytes).ok()?;
    Some((phash_image(&img), (img.width(), img.height())))
}

const N: usize = 32;
const K: usize = 8;

/// DCT tabanlı 64-bit algısal hash.
///
/// 32×32 gri tonlamaya küçült → 2B DCT-II → sol üst 8×8 düşük frekans bloğu →
/// her katsayı medyandan büyükse 1. Yeniden sıkıştırma, küçültme ve hafif renk
/// düzeltmesi birkaç biti değiştirir; farklı fotoğraflar ~32 bit farklıdır.
#[must_use]
pub fn phash_image(img: &DynamicImage) -> PHash {
    let small = img.resize_exact(N as u32, N as u32, FilterType::Triangle).to_luma8();
    let px: Vec<f64> = small.pixels().map(|p| f64::from(p.0[0])).collect();

    // cos((2x+1)uπ / 2N), u < K
    let mut cos = [[0f64; N]; K];
    for (u, row) in cos.iter_mut().enumerate() {
        for (x, c) in row.iter_mut().enumerate() {
            *c = (((2 * x + 1) * u) as f64 * std::f64::consts::PI / (2 * N) as f64).cos();
        }
    }
    // Ayrılabilir DCT: önce satırlar (yalnızca ilk K katsayı), sonra sütunlar.
    let mut rows = [[0f64; K]; N];
    for (y, out) in rows.iter_mut().enumerate() {
        for (v, o) in out.iter_mut().enumerate() {
            *o = (0..N).map(|x| px[y * N + x] * cos[v][x]).sum();
        }
    }
    let mut coeffs = [0f64; K * K];
    for u in 0..K {
        for v in 0..K {
            coeffs[u * K + v] = (0..N).map(|y| rows[y][v] * cos[u][y]).sum();
        }
    }

    let mut sorted = coeffs;
    sorted.sort_by(f64::total_cmp);
    let median = (sorted[K * K / 2 - 1] + sorted[K * K / 2]) / 2.0;
    let bits = coeffs.iter().enumerate().fold(0u64, |acc, (i, c)| if *c > median { acc | (1 << i) } else { acc });
    PHash(bits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    fn scene(seed: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(ImageBuffer::from_fn(320, 240, |x, y| {
            let v = ((x * seed + y * 3) % 256) as u8;
            let w = if (x / 40 + y / (30 + seed)).is_multiple_of(2) { 220 } else { 30 };
            Rgb([v, w, ((x + y) % 256) as u8])
        }))
    }

    fn jpeg(img: &DynamicImage, quality: u8) -> Vec<u8> {
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality).encode_image(img).unwrap();
        out
    }

    #[test]
    fn phash_survives_recompression_and_resize() {
        let a = scene(1);
        let (h1, dim) = phash_from_bytes(&jpeg(&a, 95)).unwrap();
        let (h2, _) = phash_from_bytes(&jpeg(&a, 40)).unwrap();
        let (h3, _) = phash_from_bytes(&jpeg(&a.resize(160, 120, FilterType::Lanczos3), 80)).unwrap();
        assert_eq!(dim, (320, 240));
        assert!(h1.hamming(h2) <= 6, "yeniden sıkıştırma: {}", h1.hamming(h2));
        assert!(h1.hamming(h3) <= 8, "küçültme: {}", h1.hamming(h3));
        let (other, _) = phash_from_bytes(&jpeg(&scene(7), 95)).unwrap();
        assert!(h1.hamming(other) >= 16, "farklı sahne: {}", h1.hamming(other));
    }

    #[test]
    fn reader_hash_matches_blake3_and_tees() {
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        let mut tee = Vec::new();
        let (h, n) = hash_reader(&mut data.as_slice(), 64 * 1024, Some(&mut tee)).unwrap();
        assert_eq!(h.0, *blake3::hash(&data).as_bytes());
        assert_eq!((n, tee.len()), (data.len() as u64, data.len()));
    }
}

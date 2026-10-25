//! TIFF tabanlı RAW dosyalarından (CR2, NEF, ARW, DNG, PEF…) gömülü JPEG
//! önizlemeyi çıkarır. Demosaic yapılmaz: kameranın kendi ürettiği önizleme
//! küçük resim için yeterince büyüktür (çoğu gövdede tam çözünürlük).
//!
//! IFD0 zinciri, `SubIFDs` (0x014A) ve EXIF IFD dolaşılır; adaylar
//! `JPEGInterchangeFormat`/`Length` (0x0201/0x0202) ya da sıkıştırması JPEG
//! olan tek şeritli görüntülerdir. Kayıpsız JPEG (SOF3, ham sensör verisi)
//! aday sayılmaz. En büyük geçerli aday seçilir.

use std::collections::HashSet;
use std::path::Path;

use crate::MediaError;

const TAG_COMPRESSION: u16 = 0x0103;
const TAG_STRIP_OFFSETS: u16 = 0x0111;
const TAG_STRIP_BYTE_COUNTS: u16 = 0x0117;
const TAG_SUB_IFDS: u16 = 0x014A;
const TAG_JPEG_OFFSET: u16 = 0x0201;
const TAG_JPEG_LENGTH: u16 = 0x0202;
const TAG_EXIF_IFD: u16 = 0x8769;

const MAX_IFDS: usize = 64;
const MAX_ENTRIES: u16 = 1024;

/// Dosyadaki en büyük gömülü JPEG'i döndürür.
pub fn extract_preview(path: &Path) -> Result<Vec<u8>, MediaError> {
    let data = std::fs::read(path).map_err(MediaError::io("RAW okuma", path))?;
    find_preview(&data).map(<[u8]>::to_vec).ok_or_else(|| MediaError::NoEmbeddedPreview(path.to_path_buf()))
}

/// Bellekteki TIFF kabında en büyük gömülü JPEG.
#[must_use]
pub fn find_preview(data: &[u8]) -> Option<&[u8]> {
    let tiff = Tiff::new(data)?;
    let first = tiff.u32(4)?;
    let mut queue = vec![first];
    let mut seen = HashSet::new();
    let mut best: Option<&[u8]> = None;

    while let Some(ifd) = queue.pop() {
        if ifd == 0 || seen.len() >= MAX_IFDS || !seen.insert(ifd) {
            continue;
        }
        let Some(entries) = tiff.entries(ifd) else { continue };
        let mut jpeg_off = None;
        let mut jpeg_len = None;
        let mut compression = None;
        let mut strips = None;
        let mut strip_counts = None;
        for e in &entries {
            match e.tag {
                TAG_JPEG_OFFSET => jpeg_off = tiff.values(e).and_then(|v| v.first().copied()),
                TAG_JPEG_LENGTH => jpeg_len = tiff.values(e).and_then(|v| v.first().copied()),
                TAG_COMPRESSION => compression = tiff.values(e).and_then(|v| v.first().copied()),
                TAG_STRIP_OFFSETS => strips = tiff.values(e),
                TAG_STRIP_BYTE_COUNTS => strip_counts = tiff.values(e),
                TAG_SUB_IFDS | TAG_EXIF_IFD => queue.extend(tiff.values(e).unwrap_or_default()),
                _ => {}
            }
        }
        let mut consider = |off: Option<u32>, len: Option<u32>| {
            if let Some(blob) = off.zip(len).and_then(|(o, l)| tiff.slice(o, l)).filter(|b| is_baseline_jpeg(b))
                && best.is_none_or(|b| blob.len() > b.len())
            {
                best = Some(blob);
            }
        };
        consider(jpeg_off, jpeg_len);
        // CR2 IFD0, DNG önizleme alt IFD'leri: JPEG sıkıştırmalı tek şerit.
        if matches!(compression, Some(6 | 7))
            && let (Some([o]), Some([l])) = (strips.as_deref(), strip_counts.as_deref())
        {
            consider(Some(*o), Some(*l));
        }
        if let Some(next) = tiff.next_ifd(ifd) {
            queue.push(next);
        }
    }
    best
}

/// SOI ile başlar ve kayıplı bir SOF (0xC0–0xC2) içerir mi? SOF3 (kayıpsız,
/// ham sensör verisi) önizleme değildir.
fn is_baseline_jpeg(b: &[u8]) -> bool {
    if b.len() < 4 || b[0] != 0xFF || b[1] != 0xD8 {
        return false;
    }
    let mut i = 2;
    while let (Some(&ff), Some(&marker)) = (b.get(i), b.get(i + 1)) {
        if ff != 0xFF {
            return false;
        }
        match marker {
            0xC0..=0xC2 => return true,
            0xC3 | 0xD9 | 0xDA => return false,
            0x01 | 0xD0..=0xD7 | 0xFF => i += if marker == 0xFF { 1 } else { 2 },
            _ => {
                let Some(len) = b.get(i + 2..i + 4).map(|s| usize::from(u16::from_be_bytes([s[0], s[1]]))) else {
                    return false;
                };
                i += 2 + len;
            }
        }
    }
    false
}

struct Entry {
    tag: u16,
    typ: u16,
    count: u32,
    /// Girdinin değer alanının dosyadaki konumu (satır içi değer ya da ofset).
    value_pos: usize,
}

struct Tiff<'a> {
    data: &'a [u8],
    le: bool,
}

impl<'a> Tiff<'a> {
    fn new(data: &'a [u8]) -> Option<Self> {
        let le = match data.get(..4)? {
            [b'I', b'I', 42, 0] => true,
            [b'M', b'M', 0, 42] => false,
            _ => return None,
        };
        Some(Self { data, le })
    }

    fn u16(&self, pos: usize) -> Option<u16> {
        let b: [u8; 2] = self.data.get(pos..pos + 2)?.try_into().ok()?;
        Some(if self.le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
    }

    fn u32(&self, pos: usize) -> Option<u32> {
        let b: [u8; 4] = self.data.get(pos..pos + 4)?.try_into().ok()?;
        Some(if self.le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    }

    fn slice(&self, off: u32, len: u32) -> Option<&'a [u8]> {
        let start = usize::try_from(off).ok()?;
        let end = start.checked_add(usize::try_from(len).ok()?)?;
        self.data.get(start..end)
    }

    fn entries(&self, ifd: u32) -> Option<Vec<Entry>> {
        let base = usize::try_from(ifd).ok()?;
        let n = self.u16(base)?;
        if n > MAX_ENTRIES {
            return None;
        }
        (0..usize::from(n))
            .map(|i| {
                let p = base + 2 + i * 12;
                Some(Entry { tag: self.u16(p)?, typ: self.u16(p + 2)?, count: self.u32(p + 4)?, value_pos: p + 8 })
            })
            .collect()
    }

    fn next_ifd(&self, ifd: u32) -> Option<u32> {
        let base = usize::try_from(ifd).ok()?;
        let n = usize::from(self.u16(base)?);
        self.u32(base + 2 + n * 12).filter(|&o| o != 0)
    }

    /// SHORT / LONG / IFD türündeki değerler. 4 bayta sığan değerler satır içidir.
    fn values(&self, e: &Entry) -> Option<Vec<u32>> {
        let size = match e.typ {
            3 => 2,
            4 | 13 => 4,
            _ => return None,
        };
        let count = usize::try_from(e.count).ok().filter(|&c| c <= 4096)?;
        let total = count.checked_mul(size)?;
        let start = if total <= 4 { e.value_pos } else { usize::try_from(self.u32(e.value_pos)?).ok()? };
        (0..count)
            .map(|i| {
                let p = start + i * size;
                if size == 2 { self.u16(p).map(u32::from) } else { self.u32(p) }
            })
            .collect()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Küçük bir TIFF/NEF benzeri kap: IFD0 → küçük JPEG (0x0201), SubIFD →
    /// büyük JPEG, ikinci SubIFD → kayıpsız (SOF3) "ham" veri.
    pub(crate) fn synthetic_raw(small: &[u8], large: &[u8]) -> Vec<u8> {
        // Düzen: başlık 0..8, IFD0 8..50 (3 girdi), SubIFD ofset dizisi 50..58,
        // SubIFD1 58..88, SubIFD2 88..130, veriler 130'dan itibaren.
        const IFD0: u32 = 8;
        const SUB_ARRAY: u32 = 50;
        const SUB1: u32 = 58;
        const SUB2: u32 = 88;
        const DATA: u32 = 130;
        let raw_lossless: &[u8] = &[0xFF, 0xD8, 0xFF, 0xC3, 0x00, 0x02, 0xFF, 0xD9];
        let len = |b: &[u8]| u32::try_from(b.len()).unwrap();
        let small_off = DATA;
        let large_off = small_off + len(small);
        let raw_off = large_off + len(large);

        let mut out = b"II*\0".to_vec();
        out.extend_from_slice(&IFD0.to_le_bytes());
        let entry = |out: &mut Vec<u8>, tag: u16, typ: u16, count: u32, val: u32| {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&typ.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            out.extend_from_slice(&val.to_le_bytes());
        };
        out.extend_from_slice(&3u16.to_le_bytes());
        entry(&mut out, TAG_SUB_IFDS, 13, 2, SUB_ARRAY);
        entry(&mut out, TAG_JPEG_OFFSET, 4, 1, small_off);
        entry(&mut out, TAG_JPEG_LENGTH, 4, 1, len(small));
        out.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(out.len(), SUB_ARRAY as usize);
        out.extend_from_slice(&SUB1.to_le_bytes());
        out.extend_from_slice(&SUB2.to_le_bytes());

        out.extend_from_slice(&2u16.to_le_bytes());
        entry(&mut out, TAG_JPEG_OFFSET, 4, 1, large_off);
        entry(&mut out, TAG_JPEG_LENGTH, 4, 1, len(large));
        out.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(out.len(), SUB2 as usize);

        // Kayıpsız JPEG sıkıştırmalı ham şerit: aday olmamalı.
        out.extend_from_slice(&3u16.to_le_bytes());
        entry(&mut out, TAG_COMPRESSION, 3, 1, 7);
        entry(&mut out, TAG_STRIP_OFFSETS, 4, 1, raw_off);
        entry(&mut out, TAG_STRIP_BYTE_COUNTS, 4, 1, len(raw_lossless));
        out.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(out.len(), DATA as usize);

        out.extend_from_slice(small);
        out.extend_from_slice(large);
        out.extend_from_slice(raw_lossless);
        out
    }

    fn fake_jpeg(payload: usize) -> Vec<u8> {
        let mut j = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x02];
        j.extend(std::iter::repeat_n(0xAB, payload));
        j.extend_from_slice(&[0xFF, 0xD9]);
        j
    }

    #[test]
    fn picks_largest_baseline_preview() {
        let small = fake_jpeg(10);
        let large = fake_jpeg(200);
        let file = synthetic_raw(&small, &large);
        assert_eq!(find_preview(&file), Some(large.as_slice()));
    }

    #[test]
    fn rejects_garbage_without_panicking() {
        assert_eq!(find_preview(b"not a tiff"), None);
        let mut file = synthetic_raw(&fake_jpeg(4), &fake_jpeg(8));
        // IFD0 girdi sayısını devasa yap: sınır kontrolü devreye girmeli.
        file[8] = 0xFF;
        file[9] = 0xFF;
        assert_eq!(find_preview(&file), None);
        // Kendine işaret eden IFD döngüsü.
        let mut looped = b"II*\0\x08\0\0\0\0\0\x08\0\0\0".to_vec();
        looped.resize(32, 0);
        assert_eq!(find_preview(&looped), None);
        assert!(!is_baseline_jpeg(&[0xFF, 0xD8, 0xFF, 0xC3, 0, 2]));
    }
}

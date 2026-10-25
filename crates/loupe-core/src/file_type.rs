//! Uzantıdan dosya türü, RAW biçimi ve MIME. İçerik koklama `loupe-meta`'da
//! yapılacak; burada yalnızca import sırasında satır yazmaya yetecek kadarı var.

use std::path::Path;

use crate::MediaFileType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileKind {
    pub file_type: MediaFileType,
    /// `file_type = raw` ise zorunlu (şemadaki CHECK).
    pub raw_format: Option<&'static str>,
    pub mime: &'static str,
}

impl FileKind {
    /// pHash için tek geçişte çözülebilen biçim mi?
    #[must_use]
    pub const fn decodable_for_phash(&self) -> bool {
        matches!(self.file_type, MediaFileType::Jpeg | MediaFileType::Png)
    }

    /// Fotoğraf veya video mu? (İzleyici ve tarayıcı yalnızca bunları ele alır.)
    #[must_use]
    pub const fn is_media(&self) -> bool {
        !matches!(self.file_type, MediaFileType::Other)
    }
}

const RAW: &[(&str, &str)] = &[
    ("nef", "NEF"),
    ("nrw", "NRW"),
    ("cr2", "CR2"),
    ("cr3", "CR3"),
    ("crw", "CRW"),
    ("arw", "ARW"),
    ("srf", "SRF"),
    ("sr2", "SR2"),
    ("raf", "RAF"),
    ("orf", "ORF"),
    ("rw2", "RW2"),
    ("pef", "PEF"),
    ("srw", "SRW"),
    ("x3f", "X3F"),
    ("3fr", "3FR"),
    ("iiq", "IIQ"),
    ("rwl", "RWL"),
];

#[must_use]
pub fn classify(path: &Path) -> FileKind {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    let simple = |file_type, mime| FileKind { file_type, raw_format: None, mime };
    match ext.as_str() {
        "jpg" | "jpeg" | "jpe" => simple(MediaFileType::Jpeg, "image/jpeg"),
        "heic" | "heif" | "hif" => simple(MediaFileType::Heic, "image/heic"),
        "avif" => simple(MediaFileType::Avif, "image/avif"),
        "png" => simple(MediaFileType::Png, "image/png"),
        "tif" | "tiff" => simple(MediaFileType::Tiff, "image/tiff"),
        "webp" => simple(MediaFileType::Webp, "image/webp"),
        "jxl" => simple(MediaFileType::Jxl, "image/jxl"),
        "dng" => simple(MediaFileType::Dng, "image/x-adobe-dng"),
        "mp4" | "m4v" => simple(MediaFileType::Video, "video/mp4"),
        "mov" => simple(MediaFileType::Video, "video/quicktime"),
        "mkv" => simple(MediaFileType::Video, "video/x-matroska"),
        "webm" => simple(MediaFileType::Video, "video/webm"),
        "avi" => simple(MediaFileType::Video, "video/x-msvideo"),
        "mts" | "m2ts" => simple(MediaFileType::Video, "video/mp2t"),
        "3gp" => simple(MediaFileType::Video, "video/3gpp"),
        other => match RAW.iter().find(|(e, _)| *e == other) {
            Some((_, fmt)) => FileKind { file_type: MediaFileType::Raw, raw_format: Some(fmt), mime: "image/x-raw" },
            None => simple(MediaFileType::Other, "application/octet-stream"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_types() {
        assert_eq!(classify(Path::new("a/B.JPG")).file_type, MediaFileType::Jpeg);
        let nef = classify(Path::new("DSC_0412.NEF"));
        assert_eq!((nef.file_type, nef.raw_format), (MediaFileType::Raw, Some("NEF")));
        assert_eq!(classify(Path::new("x.txt")).file_type, MediaFileType::Other);
        assert_eq!(classify(Path::new("noext")).file_type, MediaFileType::Other);
    }
}

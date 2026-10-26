//! Sidecar dosyaları: XMP (Lightroom, darktable, digiKam ile ortak) ve
//! Loupe'un kendi `.loupe.json` dosyası. Orijinal medya dosyasına asla yazılmaz.
//!
//! Yönetilen alanlar: puan (`xmp:Rating`, 0–5), renk etiketi (`xmp:Label`) ve
//! etiketler (`dc:subject`). XMP güncellenirken yalnızca bu üç özellik
//! değiştirilir; başka araçların yazdığı her şey (darktable geçmişi, Lightroom
//! ayarları, boşluklar, yorumlar) bayt bayt korunur. JSON sidecar'daki
//! bilinmeyen alanlar da korunur.
//!
//! Adlandırma: `IMG_0001.CR2.xmp` (darktable/digiKam) ve `IMG_0001.xmp`
//! (Lightroom) okunur; yazarken var olan dosya, yoksa `IMG_0001.CR2.xmp`
//! kullanılır. JSON her zaman `IMG_0001.CR2.loupe.json`.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use loupe_core::ColorLabel;
use quick_xml::NsReader;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};
use serde::{Deserialize, Serialize};

use crate::MediaError;
use crate::util::write_atomic;

const NS_RDF: &[u8] = b"http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const NS_XMP: &[u8] = b"http://ns.adobe.com/xap/1.0/";
const NS_DC: &[u8] = b"http://purl.org/dc/elements/1.1/";

/// Sidecar'larda taşınan kullanıcı verisi.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SidecarData {
    pub rating: Option<u8>,
    pub color_label: Option<ColorLabel>,
    pub tags: Vec<String>,
    /// Favori. XMP'de standart karşılığı yok; yalnızca `.loupe.json`'a yazılır.
    pub favorite: Option<bool>,
}

/// Bir medya dosyasının sidecar çifti.
#[derive(Debug, Clone)]
pub struct Sidecar {
    media: PathBuf,
}

impl Sidecar {
    #[must_use]
    pub fn for_media(media: &Path) -> Self {
        Self { media: media.to_path_buf() }
    }

    fn with_suffix(&self, suffix: &str) -> PathBuf {
        let mut s = self.media.clone().into_os_string();
        s.push(suffix);
        PathBuf::from(s)
    }

    /// Okunacak XMP adayları, öncelik sırasıyla.
    fn xmp_candidates(&self) -> [PathBuf; 2] {
        [self.with_suffix(".xmp"), self.media.with_extension("xmp")]
    }

    /// Yazılacak XMP yolu: var olan sidecar, yoksa `ad.uzantı.xmp`.
    #[must_use]
    pub fn xmp_path(&self) -> PathBuf {
        let [full, short] = self.xmp_candidates();
        if !full.is_file() && short.is_file() && short != self.media { short } else { full }
    }

    #[must_use]
    pub fn json_path(&self) -> PathBuf {
        self.with_suffix(".loupe.json")
    }

    /// İki sidecar'ı birleştirerek okur; ikisi de yoksa `None`. Aynı alan iki
    /// dosyada da varsa `.loupe.json` kazanır (Loupe'un kendi kaydı).
    pub fn read(&self) -> Result<Option<SidecarData>, MediaError> {
        let xmp_path = self.xmp_candidates().into_iter().find(|p| p.is_file() && *p != self.media);
        let xmp = match &xmp_path {
            Some(p) => {
                let bytes = std::fs::read(p).map_err(MediaError::io("XMP okuma", p))?;
                Some(parse_xmp(&bytes).map_err(|reason| MediaError::Sidecar { path: p.clone(), reason })?)
            }
            None => None,
        };
        let json_path = self.json_path();
        let json = match std::fs::read(&json_path) {
            Ok(bytes) => Some(parse_json(&bytes).map_err(|reason| MediaError::Sidecar { path: json_path, reason })?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(MediaError::io("JSON sidecar okuma", &json_path)(e)),
        };
        Ok(match (xmp, json) {
            (None, None) => None,
            (Some(x), None) => Some(x),
            (x, Some(j)) => {
                let x = x.unwrap_or_default();
                Some(SidecarData {
                    rating: j.rating.or(x.rating),
                    color_label: j.color_label.or(x.color_label),
                    tags: j.tags.unwrap_or(x.tags),
                    favorite: j.favorite,
                })
            }
        })
    }

    /// Her iki sidecar'ı atomik olarak yazar; yabancı içerik korunur.
    pub fn write(&self, data: &SidecarData) -> Result<(), MediaError> {
        let xmp_path = self.xmp_path();
        let json_path = self.json_path();
        for p in [&xmp_path, &json_path] {
            if *p == self.media {
                return Err(MediaError::Sidecar {
                    path: p.clone(),
                    reason: "sidecar yolu orijinal dosyayla aynı".into(),
                });
            }
        }

        let existing = read_optional(&xmp_path)?;
        let xmp = update_xmp(existing.as_deref(), data)
            .map_err(|reason| MediaError::Sidecar { path: xmp_path.clone(), reason })?;
        write_atomic(&xmp_path, &xmp)?;

        let mut doc = match read_optional(&json_path)? {
            Some(bytes) => serde_json::from_slice::<JsonSidecar>(&bytes)
                .map_err(|e| MediaError::Sidecar { path: json_path.clone(), reason: e.to_string() })?,
            None => JsonSidecar::default(),
        };
        doc.version = JSON_VERSION;
        doc.rating = data.rating;
        doc.color_label = data.color_label.map(|c| c.as_str().to_owned());
        doc.tags = Some(data.tags.clone());
        doc.favorite = data.favorite;
        let mut json = serde_json::to_vec_pretty(&doc).map_err(|e| MediaError::Encode(e.to_string()))?;
        json.push(b'\n');
        write_atomic(&json_path, &json)
    }
    /// `.loupe.json`'daki `edits` alanını yazar (`None`: siler); diğer alanlar
    /// ve XMP'ye dokunulmaz. Düzenleme tarifi böylece katalog olmadan da okunur.
    pub fn write_edits(&self, edits: Option<serde_json::Value>) -> Result<(), MediaError> {
        let json_path = self.json_path();
        if json_path == self.media {
            return Err(MediaError::Sidecar { path: json_path, reason: "sidecar yolu orijinal dosyayla aynı".into() });
        }
        let existing = read_optional(&json_path)?;
        if existing.is_none() && edits.is_none() {
            return Ok(());
        }
        let mut doc = match existing {
            Some(bytes) => serde_json::from_slice::<JsonSidecar>(&bytes)
                .map_err(|e| MediaError::Sidecar { path: json_path.clone(), reason: e.to_string() })?,
            None => JsonSidecar::default(),
        };
        doc.version = JSON_VERSION;
        match edits {
            Some(v) => doc.extra.insert("edits".into(), v),
            None => doc.extra.remove("edits"),
        };
        let mut json = serde_json::to_vec_pretty(&doc).map_err(|e| MediaError::Encode(e.to_string()))?;
        json.push(b'\n');
        write_atomic(&json_path, &json)
    }
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, MediaError> {
    match std::fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(MediaError::io("sidecar okuma", path)(e)),
    }
}

// ───────────────────────────── .loupe.json ─────────────────────────────

const JSON_VERSION: u32 = 1;

#[derive(Debug, Default, Serialize, Deserialize)]
struct JsonSidecar {
    #[serde(default)]
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rating: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    color_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tags: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    favorite: Option<bool>,
    /// İleride eklenecek alanlar (düzenleme yığını vb.) okunup aynen yazılır.
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

struct JsonFields {
    rating: Option<u8>,
    color_label: Option<ColorLabel>,
    tags: Option<Vec<String>>,
    favorite: Option<bool>,
}

fn parse_json(bytes: &[u8]) -> Result<JsonFields, String> {
    let doc: JsonSidecar = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if doc.version > JSON_VERSION {
        return Err(format!("sürüm {} bu Loupe sürümünden yeni", doc.version));
    }
    let rating = doc.rating.map(|r| check_rating(i64::from(r))).transpose()?;
    let color_label =
        doc.color_label.as_deref().map(|s| ColorLabel::from_str(s).map_err(|e| e.to_string())).transpose()?;
    Ok(JsonFields { rating, color_label, tags: doc.tags, favorite: doc.favorite })
}

fn check_rating(r: i64) -> Result<u8, String> {
    u8::try_from(r).ok().filter(|r| *r <= 5).ok_or_else(|| format!("puan 0–5 aralığında değil: {r}"))
}

// ───────────────────────────────── XMP ─────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Managed {
    Rating,
    Label,
    Subject,
}

/// Çözülmüş ad alanı; okuyucu ödünç alınmadan saklanabilsin diye sahipli.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ns {
    Rdf,
    Xmp,
    Dc,
    Other,
}

fn ns_of(r: &ResolveResult<'_>) -> Ns {
    match r {
        ResolveResult::Bound(Namespace(n)) if *n == NS_RDF => Ns::Rdf,
        ResolveResult::Bound(Namespace(n)) if *n == NS_XMP => Ns::Xmp,
        ResolveResult::Bound(Namespace(n)) if *n == NS_DC => Ns::Dc,
        _ => Ns::Other,
    }
}

fn managed(ns: Ns, local: &[u8]) -> Option<Managed> {
    match (ns, local) {
        (Ns::Xmp, b"Rating") => Some(Managed::Rating),
        (Ns::Xmp, b"Label") => Some(Managed::Label),
        (Ns::Dc, b"subject") => Some(Managed::Subject),
        _ => None,
    }
}

/// XMP rating: `-1` "reddedildi" demektir; Loupe'ta karşılığı puansızlıktır.
fn xmp_rating(text: &str) -> Result<Option<u8>, String> {
    let t = text.trim();
    let v: f64 = t.parse().map_err(|_| format!("geçersiz xmp:Rating: {t:?}"))?;
    if v < 0.0 {
        return Ok(None);
    }
    check_rating(v.round() as i64).map(Some)
}

/// Lightroom `Red`, darktable/digiKam küçük harf yazabilir.
fn xmp_label(text: &str) -> Option<ColorLabel> {
    ColorLabel::from_str(&text.trim().to_ascii_lowercase()).ok()
}

fn push_text(buf: &mut String, ev: &Event<'_>) -> Result<(), String> {
    match ev {
        Event::Text(t) => buf.push_str(&t.xml_content().map_err(|e| e.to_string())?),
        Event::CData(t) => buf.push_str(&t.decode().map_err(|e| e.to_string())?),
        Event::GeneralRef(r) => {
            if let Some(c) = r.resolve_char_ref().map_err(|e| e.to_string())? {
                buf.push(c);
            } else {
                let name = r.decode().map_err(|e| e.to_string())?;
                let resolved = quick_xml::escape::resolve_predefined_entity(&name)
                    .ok_or_else(|| format!("bilinmeyen varlık &{name};"))?;
                buf.push_str(resolved);
            }
        }
        _ => {}
    }
    Ok(())
}

/// XMP paketinden yönetilen alanları okur.
pub fn parse_xmp(bytes: &[u8]) -> Result<SidecarData, String> {
    let mut r = NsReader::from_reader(bytes);
    let mut out = SidecarData::default();
    let mut field: Option<(Managed, String)> = None;
    let mut subject_depth = 0usize;
    let mut li: Option<String> = None;

    loop {
        let (ns, ev) = r.read_resolved_event().map_err(|e| e.to_string())?;
        let ns = ns_of(&ns);
        match &ev {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) => {
                let local = e.local_name();
                if ns == Ns::Rdf && local.as_ref() == b"Description" {
                    for a in e.attributes() {
                        let a = a.map_err(|e| e.to_string())?;
                        let (ans, alocal) = r.resolve_attribute(a.key);
                        let value = a.unescape_value().map_err(|e| e.to_string())?;
                        match managed(ns_of(&ans), alocal.as_ref()) {
                            Some(Managed::Rating) => out.rating = xmp_rating(&value)?,
                            Some(Managed::Label) => out.color_label = xmp_label(&value),
                            _ => {}
                        }
                    }
                }
                let is_start = matches!(ev, Event::Start(_));
                if subject_depth > 0 {
                    if is_start {
                        subject_depth += 1;
                        if ns == Ns::Rdf && local.as_ref() == b"li" {
                            li = Some(String::new());
                        }
                    }
                } else if is_start {
                    match managed(ns, local.as_ref()) {
                        Some(Managed::Subject) => subject_depth = 1,
                        Some(m) => field = Some((m, String::new())),
                        None => {}
                    }
                }
            }
            Event::End(_) => {
                if subject_depth > 0 {
                    subject_depth -= 1;
                    if let Some(tag) = li.take().map(|t| t.trim().to_owned()).filter(|t| !t.is_empty())
                        && !out.tags.contains(&tag)
                    {
                        out.tags.push(tag);
                    }
                } else if let Some((m, text)) = field.take() {
                    match m {
                        Managed::Rating => out.rating = xmp_rating(&text)?,
                        Managed::Label => out.color_label = xmp_label(&text),
                        Managed::Subject => {}
                    }
                }
            }
            other => {
                if let Some(buf) = li.as_mut().or(field.as_mut().map(|(_, t)| t)) {
                    push_text(buf, other)?;
                }
            }
        }
    }
    Ok(out)
}

const XMP_TEMPLATE: &str = concat!(
    "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n",
    "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Loupe\">\n",
    " <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n",
    "  <rdf:Description rdf:about=\"\"\n",
    "    xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n",
    "    xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n",
    "  </rdf:Description>\n",
    " </rdf:RDF>\n",
    "</x:xmpmeta>\n",
    "<?xpacket end=\"w\"?>\n",
);

/// Var olan XMP'yi (yoksa boş şablonu) günceller. Yönetilen özellikler her
/// yerden (öznitelik ya da öğe) silinir ve ilk `rdf:Description` içine öğe
/// olarak yeniden yazılır; diğer tüm baytlar olduğu gibi kopyalanır.
pub fn update_xmp(existing: Option<&[u8]>, data: &SidecarData) -> Result<Vec<u8>, String> {
    let src = existing.unwrap_or(XMP_TEMPLATE.as_bytes());
    let mut r = NsReader::from_reader(src);
    let mut out = Vec::with_capacity(src.len() + 256);
    let mut skip_depth = 0usize;
    let mut inserted = false;
    // Silinen öğenin önündeki girinti de silinsin ki tekrar yazımlar boşluk biriktirmesin.
    let mut pending_ws: &[u8] = &[];

    loop {
        let start = pos(&r)?;
        let (ns, ev) = r.read_resolved_event().map_err(|e| e.to_string())?;
        let ns = ns_of(&ns);
        let end = pos(&r)?;
        let raw = src.get(start..end).ok_or("XMP konum hatası")?;
        if skip_depth > 0 {
            match ev {
                Event::Start(_) => skip_depth += 1,
                Event::End(_) => skip_depth -= 1,
                Event::Eof => return Err("XMP beklenmedik biçimde bitti".into()),
                _ => {}
            }
            continue;
        }
        if matches!(ev, Event::Text(_)) && raw.iter().all(u8::is_ascii_whitespace) {
            out.extend_from_slice(pending_ws);
            pending_ws = raw;
            continue;
        }
        let ws = std::mem::take(&mut pending_ws);
        match &ev {
            Event::Eof => {
                out.extend_from_slice(ws);
                break;
            }
            Event::Start(e) | Event::Empty(e) => {
                let is_start = matches!(ev, Event::Start(_));
                let local = e.local_name();
                if managed(ns, local.as_ref()).is_some() {
                    if is_start {
                        skip_depth = 1;
                    }
                    continue;
                }
                out.extend_from_slice(ws);
                let is_desc = ns == Ns::Rdf && local.as_ref() == b"Description";
                let insert_here = is_desc && !inserted;
                let rebuilt = rebuild_start(&r, e, insert_here)?;
                match rebuilt {
                    None => out.extend_from_slice(raw),
                    Some(tag) => {
                        out.push(b'<');
                        out.extend_from_slice(&tag);
                        out.extend_from_slice(if is_start || insert_here { b">" } else { b"/>" });
                    }
                }
                if insert_here {
                    let rdf = e.name().prefix().map(|p| format!("{}:", String::from_utf8_lossy(p.as_ref())));
                    write_props(&mut out, rdf.as_deref().unwrap_or(""), data);
                    if !is_start {
                        out.extend_from_slice(b"</");
                        out.extend_from_slice(e.name().as_ref());
                        out.push(b'>');
                    }
                    inserted = true;
                }
            }
            _ => {
                out.extend_from_slice(ws);
                out.extend_from_slice(raw);
            }
        }
    }
    if !inserted {
        return Err("rdf:Description bulunamadı".into());
    }
    Ok(out)
}

fn pos<R>(r: &NsReader<R>) -> Result<usize, String> {
    usize::try_from(r.buffer_position()).map_err(|e| e.to_string())
}

/// Yönetilen öznitelik varsa ya da ad alanı bildirimi eklenecekse etiketi
/// yeniden kurar (`<` ve `>` olmadan); gerek yoksa `None`.
fn rebuild_start<R>(r: &NsReader<R>, e: &BytesStart<'_>, declare: bool) -> Result<Option<Vec<u8>>, String> {
    let mut keep: Vec<(Vec<u8>, Cow<'_, [u8]>)> = Vec::new();
    let mut dropped = false;
    let (mut has_xmp, mut has_dc) = (false, false);
    for a in e.attributes() {
        let a = a.map_err(|e| e.to_string())?;
        let (ans, alocal) = r.resolve_attribute(a.key);
        if managed(ns_of(&ans), alocal.as_ref()).is_some() {
            dropped = true;
            continue;
        }
        has_xmp |= a.key.as_ref() == b"xmlns:xmp";
        has_dc |= a.key.as_ref() == b"xmlns:dc";
        keep.push((a.key.as_ref().to_vec(), a.value));
    }
    let add_xmp = declare && !has_xmp;
    let add_dc = declare && !has_dc;
    if !dropped && !add_xmp && !add_dc {
        return Ok(None);
    }
    let mut tag = e.name().as_ref().to_vec();
    for (k, v) in keep {
        tag.push(b' ');
        tag.extend_from_slice(&k);
        // Tek tırnakla yazılmış değer çift tırnak içerebilir.
        let q = if v.contains(&b'"') { b'\'' } else { b'"' };
        tag.push(b'=');
        tag.push(q);
        tag.extend_from_slice(&v);
        tag.push(q);
    }
    if add_xmp {
        tag.extend_from_slice(b" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"");
    }
    if add_dc {
        tag.extend_from_slice(b" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"");
    }
    Ok(Some(tag))
}

fn write_props(out: &mut Vec<u8>, rdf: &str, data: &SidecarData) {
    use std::fmt::Write as _;
    let mut s = String::new();
    if let Some(r) = data.rating {
        let _ = write!(s, "\n   <xmp:Rating>{r}</xmp:Rating>");
    }
    if let Some(l) = data.color_label {
        let _ = write!(s, "\n   <xmp:Label>{}</xmp:Label>", label_title(l));
    }
    if !data.tags.is_empty() {
        let _ = write!(s, "\n   <dc:subject>\n    <{rdf}Bag>");
        for t in &data.tags {
            let _ = write!(s, "\n     <{rdf}li>{}</{rdf}li>", quick_xml::escape::escape(t.as_str()));
        }
        let _ = write!(s, "\n    </{rdf}Bag>\n   </dc:subject>");
    }
    out.extend_from_slice(s.as_bytes());
}

const fn label_title(l: ColorLabel) -> &'static str {
    match l {
        ColorLabel::Red => "Red",
        ColorLabel::Yellow => "Yellow",
        ColorLabel::Green => "Green",
        ColorLabel::Blue => "Blue",
        ColorLabel::Purple => "Purple",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DARKTABLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="XMP Core 4.4.0-Exiv2">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:darktable="http://darktable.sf.net/"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
   xmp:Rating="2"
   xmp:Label="green"
   darktable:history_end="3">
   <darktable:history>
    <rdf:Seq>
     <rdf:li darktable:operation="exposure" darktable:enabled="1"/>
    </rdf:Seq>
   </darktable:history>
   <dc:subject>
    <rdf:Bag>
     <rdf:li>Kapadokya</rdf:li>
     <rdf:li>balon &amp; gün doğumu</rdf:li>
    </rdf:Bag>
   </dc:subject>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#;

    #[test]
    fn reads_attribute_and_element_forms() {
        let d = parse_xmp(DARKTABLE.as_bytes()).unwrap();
        assert_eq!(d.rating, Some(2));
        assert_eq!(d.color_label, Some(ColorLabel::Green));
        assert_eq!(d.tags, vec!["Kapadokya".to_owned(), "balon & gün doğumu".to_owned()]);

        let lr = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><r:RDF xmlns:r="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<r:Description xmlns:a="http://ns.adobe.com/xap/1.0/"><a:Rating>-1</a:Rating><a:Label>Purple</a:Label></r:Description>
</r:RDF></x:xmpmeta>"#;
        let d = parse_xmp(lr.as_bytes()).unwrap();
        assert_eq!((d.rating, d.color_label), (None, Some(ColorLabel::Purple)));
        assert!(parse_xmp(b"<x><unclosed></x>").is_err());
    }

    #[test]
    fn update_preserves_foreign_content() {
        let data = SidecarData {
            rating: Some(5),
            color_label: Some(ColorLabel::Red),
            tags: vec!["İstanbul".into(), "a<b".into()],
            favorite: None,
        };
        let out = update_xmp(Some(DARKTABLE.as_bytes()), &data).unwrap();
        let text = String::from_utf8(out.clone()).unwrap();
        assert!(text.contains("darktable:operation=\"exposure\""), "{text}");
        assert!(text.contains("darktable:history_end=\"3\""));
        assert!(text.contains("XMP Core 4.4.0-Exiv2"));
        assert!(!text.contains("xmp:Rating=\"2\""));
        assert!(!text.contains("Kapadokya"));
        assert_eq!(parse_xmp(&out).unwrap(), data);
        // İkinci yazım kararlı: aynı veri aynı bayt.
        assert_eq!(update_xmp(Some(&out), &data).unwrap(), out);
    }

    #[test]
    fn fresh_template_round_trip() {
        let data = SidecarData { rating: Some(0), color_label: None, tags: vec![], favorite: None };
        let out = update_xmp(None, &data).unwrap();
        assert_eq!(parse_xmp(&out).unwrap(), data);
    }
}

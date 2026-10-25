//! [`SearchQuery`] süzgeçlerini parametreli SQL'e derler. Kullanıcı değeri
//! hiçbir zaman SQL metnine girmez; yalnızca `?` yer tutucularına bağlanır.

use rusqlite::types::Value;

use crate::query::{Match, SearchQuery, SortMode};

/// `FROM … WHERE …` gövdesi ve parametreleri. Medya takma adı `m`, yer `l`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Compiled {
    pub where_sql: String,
    pub params: Vec<Value>,
}

impl Compiled {
    /// `SELECT {cols} FROM media_files m LEFT JOIN locations l … WHERE …`.
    #[must_use]
    pub fn select(&self, cols: &str) -> String {
        format!(
            "SELECT {cols} FROM media_files m LEFT JOIN locations l ON l.id = m.location_id WHERE {}",
            self.where_sql
        )
    }
}

/// Yerel takvim parçası: `strftime` UTC saniyesine saat dilimi eklenerek uygulanır.
fn local_part(fmt: &str) -> String {
    format!(
        "CAST(strftime('{fmt}', (m.taken_at_utc + COALESCE(m.tz_offset_min, 0) * 60000) / 1000, 'unixepoch') AS INTEGER)"
    )
}

#[must_use]
pub fn compile(q: &SearchQuery) -> Compiled {
    let mut w: Vec<String> = vec!["m.soft_deleted_at IS NULL".into()];
    let mut p: Vec<Value> = Vec::new();
    let bind = |p: &mut Vec<Value>, v: Value| {
        p.push(v);
        format!("?{}", p.len())
    };

    if let Some(persons) = &q.persons {
        match persons.mode {
            Match::All => {
                for &id in &persons.ids {
                    let ph = bind(&mut p, Value::Integer(id));
                    w.push(format!(
                        "EXISTS (SELECT 1 FROM v_face_person v WHERE v.media_id = m.id AND v.person_id = {ph})"
                    ));
                }
            }
            Match::Any => {
                let phs: Vec<String> = persons.ids.iter().map(|&id| bind(&mut p, Value::Integer(id))).collect();
                w.push(format!(
                    "EXISTS (SELECT 1 FROM v_face_person v WHERE v.media_id = m.id AND v.person_id IN ({}))",
                    phs.join(", ")
                ));
            }
        }
    }

    if let Some(place) = &q.place {
        let names = [
            ("l.country_code", &place.country_code),
            ("l.country", &place.country),
            ("l.admin1", &place.admin1),
            ("l.admin2", &place.admin2),
            ("l.locality", &place.locality),
        ];
        for (col, val) in names {
            if let Some(v) = val {
                let ph = bind(&mut p, Value::Text(loupe_db::turkish_lowercase(v.trim())));
                w.push(format!("tr_lower({col}) = {ph}"));
            }
        }
        if let Some(b) = place.bbox {
            let (a, c) = (bind(&mut p, Value::Real(b.min_lat)), bind(&mut p, Value::Real(b.max_lat)));
            let (lo, hi) = (bind(&mut p, Value::Real(b.min_lon)), bind(&mut p, Value::Real(b.max_lon)));
            let lon = if b.min_lon <= b.max_lon {
                format!("m.gps_lon BETWEEN {lo} AND {hi}")
            } else {
                format!("(m.gps_lon >= {lo} OR m.gps_lon <= {hi})")
            };
            w.push(format!("m.gps_lat BETWEEN {a} AND {c} AND {lon}"));
        }
        if let Some(r) = place.radius {
            let (la, lo, km) =
                (bind(&mut p, Value::Real(r.lat)), bind(&mut p, Value::Real(r.lon)), bind(&mut p, Value::Real(r.km)));
            // Enlem kutusu ucuz ön süzgeç; kesin karar büyük çember uzaklığı.
            let dlat = bind(&mut p, Value::Real(r.km / 111.0));
            w.push(format!(
                "m.gps_lat BETWEEN {la} - {dlat} AND {la} + {dlat} AND haversine_km(m.gps_lat, m.gps_lon, {la}, {lo}) <= {km}"
            ));
        }
    }

    if let Some(t) = &q.time {
        if let Some(v) = t.from_utc {
            let ph = bind(&mut p, Value::Integer(v));
            w.push(format!("m.taken_at_utc >= {ph}"));
        }
        if let Some(v) = t.to_utc {
            let ph = bind(&mut p, Value::Integer(v));
            w.push(format!("m.taken_at_utc < {ph}"));
        }
        for (fmt, v) in [("%Y", t.year.map(i64::from)), ("%m", t.month.map(i64::from)), ("%d", t.day.map(i64::from))] {
            if let Some(v) = v {
                let ph = bind(&mut p, Value::Integer(v));
                w.push(format!("{} = {ph}", local_part(fmt)));
            }
        }
    }

    if let Some(e) = q.event_id {
        let ph = bind(&mut p, Value::Integer(e));
        w.push(format!(
            "EXISTS (SELECT 1 FROM event_media em WHERE em.media_id = m.id AND em.event_id = {ph}
                      AND em.source <> 'user_excluded')"
        ));
    }
    if let Some(r) = q.min_rating {
        let ph = bind(&mut p, Value::Integer(i64::from(r)));
        w.push(format!("m.rating >= {ph}"));
    }
    if let Some(f) = q.favorite {
        let ph = bind(&mut p, Value::Integer(i64::from(f)));
        w.push(format!("m.is_favorite = {ph}"));
    }
    if !q.color_labels.is_empty() {
        let phs: Vec<String> = q.color_labels.iter().map(|c| bind(&mut p, Value::Text(c.clone()))).collect();
        w.push(format!("m.color_label IN ({})", phs.join(", ")));
    }
    for (col, val) in [("m.camera_model", &q.camera_model), ("m.lens_model", &q.lens_model)] {
        if let Some(v) = val {
            let ph = bind(&mut p, Value::Text(v.trim().to_owned()));
            w.push(format!("{col} = {ph} COLLATE NOCASE"));
        }
    }

    Compiled { where_sql: w.join("\n   AND "), params: p }
}

/// Metinsiz sorgunun sıralaması.
#[must_use]
pub const fn order_by(sort: SortMode) -> &'static str {
    match sort {
        SortMode::Relevance | SortMode::TakenDesc => "m.taken_at_utc DESC, m.id DESC",
        SortMode::TakenAsc => "m.taken_at_utc ASC, m.id ASC",
        SortMode::RatingDesc => "m.rating DESC, m.taken_at_utc DESC, m.id DESC",
    }
}

/// Sözcük araması için medyanın aranabilir metni (Türkçe küçük harf):
/// dosya adı, yer hiyerarşisi, etkinlik başlıkları ve kişi adları.
pub const HAYSTACK: &str =
    "tr_lower(m.original_filename || ' ' || COALESCE(l.locality, '') || ' ' || COALESCE(l.admin2, '')
    || ' ' || COALESCE(l.admin1, '') || ' ' || COALESCE(l.country, '') || ' ' || COALESCE(l.user_label, '')
    || ' ' || COALESCE((SELECT group_concat(e.title, ' ') FROM event_media em JOIN events e ON e.id = em.event_id
                        WHERE em.media_id = m.id AND em.source <> 'user_excluded'), '')
    || ' ' || COALESCE((SELECT group_concat(pp.name, ' ') FROM v_face_person v JOIN persons pp ON pp.id = v.person_id
                        WHERE v.media_id = m.id), ''))";

//! Puan, renk etiketi ve favori: önce katalog (tek transaction), sonra
//! XMP + `.loupe.json` sidecar. Disk çevrimdışıysa `sidecar_dirty` işaretli
//! kalır ve disk geri geldiğinde [`flush_dirty_sidecars`] yazar.

use std::str::FromStr;

use loupe_core::{ColorLabel, MediaId};
use loupe_db::Pool;
use loupe_db::models::UserFieldsUpdate;
use loupe_db::repo::media;
use loupe_media::{Sidecar, SidecarData};
use rusqlite::TransactionBehavior;
use serde::{Deserialize, Serialize};

use crate::library::Libraries;
use crate::{AppError, AppResult};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CullRequest {
    pub ids: Vec<i64>,
    /// 0–5 (0 sıfırlar).
    pub rating: Option<u8>,
    /// `red` | `yellow` | `green` | `blue` | `purple` | `none` (kaldır).
    pub label: Option<String>,
    pub favorite: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CullResult {
    pub id: i64,
    pub rating: u8,
    pub label: Option<String>,
    pub favorite: bool,
    /// `written` | `pending` (disk çevrimdışı, sonra yazılacak) | `failed`
    pub sidecar: &'static str,
}

/// Toplu seçimde en fazla.
pub const MAX_IDS: usize = 10_000;

pub fn apply(libs: &Libraries, pool: &Pool, req: &CullRequest) -> AppResult<Vec<CullResult>> {
    if req.ids.is_empty() || req.ids.len() > MAX_IDS {
        return Err(AppError::Invalid(format!("1–{MAX_IDS} medya seçilmeli")));
    }
    if req.rating.is_some_and(|r| r > 5) {
        return Err(AppError::Invalid("puan 0–5 olmalı".into()));
    }
    let label = match req.label.as_deref() {
        None => None,
        Some("none" | "") => Some(None),
        Some(s) => Some(Some(ColorLabel::from_str(s).map_err(|e| AppError::Invalid(e.to_string()))?)),
    };
    if req.rating.is_none() && label.is_none() && req.favorite.is_none() {
        return Err(AppError::Invalid("değiştirilecek alan yok".into()));
    }
    let update = UserFieldsUpdate { rating: req.rating, color_label: label, is_favorite: req.favorite, flag: None };
    pool.write(|c| {
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for &id in &req.ids {
            media::update_user_fields(&tx, MediaId(id), &update)?;
        }
        tx.commit()?;
        Ok(())
    })?;
    req.ids.iter().map(|&id| write_sidecar(libs, pool, MediaId(id))).collect()
}

/// Kataloğun güncel değerlerini sidecar'a yazar; başarıda `sidecar_dirty` temizlenir.
pub fn write_sidecar(libs: &Libraries, pool: &Pool, id: MediaId) -> AppResult<CullResult> {
    let m = pool.read(|c| media::get(c, id))?.ok_or(AppError::NotFound { entity: "medya", id: id.0 })?;
    let mut out = CullResult {
        id: id.0,
        rating: m.rating,
        label: m.color_label.map(|l| l.as_str().to_owned()),
        favorite: m.is_favorite,
        sidecar: "pending",
    };
    let Some(path) = libs.media_path(&m)? else { return Ok(out) };
    let sc = Sidecar::for_media(&path);
    // Etiketler (dc:subject) başka araçların; okunup aynen korunur.
    let tags = match sc.read() {
        Ok(existing) => existing.map(|d| d.tags).unwrap_or_default(),
        Err(_) => {
            out.sidecar = "failed";
            return Ok(out);
        }
    };
    let data = SidecarData { rating: Some(m.rating), color_label: m.color_label, tags, favorite: Some(m.is_favorite) };
    if sc.write(&data).is_err() {
        out.sidecar = "failed";
        return Ok(out);
    }
    // Yalnızca yazdığımız değerler hâlâ geçerliyse temizle (arada yeni bir değişiklik geldiyse kirli kalsın).
    pool.write(|c| {
        c.execute(
            "UPDATE media_files SET sidecar_dirty = 0
              WHERE id = ?1 AND rating = ?2 AND color_label IS ?3 AND is_favorite = ?4",
            rusqlite::params![id.0, m.rating, m.color_label.map(|l| l.as_str()), m.is_favorite],
        )?;
        Ok(())
    })?;
    out.sidecar = "written";
    Ok(out)
}

/// Bekleyen sidecar yazımları (açılışta ve disk takılınca).
pub fn flush_dirty_sidecars(libs: &Libraries, pool: &Pool, limit: u32) -> AppResult<usize> {
    let ids: Vec<i64> = pool.read(|c| {
        Ok(c.prepare_cached("SELECT id FROM media_files WHERE sidecar_dirty = 1 ORDER BY id LIMIT ?1")?
            .query_map([limit], |r| r.get(0))?
            .collect::<Result<_, _>>()?)
    })?;
    let mut written = 0;
    for id in ids {
        if write_sidecar(libs, pool, MediaId(id))?.sidecar == "written" {
            written += 1;
        }
    }
    Ok(written)
}

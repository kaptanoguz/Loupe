//! Model kimliği ve `ml_models` kaydı. Vektörler hangi modelle üretildiğini
//! taşır; model değişince eski vektörler karşılaştırılmaz.

use std::path::Path;

use loupe_core::now_ms;
use rusqlite::{Connection, OptionalExtension, params};

use crate::AiError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelTask {
    FaceDetect,
    FaceEmbed,
    ClipImage,
    ClipText,
}

impl ModelTask {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FaceDetect => "face_detect",
            Self::FaceEmbed => "face_embed",
            Self::ClipImage => "clip_image",
            Self::ClipText => "clip_text",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub task: ModelTask,
    pub name: String,
    pub version: String,
    /// Gömme boyutu (tespit modelinde `None`).
    pub dim: Option<usize>,
    /// Model dosyasının BLAKE3 özeti: bozuk ya da değiştirilmiş model ayırt edilir.
    pub file_blake3: [u8; 32],
}

impl ModelInfo {
    /// Dosyadan kimlik: özet dosya içeriğinden hesaplanır.
    pub fn from_file(
        task: ModelTask,
        name: &str,
        version: &str,
        dim: Option<usize>,
        path: &Path,
    ) -> Result<Self, AiError> {
        let bytes = std::fs::read(path).map_err(|e| AiError::ModelLoad { name: name.into(), reason: e.to_string() })?;
        Ok(Self {
            task,
            name: name.into(),
            version: version.into(),
            dim,
            file_blake3: *blake3::hash(&bytes).as_bytes(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModelId(pub i64);

/// Modeli kaydeder ve görevinin etkin modeli yapar (görev başına tek etkin model).
pub fn register(conn: &Connection, m: &ModelInfo) -> Result<ModelId, AiError> {
    let existing: Option<(i64, Vec<u8>, bool)> = conn
        .prepare_cached(
            "SELECT id, file_blake3, is_active FROM ml_models WHERE task = ?1 AND name = ?2 AND version = ?3",
        )?
        .query_row(params![m.task.as_str(), m.name, m.version], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?;
    let dim = m.dim.map(i64::try_from).transpose().map_err(|e| AiError::InvalidInput(e.to_string()))?;
    let id = match existing {
        Some((id, hash, active)) => {
            if hash != m.file_blake3 {
                return Err(AiError::ModelLoad {
                    name: m.name.clone(),
                    reason: "aynı ad ve sürümle farklı model dosyası kayıtlı".into(),
                });
            }
            if active {
                return Ok(ModelId(id));
            }
            id
        }
        None => conn
            .prepare_cached(
                "INSERT INTO ml_models (task, name, version, dim, file_blake3, is_active, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6) RETURNING id",
            )?
            .query_row(params![m.task.as_str(), m.name, m.version, dim, m.file_blake3.as_slice(), now_ms()], |r| {
                r.get(0)
            })?,
    };
    conn.execute(
        "UPDATE ml_models SET is_active = 0 WHERE task = ?1 AND id <> ?2 AND is_active = 1",
        params![m.task.as_str(), id],
    )?;
    conn.execute("UPDATE ml_models SET is_active = 1 WHERE id = ?1", [id])?;
    Ok(ModelId(id))
}

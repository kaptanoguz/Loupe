//! `cloud_targets` satırları. Sırlar DB'de değil Secret Service'tedir
//! (`secret_ref` yalnızca anahtar adıdır).

use rusqlite::{Connection, OptionalExtension, params};

use crate::CloudError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTarget<'a> {
    pub name: &'a str,
    /// `s3` | `b2` | `r2` | `wasabi` | `webdav` | `local` (…)
    pub provider: &'a str,
    pub config: serde_json::Value,
    pub secret_ref: &'a str,
    pub key_prefix: &'a str,
    /// Şifreliyse kurtarma alıcısı (`age1…`).
    pub age_recipient: Option<&'a str>,
    pub bandwidth_bps: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub id: i64,
    pub name: String,
    pub provider: String,
    pub config: serde_json::Value,
    pub secret_ref: String,
    pub key_prefix: String,
    pub encrypted: bool,
    pub age_recipient: Option<String>,
    pub bandwidth_bps: Option<u64>,
    pub enabled: bool,
}

pub fn insert(conn: &Connection, t: &NewTarget<'_>, now_ms: i64) -> Result<i64, CloudError> {
    let bw = t.bandwidth_bps.map(i64::try_from).transpose().map_err(|_| CloudError::Config("bant genişliği".into()))?;
    conn.execute(
        "INSERT INTO cloud_targets (name, provider, config, secret_ref, key_prefix, encryption, age_recipient,
                                    bandwidth_bps, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
        params![
            t.name,
            t.provider,
            t.config.to_string(),
            t.secret_ref,
            t.key_prefix,
            if t.age_recipient.is_some() { "age" } else { "none" },
            t.age_recipient,
            bw,
            now_ms
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get(conn: &Connection, id: i64) -> Result<Option<Target>, CloudError> {
    let row = conn
        .query_row(
            "SELECT id, name, provider, config, secret_ref, key_prefix, encryption, age_recipient, bandwidth_bps, enabled
               FROM cloud_targets WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    Target {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        provider: r.get(2)?,
                        config: serde_json::Value::Null,
                        secret_ref: r.get(4)?,
                        key_prefix: r.get(5)?,
                        encrypted: r.get::<_, String>(6)? == "age",
                        age_recipient: r.get(7)?,
                        bandwidth_bps: r.get::<_, Option<i64>>(8)?.and_then(|v| u64::try_from(v).ok()),
                        enabled: r.get::<_, i64>(9)? == 1,
                    },
                    r.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?;
    row.map(|(mut t, cfg)| {
        t.config = serde_json::from_str(&cfg).map_err(|e| CloudError::Config(format!("config JSON: {e}")))?;
        Ok(t)
    })
    .transpose()
}

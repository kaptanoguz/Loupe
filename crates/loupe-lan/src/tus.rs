//! tus 1.0.0 devam edebilir yükleme (RFC-001B §2.6).
//!
//! - `POST /files`: oturum açar. Önce çakışma kararı verilir: içerik zaten
//!   katalogdaysa (aynı `device_asset_id` + aynı BLAKE3 ya da başka yoldan
//!   gelmiş aynı içerik) oturum açılmaz, `200 Already Synced` döner.
//! - `HEAD /files/{id}`: `Upload-Offset` = diskte fsync edilmiş bayt.
//! - `PATCH /files/{id}`: parça önce `.part` dosyasına yazılır, fsync edilir,
//!   **sonra** `lan_transfers.offset_bytes` güncellenir. İstek gövdesi yarıda
//!   koparsa o istekte gelen baytlar atılır (dosya son onaylı offset'e kesilir):
//!   telefonun HEAD ile göreceği offset her zaman diskte gerçekten olan ve
//!   sağlamlığı bilinen bayttır. İsteğe bağlı `Upload-Checksum: blake3 <base64>`
//!   parçayı doğrular.
//! - Son parçada tam dosya BLAKE3'ü bildirilenle karşılaştırılır; eşleşirse dosya
//!   `loupe-storage` atomik importer'ıyla (aynı FS'te yalnızca `rename`)
//!   kütüphaneye girer. Medya satırı, `device_assets` ve oturumun `done` durumu
//!   tek transaction'dadır.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use loupe_core::{Blake3Hash, CommitOutcome, ImportCatalog, ImportMode, LibraryId, MediaId, NewMediaFile, Placement};
use loupe_db::Pool;
use loupe_storage::{ImportOutcome, Importer};
use rand::RngCore;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use crate::conflict::{Decision, Resolution, UploadKind, classify, decide};
use crate::store::{self, Device, NewTransfer, Transfer, write_tx};
use crate::{Clock, LanError, MAX_PATCH_BYTES, Telemetry, UPLOAD_TTL_MS};

/// Dosya adının staging'de alabileceği en fazla bayt.
const MAX_NAME_BYTES: usize = 200;

/// `POST /files` üst verisi (tus `Upload-Length` + `Upload-Metadata`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadMeta {
    pub filename: String,
    pub device_asset_id: String,
    pub blake3: Blake3Hash,
    pub size: u64,
    pub modified_at_ms: i64,
    pub taken_at_ms: Option<i64>,
    /// "Sor" politikasında telefonun verdiği karar.
    pub resolution: Option<Resolution>,
}

impl UploadMeta {
    /// tus başlıklarından. `Upload-Metadata`: `anahtar base64değer` çiftleri, virgülle.
    pub fn from_tus(upload_length: Option<&str>, metadata: Option<&str>) -> Result<Self, LanError> {
        let bad = |m: &str| LanError::BadRequest(m.to_owned());
        let size: u64 = upload_length
            .ok_or_else(|| bad("Upload-Length gerekli"))?
            .trim()
            .parse()
            .map_err(|_| bad("Upload-Length"))?;
        let mut md: HashMap<String, String> = HashMap::new();
        for pair in metadata.unwrap_or_default().split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once(' ').unwrap_or((pair, ""));
            let v = STANDARD.decode(v.trim()).map_err(|_| bad("Upload-Metadata base64 değil"))?;
            let v = String::from_utf8(v).map_err(|_| bad("Upload-Metadata UTF-8 değil"))?;
            md.insert(k.to_owned(), v);
        }
        let get = |k: &str| md.get(k).map(String::as_str).ok_or_else(|| LanError::BadRequest(format!("{k} eksik")));
        let int = |k: &str| -> Result<Option<i64>, LanError> {
            md.get(k).map(|v| v.parse::<i64>().map_err(|_| LanError::BadRequest(format!("{k} sayı değil")))).transpose()
        };
        let device_asset_id = get("device_asset_id")?.to_owned();
        if device_asset_id.is_empty() || device_asset_id.len() > 512 {
            return Err(bad("device_asset_id 1–512 bayt olmalı"));
        }
        Ok(Self {
            filename: sanitize_filename(get("filename")?)?,
            device_asset_id,
            blake3: Blake3Hash::from_hex(get("blake3")?).map_err(|_| bad("blake3 64 haneli hex olmalı"))?,
            size,
            modified_at_ms: int("modified_at")?.ok_or_else(|| bad("modified_at eksik"))?,
            taken_at_ms: int("taken_at")?,
            resolution: md.get("resolution").map(|r| Resolution::parse(r)).transpose()?,
        })
    }

    /// İstemci tarafı: `Upload-Metadata` başlık değeri.
    #[must_use]
    pub fn to_tus_metadata(&self) -> String {
        let mut pairs = vec![
            ("filename", self.filename.clone()),
            ("device_asset_id", self.device_asset_id.clone()),
            ("blake3", self.blake3.to_hex()),
            ("modified_at", self.modified_at_ms.to_string()),
        ];
        if let Some(t) = self.taken_at_ms {
            pairs.push(("taken_at", t.to_string()));
        }
        if let Some(r) = self.resolution {
            pairs.push((
                "resolution",
                match r {
                    Resolution::ReplaceWithNewer => "replace_with_newer",
                    Resolution::KeepBoth => "keep_both",
                    Resolution::Skip => "skip",
                }
                .to_owned(),
            ));
        }
        pairs.iter().map(|(k, v)| format!("{k} {}", STANDARD.encode(v))).collect::<Vec<_>>().join(",")
    }
}

/// Yalnızca son yol bileşeni; boş, `.`/`..`, NUL içeren ad reddedilir.
pub fn sanitize_filename(name: &str) -> Result<String, LanError> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default().trim();
    if base.is_empty() || base == "." || base == ".." || base.contains('\0') {
        return Err(LanError::BadRequest(format!("geçersiz dosya adı: {name:?}")));
    }
    let mut end = base.len().min(MAX_NAME_BYTES);
    while !base.is_char_boundary(end) {
        end -= 1;
    }
    Ok(base[..end].to_owned())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateOutcome {
    /// Oturum açıldı ya da aynı varlığın yarım oturumu bulundu (`resumed`).
    Created {
        upload_id: String,
        offset: u64,
        resumed: bool,
    },
    AlreadySynced {
        media_id: Option<MediaId>,
    },
    Skipped,
    /// Politika "sor": telefon kararı `resolution` üst verisiyle yeniden gönderir.
    Conflict {
        previous: Option<MediaId>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatchOutcome {
    pub offset: u64,
    /// Son parçayla dosya doğrulandı ve kataloğa girdi.
    pub completed: Option<MediaId>,
}

/// `POST /manifest` öğesi.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct ManifestItem {
    pub device_asset_id: String,
    /// 64 haneli hex.
    pub blake3: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ManifestEntry {
    pub device_asset_id: String,
    /// `have` | `new` | `conflict`
    pub status: String,
    pub media_id: Option<i64>,
}

#[derive(Debug)]
pub struct UploadService {
    pool: Arc<Pool>,
    importer: Arc<Mutex<Importer>>,
    library_id: LibraryId,
    staging: PathBuf,
    telemetry: Arc<Telemetry>,
    clock: Arc<dyn Clock>,
    max_active_per_device: usize,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

fn new_upload_id() -> String {
    let mut b = [0u8; 16];
    rand::rng().fill_bytes(&mut b);
    URL_SAFE_NO_PAD.encode(b)
}

impl UploadService {
    pub fn new(
        pool: Arc<Pool>,
        importer: Arc<Mutex<Importer>>,
        library_id: LibraryId,
        telemetry: Arc<Telemetry>,
        clock: Arc<dyn Clock>,
        max_active_per_device: usize,
    ) -> Result<Self, LanError> {
        let staging =
            importer.lock().map_err(|_| LanError::Join("importer kilidi zehirlendi".into()))?.lan_staging_dir();
        std::fs::create_dir_all(&staging).map_err(LanError::io("LAN staging dizini"))?;
        Ok(Self {
            pool,
            importer,
            library_id,
            staging,
            telemetry,
            clock,
            max_active_per_device,
            locks: Mutex::new(HashMap::new()),
        })
    }

    #[must_use]
    pub fn staging_dir(&self) -> &Path {
        &self.staging
    }

    fn lock_for(&self, upload_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        match self.locks.lock() {
            Ok(mut m) => {
                m.retain(|_, l| Arc::strong_count(l) > 1);
                m.entry(upload_id.to_owned()).or_default().clone()
            }
            Err(_) => Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    fn staging_path(&self, t: &Transfer) -> PathBuf {
        self.staging.join(&t.staging_name)
    }

    fn remove_staging(&self, staging_name: &str) {
        if let Some((dir, _)) = staging_name.split_once('/') {
            let _ignored = std::fs::remove_dir_all(self.staging.join(dir));
        }
    }

    async fn blocking<T: Send + 'static>(
        f: impl FnOnce() -> Result<T, LanError> + Send + 'static,
    ) -> Result<T, LanError> {
        tokio::task::spawn_blocking(f).await?
    }

    pub async fn create(&self, device: &Device, meta: UploadMeta) -> Result<CreateOutcome, LanError> {
        let pool = self.pool.clone();
        let now = self.clock.now_ms();
        let (library, policy, max) =
            (device.target_library_id.unwrap_or(self.library_id), device.conflict_policy, self.max_active_per_device);
        if library != self.library_id {
            return Err(LanError::BadRequest("cihazın hedef kütüphanesi bu sunucuda açık değil".into()));
        }
        let (device_id, size) = (device.id, meta.size);
        let (outcome, superseded) = Self::blocking(move || {
            write_tx(&pool, |c| {
                let class = classify(c, device_id, &meta.device_asset_id, &meta.blake3)?;
                match decide(class, policy, meta.resolution) {
                    Decision::AlreadySynced { media_id } => {
                        store::upsert_device_asset(
                            c,
                            device_id,
                            &meta.device_asset_id,
                            &meta.blake3,
                            media_id,
                            meta.modified_at_ms,
                            now,
                        )?;
                        Ok((CreateOutcome::AlreadySynced { media_id }, None))
                    }
                    Decision::Skip => {
                        // Bu sürüm "görüldü": bir daha sorulmaz, eski medya bağı korunur.
                        store::upsert_device_asset(
                            c,
                            device_id,
                            &meta.device_asset_id,
                            &meta.blake3,
                            None,
                            meta.modified_at_ms,
                            now,
                        )?;
                        Ok((CreateOutcome::Skipped, None))
                    }
                    Decision::Ask { previous } => Ok((CreateOutcome::Conflict { previous }, None)),
                    Decision::Upload(kind) => Self::open_session(c, device_id, library, &meta, kind, max, now),
                }
            })
        })
        .await?;
        if let Some(name) = superseded {
            self.remove_staging(&name);
        }
        match &outcome {
            CreateOutcome::Created { upload_id, resumed: false, .. } => {
                let dir = self.staging.join(upload_id);
                tokio::fs::create_dir_all(&dir).await.map_err(LanError::io("staging dizini"))?;
                self.telemetry.upload_started(size);
            }
            CreateOutcome::Created { .. } => {}
            CreateOutcome::AlreadySynced { .. } => self.telemetry.already_synced(),
            CreateOutcome::Skipped => self.telemetry.skipped(),
            CreateOutcome::Conflict { .. } => self.telemetry.conflict(),
        }
        Ok(outcome)
    }

    fn open_session(
        c: &rusqlite::Connection,
        device_id: i64,
        library: LibraryId,
        meta: &UploadMeta,
        kind: UploadKind,
        max_active: usize,
        now: i64,
    ) -> Result<(CreateOutcome, Option<String>), LanError> {
        let mut superseded = None;
        if let Some(old) = store::active_transfer(c, device_id, &meta.device_asset_id)? {
            if old.expected_blake3 == meta.blake3 && old.expected_size == meta.size && old.expires_at > now {
                return Ok((
                    CreateOutcome::Created { upload_id: old.upload_id, offset: old.offset, resumed: true },
                    None,
                ));
            }
            // Telefonda dosya yine değişti: yarım oturum geçersiz.
            store::set_state(c, &old.upload_id, "failed", Some("SUPERSEDED"), now)?;
            superseded = Some(old.staging_name);
        }
        if store::active_transfer_count(c, device_id)? >= max_active {
            return Err(LanError::TooManyUploads);
        }
        let upload_id = new_upload_id();
        let staging_name = format!("{upload_id}/{}", meta.filename);
        store::insert_transfer(
            c,
            &NewTransfer {
                upload_id: &upload_id,
                device_id,
                library_id: library,
                device_asset_id: &meta.device_asset_id,
                filename: &meta.filename,
                expected_blake3: &meta.blake3,
                expected_size: meta.size,
                staging_name: &staging_name,
                client_taken_at: meta.taken_at_ms,
                client_modified_at: meta.modified_at_ms,
                kind,
                now_ms: now,
                expires_at: now + UPLOAD_TTL_MS,
            },
        )?;
        Ok((CreateOutcome::Created { upload_id, offset: 0, resumed: false }, superseded))
    }

    async fn load(&self, device_id: i64, upload_id: &str) -> Result<Transfer, LanError> {
        let pool = self.pool.clone();
        let id = upload_id.to_owned();
        let t = Self::blocking(move || pool.read(|c| Ok(store::transfer(c, &id)))?).await?;
        let t = t.ok_or(LanError::UploadNotFound)?;
        // Başka cihazın oturumu var olmayan oturumla aynı yanıtı alır.
        if t.device_id != device_id {
            return Err(LanError::UploadNotFound);
        }
        if t.state != "receiving" && t.state != "verifying" {
            return Err(LanError::UploadGone(t.state));
        }
        if t.expires_at <= self.clock.now_ms() {
            return Err(LanError::UploadGone("expired".into()));
        }
        Ok(t)
    }

    /// `HEAD`: (offset, toplam boyut).
    pub async fn offset(&self, device_id: i64, upload_id: &str) -> Result<(u64, u64), LanError> {
        let lock = self.lock_for(upload_id);
        let _held = lock.lock().await;
        let t = self.load(device_id, upload_id).await?;
        Ok((t.offset, t.expected_size))
    }

    pub async fn patch<S, E>(
        &self,
        device_id: i64,
        upload_id: &str,
        client_offset: u64,
        chunk_blake3: Option<[u8; 32]>,
        body: S,
    ) -> Result<PatchOutcome, LanError>
    where
        S: Stream<Item = Result<Bytes, E>> + Send,
    {
        let lock = self.lock_for(upload_id);
        let _held = lock.lock().await;
        let t = self.load(device_id, upload_id).await?;
        if t.state != "receiving" {
            return Err(LanError::UploadGone(t.state));
        }
        if client_offset != t.offset {
            return Err(LanError::OffsetMismatch { server: t.offset, client: client_offset });
        }
        let path = self.staging_path(&t);
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir).await.map_err(LanError::io("staging dizini"))?;
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .await
            .map_err(LanError::io("staging dosyası"))?;
        // Önceki kopmuş istekten kalan onaysız kuyruk atılır.
        file.set_len(t.offset).await.map_err(LanError::io("staging kesme"))?;
        file.seek(std::io::SeekFrom::Start(t.offset)).await.map_err(LanError::io("staging konum"))?;

        let mut written: u64 = 0;
        let mut hasher = blake3::Hasher::new();
        let mut body = std::pin::pin!(body);
        let failure = loop {
            let Some(next) = body.next().await else { break None };
            let Ok(chunk) = next else { break Some(LanError::Aborted) };
            let n = chunk.len() as u64;
            if t.offset + written + n > t.expected_size || written + n > MAX_PATCH_BYTES {
                break Some(LanError::TooLarge);
            }
            hasher.update(&chunk);
            if let Err(e) = file.write_all(&chunk).await {
                break Some(LanError::Io { context: "staging yazma", source: e });
            }
            written += n;
            self.telemetry.bytes(n);
        };
        let failure = failure.or_else(|| match chunk_blake3 {
            Some(c) if *hasher.finalize().as_bytes() != c => Some(LanError::ChunkChecksum),
            _ => None,
        });
        if let Some(e) = failure {
            let _ignored = file.set_len(t.offset).await;
            self.telemetry.bytes_discarded(written);
            return Err(e);
        }
        file.flush().await.map_err(LanError::io("staging flush"))?;
        file.sync_all().await.map_err(LanError::io("staging fsync"))?;
        drop(file);

        let offset = t.offset + written;
        let (pool, id, now) = (self.pool.clone(), t.upload_id.clone(), self.clock.now_ms());
        Self::blocking(move || write_tx(&pool, |c| store::set_offset(c, &id, offset, now))).await?;
        if offset < t.expected_size {
            return Ok(PatchOutcome { offset, completed: None });
        }
        let media = self.finalize(t).await?;
        Ok(PatchOutcome { offset, completed: Some(media) })
    }

    /// Tam dosya BLAKE3 → atomik içe aktarma → oturum `done`.
    async fn finalize(&self, t: Transfer) -> Result<MediaId, LanError> {
        let path = self.staging_path(&t);
        let (pool, importer, clock) = (self.pool.clone(), self.importer.clone(), self.clock.clone());
        let staging_name = t.staging_name.clone();
        let result = Self::blocking(move || {
            let now = clock.now_ms();
            write_tx(&pool, |c| store::set_state(c, &t.upload_id, "verifying", None, now))?;
            let actual = hash_path(&path)?;
            if actual != t.expected_blake3 {
                write_tx(&pool, |c| store::set_state(c, &t.upload_id, "failed", Some("BLAKE3_MISMATCH"), now))?;
                return Err(LanError::FileChecksum);
            }
            // MtimeResolver tarih ağacı için telefonun zamanını kullanır.
            let when = t.client_taken_at.unwrap_or(t.client_modified_at);
            if let Some(ts) = system_time(when) {
                let f = std::fs::File::options().write(true).open(&path).map_err(LanError::io("staging"))?;
                f.set_modified(ts).map_err(LanError::io("staging mtime"))?;
            }
            let importer = importer.lock().map_err(|_| LanError::Join("importer kilidi zehirlendi".into()))?;
            let mut catalog = LanCatalog { pool: &pool, transfer: &t, clock: clock.as_ref() };
            let outcome = importer.import_file_as(&mut catalog, &path, ImportMode::Move, None, Placement::Lan);
            match outcome {
                Ok(ImportOutcome::Imported { media_id, .. }) => Ok(media_id),
                Ok(ImportOutcome::Duplicate { existing, .. }) => {
                    write_tx(&pool, |c| store::complete_transfer(c, &t, existing, false, clock.now_ms()))?;
                    Ok(existing)
                }
                Err(e) => {
                    write_tx(&pool, |c| store::set_state(c, &t.upload_id, "failed", Some("IMPORT"), clock.now_ms()))?;
                    Err(e.into())
                }
            }
        })
        .await;
        self.remove_staging(&staging_name);
        match &result {
            Ok(_) => self.telemetry.verified(),
            Err(_) => self.telemetry.failed(0),
        }
        result
    }

    /// tus termination.
    pub async fn terminate(&self, device_id: i64, upload_id: &str) -> Result<(), LanError> {
        let lock = self.lock_for(upload_id);
        let _held = lock.lock().await;
        let t = self.load(device_id, upload_id).await?;
        let (pool, now) = (self.pool.clone(), self.clock.now_ms());
        let id = t.upload_id.clone();
        Self::blocking(move || write_tx(&pool, |c| store::set_state(c, &id, "failed", Some("TERMINATED"), now)))
            .await?;
        self.remove_staging(&t.staging_name);
        self.telemetry.failed(t.expected_size.saturating_sub(t.offset));
        Ok(())
    }

    /// Telefonun bekleyen listesini sınıflandırır; `have` öğeler bağlanır ve
    /// ağdan hiç geçmez.
    pub async fn manifest(&self, device: &Device, items: Vec<ManifestItem>) -> Result<Vec<ManifestEntry>, LanError> {
        let (pool, now, device_id) = (self.pool.clone(), self.clock.now_ms(), device.id);
        Self::blocking(move || {
            write_tx(&pool, |c| {
                items
                    .into_iter()
                    .map(|it| {
                        let hash = Blake3Hash::from_hex(&it.blake3)
                            .map_err(|_| LanError::BadRequest(format!("{}: blake3 geçersiz", it.device_asset_id)))?;
                        let (status, media) = match classify(c, device_id, &it.device_asset_id, &hash)? {
                            crate::Classification::AlreadySynced { media_id } => {
                                store::upsert_device_asset(
                                    c,
                                    device_id,
                                    &it.device_asset_id,
                                    &hash,
                                    media_id,
                                    now,
                                    now,
                                )?;
                                ("have", media_id)
                            }
                            crate::Classification::New => ("new", None),
                            crate::Classification::Conflict { previous } => ("conflict", previous),
                        };
                        Ok(ManifestEntry {
                            device_asset_id: it.device_asset_id,
                            status: status.to_owned(),
                            media_id: media.map(|m| m.0),
                        })
                    })
                    .collect()
            })
        })
        .await
    }

    /// Süresi dolan oturumları kapatır ve yarım dosyalarını siler.
    pub async fn expire(&self) -> Result<usize, LanError> {
        let (pool, now) = (self.pool.clone(), self.clock.now_ms());
        let names = Self::blocking(move || write_tx(&pool, |c| store::expire_transfers(c, now))).await?;
        for n in &names {
            self.remove_staging(n);
        }
        Ok(names.len())
    }
}

fn hash_path(path: &Path) -> Result<Blake3Hash, LanError> {
    let mut f = std::fs::File::open(path).map_err(LanError::io("staging okuma"))?;
    let mut h = blake3::Hasher::new();
    std::io::copy(&mut f, &mut h).map_err(LanError::io("staging okuma"))?;
    Ok(Blake3Hash(*h.finalize().as_bytes()))
}

fn system_time(ms: i64) -> Option<std::time::SystemTime> {
    let d = std::time::Duration::from_millis(u64::try_from(ms).ok()?);
    std::time::UNIX_EPOCH.checked_add(d)
}

/// Kestirmelerden tek istekle gelen dosyanın sonucu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WholeOutcome {
    pub media_id: MediaId,
    /// İçerik zaten katalogdaydı (aynı BLAKE3); dosya atıldı.
    pub duplicate: bool,
}

/// Tek istekte kabul edilen en büyük dosya (uzun 4K video payı).
pub const MAX_WHOLE_BYTES: u64 = 16 * 1024 * 1024 * 1024;

impl UploadService {
    /// Kestirmeler tus konuşamaz ve özeti önceden hesaplayamaz: gövde tek
    /// istekte gelir, akarken BLAKE3'lenir, sonra aynı atomik importer'la
    /// kütüphaneye girer. Aynı içerik zaten varsa dosya atılır.
    pub async fn receive_whole<S, E>(
        &self,
        name: Option<&str>,
        modified_at_ms: Option<i64>,
        body: S,
    ) -> Result<WholeOutcome, LanError>
    where
        S: Stream<Item = Result<Bytes, E>> + Send,
    {
        let dir_name = new_upload_id();
        let dir = self.staging.join(&dir_name);
        tokio::fs::create_dir_all(&dir).await.map_err(LanError::io("staging dizini"))?;
        let result = self.receive_into(&dir, name, modified_at_ms, body).await;
        self.remove_staging(&format!("{dir_name}/"));
        match &result {
            Ok(_) => self.telemetry.verified(),
            Err(_) => self.telemetry.failed(0),
        }
        result
    }

    async fn receive_into<S, E>(
        &self,
        dir: &Path,
        name: Option<&str>,
        modified_at_ms: Option<i64>,
        body: S,
    ) -> Result<WholeOutcome, LanError>
    where
        S: Stream<Item = Result<Bytes, E>> + Send,
    {
        let part = dir.join("gelen.part");
        let mut file = tokio::fs::File::create(&part).await.map_err(LanError::io("staging dosyası"))?;
        let (mut hasher, mut size, mut head) = (blake3::Hasher::new(), 0u64, Vec::with_capacity(32));
        let mut body = std::pin::pin!(body);
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|_| LanError::Aborted)?;
            size += chunk.len() as u64;
            if size > MAX_WHOLE_BYTES {
                return Err(LanError::TooLarge);
            }
            if head.len() < 32 {
                head.extend_from_slice(&chunk[..chunk.len().min(32 - head.len())]);
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await.map_err(LanError::io("staging yazma"))?;
        }
        if size == 0 {
            return Err(LanError::BadRequest("boş gövde".into()));
        }
        file.sync_all().await.map_err(LanError::io("staging fsync"))?;
        drop(file);
        let digest = Blake3Hash(*hasher.finalize().as_bytes());
        let filename = match name.map(sanitize_filename) {
            Some(Ok(n)) if n.contains('.') => n,
            _ => format!("IMG_{}.{}", &digest.to_hex()[..8], sniff_ext(&head)),
        };
        let path = dir.join(&filename);
        tokio::fs::rename(&part, &path).await.map_err(LanError::io("staging adı"))?;
        let (pool, importer) = (self.pool.clone(), self.importer.clone());
        Self::blocking(move || {
            if let Some(existing) = pool.read(|c| loupe_db::repo::media::find_by_blake3(c, &digest))? {
                return Ok(WholeOutcome { media_id: existing, duplicate: true });
            }
            if let Some(ts) = modified_at_ms.and_then(system_time) {
                let f = std::fs::File::options().write(true).open(&path).map_err(LanError::io("staging"))?;
                f.set_modified(ts).map_err(LanError::io("staging mtime"))?;
            }
            let importer = importer.lock().map_err(|_| LanError::Join("importer kilidi zehirlendi".into()))?;
            let mut catalog = PlainCatalog { pool: &pool };
            Ok(match importer.import_file_as(&mut catalog, &path, ImportMode::Move, None, Placement::Lan)? {
                ImportOutcome::Imported { media_id, .. } => WholeOutcome { media_id, duplicate: false },
                ImportOutcome::Duplicate { existing, .. } => WholeOutcome { media_id: existing, duplicate: true },
            })
        })
        .await
    }
}

/// Dosya adı gelmediyse uzantı içerikten: JPEG, PNG, HEIC/HEIF, MOV, MP4.
#[must_use]
pub fn sniff_ext(head: &[u8]) -> &'static str {
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return "jpg";
    }
    if head.starts_with(b"\x89PNG") {
        return "png";
    }
    match head.get(4..12) {
        Some(b) if &b[..4] == b"ftyp" => match &b[4..8] {
            b"heic" | b"heix" | b"hevc" | b"heim" | b"heis" => "heic",
            b"mif1" | b"msf1" => "heif",
            b"qt  " => "mov",
            _ => "mp4",
        },
        _ => "bin",
    }
}

/// Tek istekli yüklemelerin katalog tarafı: yalnızca medya satırı.
struct PlainCatalog<'a> {
    pool: &'a Pool,
}

impl ImportCatalog for PlainCatalog<'_> {
    type Error = LanError;

    fn find_by_blake3(&mut self, hash: &Blake3Hash) -> Result<Option<MediaId>, LanError> {
        Ok(self.pool.read(|c| loupe_db::repo::media::find_by_blake3(c, hash))?)
    }

    fn commit_media(&mut self, media: &NewMediaFile) -> Result<CommitOutcome, LanError> {
        write_tx(self.pool, |c| Ok(loupe_db::repo::media::insert(c, media)?))
    }
}

/// Importer'ın katalog tarafı: medya satırı, `device_assets` ve oturumun
/// tamamlanması tek `BEGIN IMMEDIATE` içinde.
struct LanCatalog<'a> {
    pool: &'a Pool,
    transfer: &'a Transfer,
    clock: &'a dyn Clock,
}

impl ImportCatalog for LanCatalog<'_> {
    type Error = LanError;

    fn find_by_blake3(&mut self, hash: &Blake3Hash) -> Result<Option<MediaId>, LanError> {
        Ok(self.pool.read(|c| loupe_db::repo::media::find_by_blake3(c, hash))?)
    }

    fn commit_media(&mut self, media: &NewMediaFile) -> Result<CommitOutcome, LanError> {
        let now = self.clock.now_ms();
        write_tx(self.pool, |c| {
            let out = loupe_db::repo::media::insert(c, media)?;
            if let CommitOutcome::Inserted(id) = out {
                store::complete_transfer(c, self.transfer, id, true, now)?;
            }
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tus_metadata_round_trips() {
        let m = UploadMeta {
            filename: "IMG_0412.HEIC".into(),
            device_asset_id: "MediaStore:41:1761390000".into(),
            blake3: Blake3Hash([3; 32]),
            size: 2 * 1024 * 1024,
            modified_at_ms: 1_761_390_000_000,
            taken_at_ms: Some(1_761_380_000_000),
            resolution: Some(Resolution::KeepBoth),
        };
        let back = UploadMeta::from_tus(Some("2097152"), Some(&m.to_tus_metadata())).unwrap();
        assert_eq!(back, m);
        assert!(UploadMeta::from_tus(None, Some(&m.to_tus_metadata())).is_err());
        assert!(UploadMeta::from_tus(Some("1"), Some("filename aGk=")).is_err());
    }

    #[test]
    fn filenames_are_reduced_to_a_safe_basename() {
        assert_eq!(sanitize_filename("../../etc/passwd").unwrap(), "passwd");
        assert_eq!(sanitize_filename("DCIM\\Camera\\IMG_1.JPG").unwrap(), "IMG_1.JPG");
        assert!(sanitize_filename("..").is_err());
        assert!(sanitize_filename("a/").is_err());
        let long = "ğ".repeat(150);
        let s = sanitize_filename(&long).unwrap();
        assert!(s.len() <= MAX_NAME_BYTES && s.chars().all(|c| c == 'ğ'));
    }
}

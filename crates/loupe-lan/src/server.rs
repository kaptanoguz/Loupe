//! HTTP/2 + TLS 1.3 sunucusu (RFC-001B §2.4–2.6).
//!
//! | Uç nokta | Kimlik | Amaç |
//! |---|---|---|
//! | `POST /pair` | QR belirteci | CSR → istemci sertifikası, `devices` kaydı |
//! | `POST /manifest` | mTLS | bekleyen listeyi have / new / conflict diye sınıflandır |
//! | `OPTIONS /files` | mTLS | tus keşfi |
//! | `POST /files` | mTLS | tus creation (+ çakışma kararı) |
//! | `HEAD /files/{id}` | mTLS | `Upload-Offset` |
//! | `PATCH /files/{id}` | mTLS | parça yaz |
//! | `DELETE /files/{id}` | mTLS | tus termination |
//!
//! Sunucu yalnızca LAN adreslerinden (RFC1918, ULA, link-local, loopback)
//! bağlantı kabul eder; diğerleri TLS'ten önce kapatılır.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Extension, Json, Router};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use hyper_util::service::TowerToHyperService;
use loupe_core::LibraryId;
use loupe_db::Pool;
use loupe_storage::Importer;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio_rustls::TlsAcceptor;

use crate::identity::sha256;
use crate::store::{self, Device, write_tx};
use crate::tus::{CreateOutcome, ManifestEntry, ManifestItem, UploadMeta, UploadService};
use crate::{
    Clock, DeviceRegistry, LanError, PairingManager, PairingPayload, ServerIdentity, Telemetry, server_config,
};

pub const TUS_VERSION: &str = "1.0.0";
const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct LanConfig {
    pub bind: SocketAddr,
    /// Telefonlardan gelen dosyaların hedef kütüphanesi (importer ile aynı).
    pub library_id: LibraryId,
    /// mDNS örnek adı ve sunucu sertifikasının CN'i.
    pub display_name: String,
    /// Cihaz başına aynı anda açık yükleme oturumu.
    pub max_active_per_device: usize,
}

impl LanConfig {
    #[must_use]
    pub fn new(bind: SocketAddr, library_id: LibraryId, display_name: &str) -> Self {
        Self { bind, library_id, display_name: display_name.to_owned(), max_active_per_device: 4 }
    }
}

/// Eşleşme isteği için masaüstü onay penceresine giden bilgi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingRequestInfo {
    pub device_name: String,
    pub platform: String,
    pub addr: SocketAddr,
}

/// "Pixel 9 bağlanmak istiyor [Enter / Esc]". 30 sn içinde yanıt yoksa ret.
pub trait PairingApprover: Send + Sync + std::fmt::Debug {
    fn approve(&self, req: PairingRequestInfo) -> Pin<Box<dyn Future<Output = bool> + Send>>;
}

/// Testler ve başsız kurulum: belirteç doğruysa onaylar.
#[derive(Debug, Default, Clone, Copy)]
pub struct AutoApprove;

impl PairingApprover for AutoApprove {
    fn approve(&self, _req: PairingRequestInfo) -> Pin<Box<dyn Future<Output = bool> + Send>> {
        Box::pin(async { true })
    }
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct PairRequest {
    pub token: String,
    /// Telefonda üretilen P-256 anahtarının CSR'ı (PEM).
    pub csr_pem: String,
    pub device_name: String,
    /// `android` | `ios` | `desktop`
    pub platform: String,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct PairResponse {
    pub device_id: i64,
    pub client_cert_pem: String,
    pub ca_cert_pem: String,
    pub install_id: String,
    /// İstemcinin sunucu sertifikasında doğrulayacağı ad.
    pub server_name: String,
}

#[derive(Debug)]
struct Shared {
    pool: Arc<Pool>,
    identity: Arc<ServerIdentity>,
    registry: Arc<DeviceRegistry>,
    pairing: Arc<PairingManager>,
    uploads: Arc<UploadService>,
    telemetry: Arc<Telemetry>,
    clock: Arc<dyn Clock>,
    approver: Arc<dyn PairingApprover>,
    cfg: LanConfig,
}

#[derive(Debug, Clone)]
struct AppState(Arc<Shared>);

#[derive(Debug)]
pub struct LanServer {
    shared: Arc<Shared>,
    local_addr: SocketAddr,
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

/// Bağlantı kabul edilecek kaynak adresler.
#[must_use]
pub fn is_lan(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_loopback() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_lan(IpAddr::V4(v4));
            }
            let s = v6.segments()[0];
            v6.is_loopback() || (s & 0xfe00) == 0xfc00 || (s & 0xffc0) == 0xfe80
        }
    }
}

impl LanServer {
    pub async fn start(
        cfg: LanConfig,
        pool: Arc<Pool>,
        identity: ServerIdentity,
        importer: Importer,
        clock: Arc<dyn Clock>,
        approver: Arc<dyn PairingApprover>,
    ) -> Result<Self, LanError> {
        let registry = Arc::new(DeviceRegistry::default());
        let now = clock.now_ms();
        let p = pool.clone();
        let certs = tokio::task::spawn_blocking(move || p.read(|c| Ok(store::active_device_certs(c, now)))).await??;
        registry.replace_all(certs?);
        let tls = TlsAcceptor::from(server_config(&identity, registry.clone())?);
        let telemetry = Arc::new(Telemetry::new());
        let uploads = Arc::new(UploadService::new(
            pool.clone(),
            Arc::new(Mutex::new(importer)),
            cfg.library_id,
            telemetry.clone(),
            clock.clone(),
            cfg.max_active_per_device,
        )?);
        let listener = TcpListener::bind(cfg.bind).await.map_err(LanError::io("dinleme"))?;
        let local_addr = listener.local_addr().map_err(LanError::io("dinleme adresi"))?;
        let shared = Arc::new(Shared {
            pool,
            identity: Arc::new(identity),
            registry,
            pairing: Arc::new(PairingManager::new(clock.clone())),
            uploads,
            telemetry,
            clock,
            approver,
            cfg,
        });
        let (stop, stopped) = watch::channel(false);
        let task = tokio::spawn(accept_loop(listener, tls, router(AppState(shared.clone())), stopped));
        Ok(Self { shared, local_addr, stop, task })
    }

    #[must_use]
    pub const fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    #[must_use]
    pub fn telemetry(&self) -> Arc<Telemetry> {
        self.shared.telemetry.clone()
    }

    #[must_use]
    pub fn pairing(&self) -> Arc<PairingManager> {
        self.shared.pairing.clone()
    }

    #[must_use]
    pub fn identity(&self) -> Arc<ServerIdentity> {
        self.shared.identity.clone()
    }

    #[must_use]
    pub fn uploads(&self) -> Arc<UploadService> {
        self.shared.uploads.clone()
    }

    /// "Telefon ekle" ekranı: yeni belirteç ve QR yükü (önceki belirteç düşer).
    pub fn pairing_payload(&self, addrs: Vec<IpAddr>) -> Result<PairingPayload, LanError> {
        let t = self.shared.pairing.issue()?;
        Ok(PairingPayload {
            addrs,
            port: self.local_addr.port(),
            ca_spki_sha256: self.shared.identity.ca_spki_sha256(),
            server_cert_sha256: self.shared.identity.server_cert_sha256(),
            token: t.token,
            expires_at_s: t.expires_at_ms.div_euclid(1000),
            name: self.shared.cfg.display_name.clone(),
        })
    }

    /// Cihazı iptal eder; bir sonraki el sıkışması reddedilir.
    pub async fn revoke_device(&self, device_id: i64, reason: &str) -> Result<(), LanError> {
        let (pool, now, reason) = (self.shared.pool.clone(), self.shared.clock.now_ms(), reason.to_owned());
        tokio::task::spawn_blocking(move || write_tx(&pool, |c| store::revoke_device(c, device_id, &reason, now)))
            .await??;
        self.shared.registry.remove_device(device_id);
        Ok(())
    }

    pub async fn shutdown(self) {
        let _ignored = self.stop.send(true);
        let _ignored = self.task.await;
    }
}

async fn accept_loop(listener: TcpListener, tls: TlsAcceptor, app: Router, mut stopped: watch::Receiver<bool>) {
    let mut conns = JoinSet::new();
    loop {
        let accepted = tokio::select! {
            a = listener.accept() => a,
            _ = stopped.changed() => break,
        };
        while conns.try_join_next().is_some() {}
        let Ok((tcp, addr)) = accepted else { continue };
        if !is_lan(addr.ip()) {
            drop(tcp);
            continue;
        }
        let (tls, app) = (tls.clone(), app.clone());
        conns.spawn(async move {
            let Ok(Ok(stream)) = tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, tls.accept(tcp)).await else { return };
            let cert_sha256 = {
                let (_, conn) = stream.get_ref();
                conn.peer_certificates().and_then(|c| c.first()).map(|c| sha256(c))
            };
            let svc = app.layer(Extension(PeerCert { addr, cert_sha256 }));
            let _closed = auto::Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(stream), TowerToHyperService::new(svc))
                .await;
        });
    }
    conns.abort_all();
}

/// El sıkışmada görülen istemci sertifikası; cihaz her istekte kayıttan çözülür
/// (bağlantı açıkken iptal edilen cihaz da hemen düşer).
#[derive(Debug, Clone, Copy)]
struct PeerCert {
    addr: SocketAddr,
    cert_sha256: Option<[u8; 32]>,
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/pair", post(pair))
        .route("/manifest", post(manifest))
        .route("/files", post(create).options(options_files))
        .route("/files/{id}", axum::routing::head(head_upload).patch(patch_upload).delete(terminate_upload))
        .with_state(state)
}

struct ApiError(LanError);

impl From<LanError> for ApiError {
    fn from(e: LanError) -> Self {
        Self(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = match &self.0 {
            LanError::PairingFailed => (StatusCode::FORBIDDEN, "PAIRING_FAILED"),
            LanError::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "RATE_LIMITED"),
            LanError::Unauthorized => (StatusCode::UNAUTHORIZED, "UNAUTHORIZED"),
            LanError::BadRequest(_) | LanError::InvalidPayload(_) | LanError::Aborted => {
                (StatusCode::BAD_REQUEST, "BAD_REQUEST")
            }
            LanError::UploadNotFound => (StatusCode::NOT_FOUND, "NOT_FOUND"),
            LanError::UploadGone(_) => (StatusCode::GONE, "GONE"),
            LanError::OffsetMismatch { .. } => (StatusCode::CONFLICT, "OFFSET_MISMATCH"),
            // tus checksum uzantısı: 460 Checksum Mismatch.
            LanError::ChunkChecksum | LanError::FileChecksum => {
                (StatusCode::from_u16(460).unwrap_or(StatusCode::BAD_REQUEST), "CHECKSUM_MISMATCH")
            }
            LanError::TooLarge => (StatusCode::PAYLOAD_TOO_LARGE, "TOO_LARGE"),
            LanError::TooManyUploads => (StatusCode::TOO_MANY_REQUESTS, "TOO_MANY_UPLOADS"),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL"),
        };
        // Eşleşme hatası hangi adımın başarısız olduğunu söylemez.
        let message = if matches!(self.0, LanError::PairingFailed) { String::new() } else { self.0.to_string() };
        let mut res = (status, Json(serde_json::json!({ "error": code, "message": message }))).into_response();
        res.headers_mut().insert("tus-resumable", HeaderValue::from_static(TUS_VERSION));
        res
    }
}

type ApiResult = Result<Response, ApiError>;

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, LanError> + Send + 'static) -> Result<T, LanError> {
    tokio::task::spawn_blocking(f).await?
}

/// mTLS ile gelen, kayıtlı ve iptal edilmemiş cihaz.
async fn authed(st: &AppState, pc: &PeerCert) -> Result<Device, LanError> {
    let id = pc.cert_sha256.and_then(|s| st.0.registry.lookup(&s)).ok_or(LanError::Unauthorized)?;
    let (pool, now, addr) = (st.0.pool.clone(), st.0.clock.now_ms(), pc.addr.ip().to_string());
    let device = blocking(move || {
        let d = pool.read(|c| Ok(store::device(c, id)))??;
        if d.as_ref().is_some_and(|d| !d.revoked) {
            write_tx(&pool, |c| store::touch_device(c, id, &addr, now))?;
        }
        Ok(d)
    })
    .await?;
    device.filter(|d| !d.revoked).ok_or(LanError::Unauthorized)
}

fn header_str<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
    h.get(name).and_then(|v| v.to_str().ok())
}

/// `Tus-Resumable: 1.0.0` yoksa 412 yanıtı.
fn tus_precondition(h: &HeaderMap) -> Option<Response> {
    if header_str(h, "tus-resumable") == Some(TUS_VERSION) {
        return None;
    }
    let mut res = StatusCode::PRECONDITION_FAILED.into_response();
    res.headers_mut().insert("tus-version", HeaderValue::from_static(TUS_VERSION));
    Some(res)
}

fn tus_response(status: StatusCode, headers: &[(&'static str, String)]) -> Response {
    let mut res = status.into_response();
    let h = res.headers_mut();
    h.insert("tus-resumable", HeaderValue::from_static(TUS_VERSION));
    for (k, v) in headers {
        if let Ok(v) = HeaderValue::from_str(v) {
            h.insert(*k, v);
        }
    }
    res
}

async fn pair(
    State(st): State<AppState>,
    Extension(pc): Extension<PeerCert>,
    Json(req): Json<PairRequest>,
) -> ApiResult {
    let s = &st.0;
    if !s.pairing.allow_attempt(pc.addr.ip()) {
        return Err(LanError::RateLimited.into());
    }
    // Belirteç her durumda tüketilir (başarılı ya da başarısız).
    s.pairing.redeem(&req.token)?;
    let name = req.device_name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 80 || !matches!(req.platform.as_str(), "android" | "ios" | "desktop") {
        return Err(LanError::PairingFailed.into());
    }
    let info = PairingRequestInfo { device_name: name.clone(), platform: req.platform.clone(), addr: pc.addr };
    let approved = tokio::time::timeout(APPROVAL_TIMEOUT, s.approver.approve(info)).await.unwrap_or(false);
    if !approved {
        return Err(LanError::PairingFailed.into());
    }
    let now = s.clock.now_ms();
    let issued = s.identity.sign_client_csr(&req.csr_pem, &name, now).map_err(|_| LanError::PairingFailed)?;
    let (pool, lib, cert, platform) = (s.pool.clone(), s.cfg.library_id, issued.clone(), req.platform);
    let device_id =
        blocking(move || write_tx(&pool, |c| store::insert_device(c, &name, &platform, &cert, lib, now))).await?;
    s.registry.insert(issued.cert_sha256, device_id);
    let body = PairResponse {
        device_id,
        client_cert_pem: issued.cert_pem,
        ca_cert_pem: s.identity.ca_cert_pem().to_owned(),
        install_id: s.identity.install_id().to_owned(),
        server_name: crate::SERVER_NAME.to_owned(),
    };
    Ok((StatusCode::CREATED, Json(body)).into_response())
}

async fn manifest(
    State(st): State<AppState>,
    Extension(pc): Extension<PeerCert>,
    Json(items): Json<Vec<ManifestItem>>,
) -> ApiResult {
    let device = authed(&st, &pc).await?;
    let out: Vec<ManifestEntry> = st.0.uploads.manifest(&device, items).await?;
    Ok(Json(out).into_response())
}

async fn options_files() -> Response {
    tus_response(
        StatusCode::NO_CONTENT,
        &[
            ("tus-version", TUS_VERSION.to_owned()),
            ("tus-extension", "creation,checksum,termination".to_owned()),
            ("tus-checksum-algorithm", "blake3".to_owned()),
            ("tus-max-size", i64::MAX.to_string()),
        ],
    )
}

async fn create(State(st): State<AppState>, Extension(pc): Extension<PeerCert>, headers: HeaderMap) -> ApiResult {
    if let Some(r) = tus_precondition(&headers) {
        return Ok(r);
    }
    let device = authed(&st, &pc).await?;
    let meta = UploadMeta::from_tus(header_str(&headers, "upload-length"), header_str(&headers, "upload-metadata"))?;
    let status = |s: &str| ("loupe-sync-status", s.to_owned());
    Ok(match st.0.uploads.create(&device, meta).await? {
        CreateOutcome::Created { upload_id, offset, .. } => tus_response(
            StatusCode::CREATED,
            &[(header::LOCATION.as_str(), format!("/files/{upload_id}")), ("upload-offset", offset.to_string())],
        ),
        CreateOutcome::AlreadySynced { media_id } => {
            let mut r = Json(serde_json::json!({ "status": "already_synced", "media_id": media_id.map(|m| m.0) }))
                .into_response();
            add_headers(&mut r, &[status("already-synced")]);
            r
        }
        CreateOutcome::Skipped => {
            let mut r = Json(serde_json::json!({ "status": "skipped" })).into_response();
            add_headers(&mut r, &[status("skipped")]);
            r
        }
        CreateOutcome::Conflict { previous } => {
            let mut r = (
                StatusCode::CONFLICT,
                Json(serde_json::json!({ "status": "conflict", "previous_media_id": previous.map(|m| m.0) })),
            )
                .into_response();
            add_headers(&mut r, &[status("conflict")]);
            r
        }
    })
}

fn add_headers(r: &mut Response, extra: &[(&'static str, String)]) {
    let h = r.headers_mut();
    h.insert("tus-resumable", HeaderValue::from_static(TUS_VERSION));
    for (k, v) in extra {
        if let Ok(v) = HeaderValue::from_str(v) {
            h.insert(*k, v);
        }
    }
}

async fn head_upload(
    State(st): State<AppState>,
    Extension(pc): Extension<PeerCert>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult {
    if let Some(r) = tus_precondition(&headers) {
        return Ok(r);
    }
    let device = authed(&st, &pc).await?;
    let (offset, length) = st.0.uploads.offset(device.id, &id).await?;
    Ok(tus_response(
        StatusCode::OK,
        &[
            ("upload-offset", offset.to_string()),
            ("upload-length", length.to_string()),
            (header::CACHE_CONTROL.as_str(), "no-store".to_owned()),
        ],
    ))
}

fn parse_checksum(v: Option<&str>) -> Result<Option<[u8; 32]>, LanError> {
    let Some(v) = v else { return Ok(None) };
    let (algo, b64) = v.trim().split_once(' ').ok_or_else(|| LanError::BadRequest("Upload-Checksum".into()))?;
    if !algo.eq_ignore_ascii_case("blake3") {
        return Err(LanError::BadRequest(format!("desteklenmeyen checksum algoritması: {algo}")));
    }
    let raw = STANDARD.decode(b64.trim()).map_err(|_| LanError::BadRequest("Upload-Checksum base64".into()))?;
    <[u8; 32]>::try_from(raw.as_slice()).map(Some).map_err(|_| LanError::BadRequest("Upload-Checksum boyu".into()))
}

async fn patch_upload(
    State(st): State<AppState>,
    Extension(pc): Extension<PeerCert>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult {
    if let Some(r) = tus_precondition(&headers) {
        return Ok(r);
    }
    let device = authed(&st, &pc).await?;
    if header_str(&headers, "content-type") != Some("application/offset+octet-stream") {
        return Ok(tus_response(StatusCode::UNSUPPORTED_MEDIA_TYPE, &[]));
    }
    let offset: u64 = header_str(&headers, "upload-offset")
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| LanError::BadRequest("Upload-Offset gerekli".into()))?;
    let checksum = parse_checksum(header_str(&headers, "upload-checksum"))?;
    let out = st.0.uploads.patch(device.id, &id, offset, checksum, body.into_data_stream()).await?;
    let mut extra = vec![("upload-offset", out.offset.to_string())];
    if let Some(m) = out.completed {
        extra.push(("loupe-sync-status", "verified".to_owned()));
        extra.push(("loupe-media-id", m.0.to_string()));
    }
    Ok(tus_response(StatusCode::NO_CONTENT, &extra))
}

async fn terminate_upload(
    State(st): State<AppState>,
    Extension(pc): Extension<PeerCert>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult {
    if let Some(r) = tus_precondition(&headers) {
        return Ok(r);
    }
    let device = authed(&st, &pc).await?;
    st.0.uploads.terminate(device.id, &id).await?;
    Ok(tus_response(StatusCode::NO_CONTENT, &[]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_lan_sources_are_accepted() {
        for ok in ["192.168.1.5", "10.0.0.2", "172.16.4.4", "127.0.0.1", "169.254.1.1", "fd12::1", "fe80::1", "::1"] {
            assert!(is_lan(ok.parse().unwrap()), "{ok}");
        }
        for no in ["8.8.8.8", "2001:db8::1", "100.64.0.1", "::ffff:8.8.8.8"] {
            assert!(!is_lan(no.parse().unwrap()), "{no}");
        }
        assert!(is_lan("::ffff:192.168.1.9".parse().unwrap()));
    }

    #[test]
    fn checksum_header_is_parsed() {
        let b64 = STANDARD.encode([5u8; 32]);
        assert_eq!(parse_checksum(Some(&format!("blake3 {b64}"))).unwrap(), Some([5; 32]));
        assert!(parse_checksum(Some(&format!("sha1 {b64}"))).is_err());
        assert_eq!(parse_checksum(None).unwrap(), None);
    }
}

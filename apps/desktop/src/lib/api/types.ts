/**
 * IPC sözleşmesi: crates/loupe-app'teki serde DTO'larının birebir TypeScript
 * karşılığı (camelCase). Rust tarafında bir alan değişirse burası da değişir;
 * apps/desktop/src-tauri/tests/ipc.rs alan adlarını doğrular.
 */

export type View =
  | { kind: "all" }
  | { kind: "favorites" }
  | { kind: "library"; id: number }
  | { kind: "events" }
  | { kind: "people" }
  | { kind: "places" }
  | { kind: "smart_album"; id: number }
  | { kind: "search"; query: SearchQuery }
  | { kind: "person"; id: number }
  | { kind: "cluster"; id: number }
  | { kind: "event"; id: number };

export type ViewKey = "all" | "favorites" | "events" | "people" | "places";

export type Grouping = "day" | "month";

export interface TimelineRequest {
  view: View;
  grouping: Grouping;
  offset: number;
  limit: number;
}

export interface Section {
  key: string;
  title: string;
  subtitle: string | null;
  /** Görünümdeki ilk öğenin sırası. */
  start: number;
  count: number;
}

export type ColorLabel = "red" | "yellow" | "green" | "blue" | "purple";
export type SyncState = "local" | "queued" | "synced" | "error" | "offline";

export interface TileItem {
  id: number;
  hash: string;
  aspect: number;
  rating: number;
  label: ColorLabel | null;
  favorite: boolean;
  flag: "none" | "pick" | "reject";
  raw: string | null;
  video: boolean;
  sync: SyncState;
  takenAtUtc: number;
  tzOffsetMin: number | null;
}

export interface TimelineSlice {
  total: number;
  offset: number;
  sections: Section[];
  items: TileItem[];
}

export interface Histogram {
  bins: number;
  luma: number[];
  red: number[];
  green: number[];
  blue: number[];
  shadowsClippedPermille: number;
  highlightsClippedPermille: number;
  source: "sm" | "md";
}

export interface GpsDto {
  lat: number;
  lon: number;
  altM: number | null;
  dms: string;
}

export interface PlaceDto {
  display: string;
  locality: string | null;
  admin2: string | null;
  admin1: string | null;
  country: string;
  countryCode: string;
}

export interface SyncTargetDto {
  target: string;
  provider: string;
  state: "local_only" | "queued" | "uploading" | "synced" | "conflict" | "error" | "remote_deleted";
  encryption: "none" | "age";
  keyFingerprint: string | null;
  remoteChecksumAlgo: string | null;
  uploadedAt: number | null;
  lastVerifiedAt: number | null;
  lastErrorCode: string | null;
}

export interface InspectorDetail {
  id: number;
  hash: string;
  fileName: string;
  originalFilename: string;
  relPath: string;
  path: string | null;
  libraryName: string;
  libraryOnline: boolean;
  fileType: string;
  rawFormat: string | null;
  mime: string;
  sizeBytes: number;
  width: number | null;
  height: number | null;
  megapixels: number | null;
  takenAtUtc: number;
  tzOffsetMin: number | null;
  takenLocal: string;
  takenSource: string;
  cameraMake: string | null;
  cameraModel: string | null;
  lensModel: string | null;
  focalLength: string | null;
  aperture: string | null;
  exposure: string | null;
  iso: number | null;
  gps: GpsDto | null;
  place: PlaceDto | null;
  rating: number;
  label: ColorLabel | null;
  favorite: boolean;
  flag: string;
  sidecarDirty: boolean;
  verifyState: string;
  lastVerifiedAt: number;
  importedAt: number;
  placement: string;
  sync: SyncTargetDto[];
  histogram: Histogram | null;
}

export interface CullRequest {
  ids: number[];
  rating?: number;
  /** `none` etiketi kaldırır. */
  label?: ColorLabel | "none";
  favorite?: boolean;
}

export interface CullResult {
  id: number;
  rating: number;
  label: ColorLabel | null;
  favorite: boolean;
  sidecar: "written" | "pending" | "failed";
}

export interface LibraryDto {
  id: number;
  uuid: string;
  name: string;
  kind: string;
  state: "online" | "offline" | "readonly" | "error";
  stateReason: string | null;
  root: string | null;
  volumeUuid: string | null;
  volumeLabel: string | null;
  fsType: string | null;
  managed: boolean;
  primary: boolean;
  mediaCount: number;
  totalBytes: number | null;
  freeBytes: number | null;
}

export type ImportMode = "copy" | "move" | "hardlink";

export interface ImportRequest {
  libraryId: number;
  source: string;
  mode: ImportMode;
}

export interface ImportSessionDto {
  id: number;
  libraryId: number;
  mode: string;
  source: string;
  state: "scanning" | "running" | "done" | "failed" | "cancelled" | "paused";
  totalFiles: number;
  totalBytes: number;
  doneFiles: number;
  doneBytes: number;
  skippedDup: number;
  failedFiles: number;
  startedAt: number;
  finishedAt: number | null;
  lastError: string | null;
}

export interface AddedLibrary {
  library: LibraryDto;
  scan: ImportSessionDto | null;
}

export interface SmartAlbumDto {
  id: number;
  name: string;
  pinned: boolean;
}

export interface SidebarDto {
  libraries: LibraryDto[];
  views: { key: ViewKey; count: number }[];
  smartAlbums: SmartAlbumDto[];
}

export interface LanStatusDto {
  state: "off" | "listening" | "no_library" | "error";
  port: number | null;
  pairedDevices: number;
  activeUploads: number;
  bytesPerSec: number;
  bytesExpected: number;
  bytesReceived: number;
  /** Kalan süre (sn); hız bilinmiyorsa `null`. */
  etaSecs: number | null;
  verifiedFiles: number;
  skipped: number;
  conflicts: number;
  failed: number;
  error: string | null;
}

export interface CloudTargetStatusDto {
  id: number;
  name: string;
  provider: string;
  encrypted: boolean;
  state: "idle" | "uploading" | "needs_setup" | "error" | "disabled";
  pending: number;
  synced: number;
  lastError: string | null;
}

export interface StatusDto {
  totalMedia: number;
  queue: { pending: number; running: number; failed: number; dead: number };
  workers: string;
  importsActive: number;
  librariesOnline: number;
  librariesOffline: number;
  freeBytes: number | null;
  lan: LanStatusDto;
  cloud: CloudTargetStatusDto[];
  ai: AiStatusDto;
  /** Süren "Kütüphaneyi Doğrula" sayısı. */
  verifying: number;
}

/** Rust `AppError` serileştirmesi. */
export interface AppErrorDto {
  code: string;
  message: string;
}

export type ThumbSize = "sm" | "md" | "lg";
/** `orig`: tam çözünürlük (RAW gömülü önizleme ya da yönü düzeltilmiş özgün). */
export type ImageSize = ThumbSize | "orig";

// ───────────────────────────── arama paleti ─────────────────────────────

/**
 * `loupe_search::SearchQuery` (alanlar snake_case). Arayüz bunu kurmaz:
 * paletin döndürdüğü sorgu görünüme ve akıllı albüme olduğu gibi geçer.
 */
export interface SearchQuery {
  persons?: { ids: number[]; mode?: "all" | "any" } | null;
  place?: Record<string, unknown> | null;
  time?: { from_utc?: number | null; to_utc?: number | null; year?: number | null; month?: number | null; day?: number | null } | null;
  event_id?: number | null;
  min_rating?: number | null;
  favorite?: boolean | null;
  color_labels?: string[];
  camera_model?: string | null;
  lens_model?: string | null;
  text?: string | null;
  limit?: number | null;
  sort?: "relevance" | "taken_desc" | "taken_asc" | "rating_desc";
}

export type ChipKind = "person" | "place" | "event" | "rating" | "color" | "camera" | "lens" | "year" | "favorite";

export interface Chip {
  kind: ChipKind;
  raw: string;
  label: string;
  ok: boolean;
  error: string | null;
}

export interface Suggestion {
  kind: ChipKind;
  label: string;
  /** Son belirtecin yerine yazılacak metin. */
  insert: string;
  detail: string | null;
}

export interface PaletteRequest {
  input: string;
  preview?: number;
}

export interface PaletteResult {
  query: SearchQuery;
  chips: Chip[];
  suggestions: Suggestion[];
  text: string | null;
  searched: boolean;
  total: number;
  items: TileItem[];
  semantic: boolean;
  elapsedMs: number;
}

// ───────────────────────────── kişiler ─────────────────────────────

export interface FaceRef {
  faceId: number;
  mediaId: number;
  hash: string;
  aspect: number;
  /** Küçük resme göre 0..1. */
  bbox: { x: number; y: number; w: number; h: number };
}

export interface PersonCard {
  personId: number | null;
  clusterId: number | null;
  name: string | null;
  faceCount: number;
  photoCount: number;
  cover: FaceRef | null;
}

export interface PeopleDto {
  named: PersonCard[];
  unnamed: PersonCard[];
  ignored: number;
}

// ───────────────────────────── etkinlikler ─────────────────────────────

export interface EventCard {
  id: number;
  title: string;
  range: string;
  startUtc: number;
  endUtc: number;
  count: number;
  coverHash: string | null;
  coverAspect: number;
  place: string | null;
  /** Kullanıcı kilidi: yeniden kümeleme dokunmaz. */
  locked: boolean;
  origin: "auto" | "manual";
}

// ───────────────────────────── yapay zekâ ─────────────────────────────

export type ResourceMode = "quiet" | "balanced" | "full";

export interface AiStatusDto {
  mode: "off" | "mock" | "onnx";
  reason: string | null;
  semantic: boolean;
  paused: boolean;
  resourceMode: ResourceMode;
  pending: { faces: number; clip: number; clustering: number };
  facesDone: number;
  clipDone: number;
  images: number;
}

export type ModelFileState = "missing" | "verified" | "corrupt" | "unpinned" | "incomplete";

export interface ModelFileDto {
  file: string;
  /** `face_detect` | `face_embed` | `clip_image` | `clip_text` | `tokenizer` | `runtime` */
  role: string;
  state: ModelFileState;
  size: number | null;
  sha256: string | null;
  license: string | null;
}

export interface ModelJobDto {
  running: boolean;
  source: string | null;
  phase: "resolve" | "download" | "verify" | "extract" | "done" | null;
  done: number;
  total: number;
  installed: string[];
  failed: { source: string; files: string[]; reason: string }[];
  error: string | null;
}

export interface ModelsDto {
  dir: string;
  files: ModelFileDto[];
  ready: boolean;
  runtime: boolean;
  onnxBuild: boolean;
  restartNeeded: boolean;
  job: ModelJobDto;
}

// ───────────────────────────── Wi-Fi eşleştirme ─────────────────────────────

export interface PairingQrDto {
  /** `https://<adres>:<port>/m#t=…`: kamera tarayıcıda açar. Tek kullanımlık belirteç içerir. */
  uri: string;
  /** Yalnızca `<rect>` ve `<path>`. */
  svg: string;
  expiresAtMs: number;
  ttlSecs: number;
  addrs: string[];
  port: number;
  name: string;
  /** Sunucu sertifikasının SHA-256 parmak izi (`AB:CD:…`). */
  fingerprint: string;
}

export type ConflictPolicy = "ask_user" | "replace_with_newer" | "keep_both" | "skip";

export interface DeviceDto {
  id: number;
  name: string;
  platform: string;
  conflictPolicy: ConflictPolicy;
  pairedAt: number;
  lastSeenAt: number | null;
  lastAddress: string | null;
  revoked: boolean;
  /** `app` (mTLS) | `web` (tarayıcı oturumu). */
  kind: "app" | "web";
  received: number;
}

// ───────────────────────────── bulut ─────────────────────────────

export type CloudProvider = "s3" | "b2" | "r2" | "wasabi" | "webdav" | "local";

export interface CloudTargetInput {
  name: string;
  provider: CloudProvider;
  /** `endpoint`, `bucket`, `region`, `url`, `path`. */
  config: Record<string, string>;
  /** Güncellemede `null`: değişmez. */
  user?: string | null;
  secret?: string | null;
  keyPrefix?: string;
  encrypted?: boolean;
  bandwidthMbps?: number | null;
  enabled?: boolean;
}

export interface CloudTargetDto {
  id: number;
  name: string;
  provider: CloudProvider;
  config: Record<string, unknown>;
  keyPrefix: string;
  encrypted: boolean;
  bandwidthMbps: number | null;
  enabled: boolean;
  hasCredentials: boolean;
  user: string | null;
}

export interface EncryptionDto {
  configured: boolean;
  recipient: string | null;
  fingerprint: string | null;
  deviceKey: boolean;
  createdAt: number | null;
}

export interface CloudSettingsDto {
  targets: CloudTargetDto[];
  globalBandwidthMbps: number | null;
  encryption: EncryptionDto;
  secretBackend: "secret-service" | "file";
}

export interface RecoveryDraftDto {
  prefix: string;
  groups: string[];
  secret: string;
  recipient: string;
  /** Geri yazılacak grup numaraları (0'dan). */
  challenge: number[];
  replaces: boolean;
}

export interface RecoveryAnswer {
  group: number;
  value: string;
}

// ───────────────────────────── doğrulama ─────────────────────────────

export interface VerifyRunDto {
  id: number;
  libraryId: number;
  library: string;
  trigger: "manual" | "scheduled" | "remount";
  state: "running" | "paused" | "done" | "cancelled" | "failed";
  total: number;
  checked: number;
  ok: number;
  missing: number;
  mismatched: number;
  unreadable: number;
  bytesRead: number;
  startedAt: number;
  finishedAt: number | null;
}

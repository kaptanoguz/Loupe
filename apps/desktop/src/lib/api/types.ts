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
  | { kind: "smart_album"; id: number };

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
  verifiedFiles: number;
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
}

/** Rust `AppError` serileştirmesi. */
export interface AppErrorDto {
  code: string;
  message: string;
}

export type ThumbSize = "sm" | "md" | "lg";

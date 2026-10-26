/**
 * Arayüzün çekirdekle konuştuğu tek yer. Tauri içinde `invoke`, tarayıcıda
 * (vite dev, testler) bellek içi sahte katalog kullanılır; bileşenler farkı
 * bilmez.
 */
import { invoke, isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { MockBackend } from "./mock";
import type {
  AddedLibrary,
  AiStatusDto,
  AlbumDto,
  ModelsDto,
  AppErrorDto,
  AutoTone,
  EditRecipe,
  EditStateDto,
  ExportDto,
  ExternalEditorDto,
  CloudSettingsDto,
  CloudTargetInput,
  CloudTargetStatusDto,
  ConflictPolicy,
  CullRequest,
  CullResult,
  DeviceDto,
  EncryptionDto,
  EventCard,
  FaceRef,
  ImageSize,
  ImportRequest,
  ImportSessionDto,
  IphoneDto,
  TrashResultDto,
  InspectorDetail,
  LibraryDto,
  LibraryPatch,
  PairingQrDto,
  PaletteRequest,
  PaletteResult,
  PeopleDto,
  RecoveryAnswer,
  RecoveryDraftDto,
  ResourceMode,
  SearchQuery,
  SidebarDto,
  SmartAlbumDto,
  StatusDto,
  TimelineRequest,
  TimelineSlice,
  VerifyRunDto,
} from "./types";

export interface Backend {
  pickOrAddLibraryRoot(path?: string, name?: string, managed?: boolean): Promise<AddedLibrary | null>;
  pickImportSource(): Promise<string | null>;
  findIphone(): Promise<IphoneDto[]>;
  removeLibrary(id: number): Promise<number>;
  trashMedia(ids: number[]): Promise<TrashResultDto>;
  listLibraries(): Promise<LibraryDto[]>;
  refreshLibraries(): Promise<LibraryDto[]>;
  startImport(req: ImportRequest): Promise<ImportSessionDto>;
  updateLibrary(id: number, patch: LibraryPatch): Promise<LibraryDto>;
  /** Medyayı sistemin varsayılan uygulamasında açar (video oynatıcı). */
  openMedia(id: number): Promise<void>;
  /** Dosyanın klasörünü dosya yöneticisinde açar. Döner: klasör yolu. */
  revealMedia(id: number): Promise<string>;
  /** Pencereyi tam ekrana alır ya da çıkarır (slayt gösterisi). */
  setFullscreen(on: boolean): Promise<void>;
  importStatus(id: number): Promise<ImportSessionDto>;
  recentImports(limit?: number): Promise<ImportSessionDto[]>;
  cancelImport(id: number): Promise<boolean>;
  timelineSlice(req: TimelineRequest): Promise<TimelineSlice>;
  prioritizeViewport(ids: number[]): Promise<number>;
  inspectorDetail(id: number): Promise<InspectorDetail>;
  setRatingLabelFavorite(req: CullRequest): Promise<CullResult[]>;
  sidebar(): Promise<SidebarDto>;
  status(): Promise<StatusDto>;

  searchPalette(req: PaletteRequest): Promise<PaletteResult>;
  saveSmartAlbum(name: string, query: SearchQuery, pinned?: boolean): Promise<SmartAlbumDto>;
  deleteSmartAlbum(id: number): Promise<void>;
  createAlbum(name: string): Promise<AlbumDto>;
  renameAlbum(id: number, name: string): Promise<AlbumDto>;
  deleteAlbum(id: number): Promise<void>;
  /** Döner: yeni eklenen sayısı (zaten albümde olanlar atlanır). */
  addToAlbum(id: number, ids: number[]): Promise<number>;
  removeFromAlbum(id: number, ids: number[]): Promise<number>;

  listPeople(): Promise<PeopleDto>;
  personFaces(target: { person?: number; cluster?: number }, limit?: number): Promise<FaceRef[]>;
  /** Kişi kimliği döner (aynı adlı kişi varsa ona katılır). */
  nameCluster(cluster: number, name: string): Promise<number>;
  renamePerson(person: number, name: string): Promise<number>;
  mergeClusters(into: number, from: number[]): Promise<void>;
  mergePersons(into: number, from: number[]): Promise<void>;
  moveFace(face: number, cluster: number): Promise<void>;
  assignFace(face: number, person: number): Promise<void>;
  /** Yüzün yeni (tek elemanlı) kümesi döner. */
  detachFace(face: number): Promise<number>;
  rejectFace(face: number): Promise<void>;
  ignoreCluster(cluster: number, ignored: boolean): Promise<void>;

  listEvents(): Promise<EventCard[]>;
  renameEvent(id: number, title: string): Promise<EventCard>;
  excludeFromEvent(id: number, media: number[]): Promise<EventCard>;
  setEventLocked(id: number, locked: boolean): Promise<EventCard>;

  aiStatus(): Promise<AiStatusDto>;
  setPaused(paused: boolean): Promise<AiStatusDto>;
  setResourceMode(mode: ResourceMode): Promise<AiStatusDto>;
  modelsStatus(): Promise<ModelsDto>;
  /** Arka planda indirir; ilerleme `modelsStatus` ile yoklanır. */
  modelsInstall(): Promise<ModelsDto>;
  modelsCancel(): Promise<ModelsDto>;

  pairingQr(): Promise<PairingQrDto>;
  cancelPairing(): Promise<void>;
  listDevices(): Promise<DeviceDto[]>;
  revokeDevice(id: number): Promise<DeviceDto[]>;
  setConflictPolicy(id: number, policy: ConflictPolicy): Promise<DeviceDto[]>;

  cloudSettings(): Promise<CloudSettingsDto>;
  saveCloudTarget(id: number | null, input: CloudTargetInput): Promise<CloudSettingsDto>;
  setCloudTargetEnabled(id: number, enabled: boolean): Promise<CloudSettingsDto>;
  deleteCloudTarget(id: number): Promise<CloudSettingsDto>;
  setGlobalBandwidth(mbps: number | null): Promise<CloudSettingsDto>;
  startCloudSync(): Promise<CloudTargetStatusDto[]>;

  generateRecoveryKey(): Promise<RecoveryDraftDto>;
  /** Kaydetme penceresi; seçilen yol ya da vazgeçildiyse `null`. */
  exportRecoveryKey(): Promise<string | null>;
  confirmRecoveryKey(answers: RecoveryAnswer[]): Promise<EncryptionDto>;
  cancelRecoveryKey(): Promise<void>;
  checkRecoveryKey(key: string): Promise<boolean>;

  /** Başlatılan kütüphane kimlikleri. */
  verifyLibrary(library?: number): Promise<number[]>;
  cancelVerify(library?: number): Promise<number>;
  verifyRuns(limit?: number): Promise<VerifyRunDto[]>;

  editState(id: number): Promise<EditStateDto>;
  /** Tarifi kaydeder (her şey sıfırsa orijinale döner). */
  saveEdit(id: number, recipe: EditRecipe): Promise<EditStateDto>;
  revertEdit(id: number): Promise<EditStateDto>;
  /** Tek tıkla iyileştirme önerisi; kaydetmez. */
  autoTone(id: number): Promise<AutoTone>;
  /** Kaydetme penceresi ve JPEG; vazgeçildiyse `null`. */
  exportEdit(id: number): Promise<ExportDto | null>;
  externalEditors(): Promise<ExternalEditorDto[]>;
  openInEditor(id: number, editor: ExternalEditorDto["id"]): Promise<void>;

  /**
   * `<img src>` için küçük resim (ya da `orig`: tam çözünürlük) adresi.
   * `edit` > 0: düzenlenmiş sürüm (revizyon adreste, önbellek bozulmaz).
   */
  thumbUrl(size: ImageSize, hash: string, edit?: number): string;
}

/** Çekirdekten gelen hata: `code` ile dallanılır, `message` kullanıcıya gösterilir. */
export class BackendError extends Error {
  readonly code: string;
  constructor(dto: AppErrorDto) {
    super(dto.message);
    this.code = dto.code;
  }
}

function isAppError(e: unknown): e is AppErrorDto {
  return typeof e === "object" && e !== null && "code" in e && "message" in e;
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw isAppError(e) ? new BackendError(e) : new BackendError({ code: "IPC", message: String(e) });
  }
}

export const tauriBackend: Backend = {
  pickOrAddLibraryRoot: (path, name, managed) => call("pick_or_add_library_root", { path, name, managed }),
  pickImportSource: () => call("pick_import_source"),
  findIphone: () => call("find_iphone"),
  removeLibrary: (id) => call("remove_library", { id }),
  trashMedia: (ids) => call("trash_media", { ids }),
  listLibraries: () => call("list_libraries"),
  refreshLibraries: () => call("refresh_libraries"),
  startImport: (req) => call("start_import", { req }),
  updateLibrary: (id, patch) => call("update_library", { id, patch }),
  openMedia: (id) => call("open_media", { id }),
  revealMedia: (id) => call("reveal_media", { id }),
  setFullscreen: async (on) => {
    try {
      await getCurrentWindow().setFullscreen(on);
    } catch {
      // Tam ekran olmadan da gösteri sürer.
    }
  },
  importStatus: (id) => call("import_status", { id }),
  recentImports: (limit) => call("recent_imports", { limit }),
  cancelImport: (id) => call("cancel_import", { id }),
  timelineSlice: (req) => call("get_timeline_slice", { req }),
  prioritizeViewport: (ids) => call("prioritize_viewport", { ids }),
  inspectorDetail: (id) => call("get_media_inspector_detail", { id }),
  setRatingLabelFavorite: (req) => call("set_rating_label_favorite", { req }),
  sidebar: () => call("get_sidebar"),
  status: () => call("get_status"),

  searchPalette: (req) => call("search_palette", { req }),
  saveSmartAlbum: (name, query, pinned) => call("save_smart_album", { name, query, pinned }),
  deleteSmartAlbum: (id) => call("delete_smart_album", { id }),
  createAlbum: (name) => call("create_album", { name }),
  renameAlbum: (id, name) => call("rename_album", { id, name }),
  deleteAlbum: (id) => call("delete_album", { id }),
  addToAlbum: (id, ids) => call("add_to_album", { id, ids }),
  removeFromAlbum: (id, ids) => call("remove_from_album", { id, ids }),

  listPeople: () => call("list_people"),
  personFaces: (target, limit) => call("person_faces", { person: target.person, cluster: target.cluster, limit }),
  nameCluster: (cluster, name) => call("name_cluster", { cluster, name }),
  renamePerson: (person, name) => call("rename_person", { person, name }),
  mergeClusters: (into, from) => call("merge_clusters", { into, from }),
  mergePersons: (into, from) => call("merge_persons", { into, from }),
  moveFace: (face, cluster) => call("move_face", { face, cluster }),
  assignFace: (face, person) => call("assign_face", { face, person }),
  detachFace: (face) => call("detach_face", { face }),
  rejectFace: (face) => call("reject_face", { face }),
  ignoreCluster: (cluster, ignored) => call("ignore_cluster", { cluster, ignored }),

  listEvents: () => call("list_events"),
  renameEvent: (id, title) => call("rename_event", { id, title }),
  excludeFromEvent: (id, media) => call("exclude_from_event", { id, media }),
  setEventLocked: (id, locked) => call("set_event_locked", { id, locked }),

  aiStatus: () => call("ai_status"),
  setPaused: (paused) => call("set_paused", { paused }),
  setResourceMode: (mode) => call("set_resource_mode", { mode }),
  modelsStatus: () => call("models_status"),
  modelsInstall: () => call("models_install"),
  modelsCancel: () => call("models_cancel"),

  pairingQr: () => call("pairing_qr"),
  cancelPairing: () => call("cancel_pairing"),
  listDevices: () => call("list_devices"),
  revokeDevice: (id) => call("revoke_device", { id }),
  setConflictPolicy: (id, policy) => call("set_conflict_policy", { id, policy }),

  cloudSettings: () => call("cloud_settings"),
  saveCloudTarget: (id, input) => call("save_cloud_target", { id, input }),
  setCloudTargetEnabled: (id, enabled) => call("set_cloud_target_enabled", { id, enabled }),
  deleteCloudTarget: (id) => call("delete_cloud_target", { id }),
  setGlobalBandwidth: (mbps) => call("set_global_bandwidth", { mbps }),
  startCloudSync: () => call("start_cloud_sync"),

  generateRecoveryKey: () => call("generate_recovery_key"),
  exportRecoveryKey: () => call("export_recovery_key"),
  confirmRecoveryKey: (answers) => call("confirm_recovery_key", { answers }),
  cancelRecoveryKey: () => call("cancel_recovery_key"),
  checkRecoveryKey: (key) => call("check_recovery_key", { key }),

  verifyLibrary: (library) => call("verify_library", { library }),
  cancelVerify: (library) => call("cancel_verify", { library }),
  verifyRuns: (limit) => call("verify_runs", { limit }),
  // WebKitGTK özel şemayı olduğu gibi yükler (Windows biçimi thumb.localhost
  // çekirdekte de çözülür, Linux hedefimiz).
  editState: (id) => call("get_edit", { id }),
  saveEdit: (id, recipe) => call("save_edit", { id, recipe }),
  revertEdit: (id) => call("revert_edit", { id }),
  autoTone: (id) => call("auto_tone", { id }),
  exportEdit: (id) => call("export_edit", { id }),
  externalEditors: () => call("external_editors"),
  openInEditor: (id, editor) => call("open_in_editor", { id, editor }),
  thumbUrl: (size, hash, edit) => `thumb://${size}/${hash}${edit ? `?e=${edit}` : ""}`,
};

export function createBackend(): Backend {
  return isTauri() ? tauriBackend : new MockBackend();
}

/**
 * Arayüzün çekirdekle konuştuğu tek yer. Tauri içinde `invoke`, tarayıcıda
 * (vite dev, testler) bellek içi sahte katalog kullanılır; bileşenler farkı
 * bilmez.
 */
import { invoke, isTauri } from "@tauri-apps/api/core";

import { MockBackend } from "./mock";
import type {
  AddedLibrary,
  AppErrorDto,
  CullRequest,
  CullResult,
  ImportRequest,
  ImportSessionDto,
  InspectorDetail,
  LibraryDto,
  SidebarDto,
  StatusDto,
  ThumbSize,
  TimelineRequest,
  TimelineSlice,
} from "./types";

export interface Backend {
  pickOrAddLibraryRoot(path?: string, name?: string, managed?: boolean): Promise<AddedLibrary | null>;
  pickImportSource(): Promise<string | null>;
  listLibraries(): Promise<LibraryDto[]>;
  refreshLibraries(): Promise<LibraryDto[]>;
  startImport(req: ImportRequest): Promise<ImportSessionDto>;
  importStatus(id: number): Promise<ImportSessionDto>;
  recentImports(limit?: number): Promise<ImportSessionDto[]>;
  cancelImport(id: number): Promise<boolean>;
  timelineSlice(req: TimelineRequest): Promise<TimelineSlice>;
  prioritizeViewport(ids: number[]): Promise<number>;
  inspectorDetail(id: number): Promise<InspectorDetail>;
  setRatingLabelFavorite(req: CullRequest): Promise<CullResult[]>;
  sidebar(): Promise<SidebarDto>;
  status(): Promise<StatusDto>;
  /** `<img src>` için küçük resim adresi. */
  thumbUrl(size: ThumbSize, hash: string): string;
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
  listLibraries: () => call("list_libraries"),
  refreshLibraries: () => call("refresh_libraries"),
  startImport: (req) => call("start_import", { req }),
  importStatus: (id) => call("import_status", { id }),
  recentImports: (limit) => call("recent_imports", { limit }),
  cancelImport: (id) => call("cancel_import", { id }),
  timelineSlice: (req) => call("get_timeline_slice", { req }),
  prioritizeViewport: (ids) => call("prioritize_viewport", { ids }),
  inspectorDetail: (id) => call("get_media_inspector_detail", { id }),
  setRatingLabelFavorite: (req) => call("set_rating_label_favorite", { req }),
  sidebar: () => call("get_sidebar"),
  status: () => call("get_status"),
  // WebKitGTK özel şemayı olduğu gibi yükler (Windows biçimi thumb.localhost
  // çekirdekte de çözülür, Linux hedefimiz).
  thumbUrl: (size, hash) => `thumb://${size}/${hash}`,
};

export function createBackend(): Backend {
  return isTauri() ? tauriBackend : new MockBackend();
}

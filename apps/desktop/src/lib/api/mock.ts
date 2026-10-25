/**
 * Tarayıcıda (vite dev) ve testlerde kullanılan bellek içi katalog.
 * Deterministik: aynı tohum aynı kataloğu üretir. `?mock=100000` ile büyük
 * kütüphane davranışı (sanal kaydırma, düzen süresi) Tauri olmadan denenir.
 */
import type { Backend } from "./backend";
import type {
  AddedLibrary,
  ColorLabel,
  CullRequest,
  CullResult,
  ImportRequest,
  ImportSessionDto,
  InspectorDetail,
  LibraryDto,
  Section,
  SidebarDto,
  StatusDto,
  SyncState,
  ThumbSize,
  TileItem,
  TimelineRequest,
  TimelineSlice,
} from "./types";

const MONTHS = ["Ocak", "Şubat", "Mart", "Nisan", "Mayıs", "Haziran", "Temmuz", "Ağustos", "Eylül", "Ekim", "Kasım", "Aralık"];
const DAYS = ["Pazar", "Pazartesi", "Salı", "Çarşamba", "Perşembe", "Cuma", "Cumartesi"];
const ASPECTS = [1.5, 1.5, 1.5, 0.667, 1.333, 1, 1.778, 0.75, 3.2];
const RAWS = [null, null, null, "CR2", "NEF", "ARW", "DNG"];
const SYNCS: SyncState[] = ["synced", "synced", "local", "queued", "synced", "error"];
const LABELS: (ColorLabel | null)[] = [null, null, null, null, "red", "yellow", "green", "blue"];

/** mulberry32: küçük, hızlı, tohumlanabilir. */
function rng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function pick<T>(r: () => number, xs: readonly T[]): T {
  return xs[Math.floor(r() * xs.length)] as T;
}

function hexHash(r: () => number): string {
  let s = "";
  for (let i = 0; i < 64; i++) s += Math.floor(r() * 16).toString(16);
  return s;
}

const DAY_S = 86_400;
/** 2025-10-24 12:00 UTC */
const NEWEST = 1_761_307_200;

export class MockBackend implements Backend {
  private items: TileItem[] = [];
  private libraries: LibraryDto[];

  constructor(count = MockBackend.countFromUrl(), seed = 7) {
    const r = rng(seed);
    let t = NEWEST;
    for (let i = 0; i < count; i++) {
      // Günde ortalama ~12 fotoğraf, ara sıra boş günler.
      t -= Math.floor(r() * 7200) + (r() < 0.08 ? DAY_S * Math.ceil(r() * 3) : 0);
      this.items.push({
        id: i + 1,
        hash: hexHash(r),
        aspect: pick(r, ASPECTS),
        rating: r() < 0.3 ? Math.ceil(r() * 5) : 0,
        label: pick(r, LABELS),
        favorite: r() < 0.06,
        flag: "none",
        raw: pick(r, RAWS),
        video: r() < 0.02,
        sync: pick(r, SYNCS),
        takenAtUtc: t,
        tzOffsetMin: 180,
      });
    }
    this.libraries = [
      {
        id: 1,
        uuid: "6f1c2a9e-4b7d-4e21-9a55-0c3d8e7f1b42",
        name: "Fotoğraflar",
        kind: "internal",
        state: "online",
        stateReason: null,
        root: "/home/oguz/Resimler/Loupe",
        volumeUuid: "a3e1f0c2-77d1-4c0b-8f5e-2b9d4c6a1e03",
        volumeLabel: null,
        fsType: "ext4",
        managed: true,
        primary: true,
        mediaCount: count,
        totalBytes: 1_000_204_886_016,
        freeBytes: 412_316_860_416,
      },
      {
        id: 2,
        uuid: "d84b7e10-2c3f-4a6e-b1d9-5f0e8a2c7b61",
        name: "Arşiv 2019",
        kind: "external",
        state: "offline",
        stateReason: "disk bağlı değil",
        root: null,
        volumeUuid: "5E2A-91C4",
        volumeLabel: "ARSIV",
        fsType: "exfat",
        managed: false,
        primary: false,
        mediaCount: 0,
        totalBytes: null,
        freeBytes: null,
      },
    ];
  }

  static countFromUrl(): number {
    if (typeof location === "undefined") return 1200;
    const n = Number(new URLSearchParams(location.search).get("mock"));
    return Number.isFinite(n) && n > 0 ? Math.min(n, 500_000) : 1200;
  }

  private viewItems(req: TimelineRequest): TileItem[] {
    switch (req.view.kind) {
      case "favorites":
        return this.items.filter((i) => i.favorite);
      case "library":
        return req.view.id === 1 ? this.items : [];
      default:
        return this.items;
    }
  }

  async timelineSlice(req: TimelineRequest): Promise<TimelineSlice> {
    const all = this.viewItems(req);
    const sections: Section[] = [];
    for (let i = 0; i < all.length; i++) {
      const it = all[i] as TileItem;
      const d = new Date((it.takenAtUtc + (it.tzOffsetMin ?? 0) * 60) * 1000);
      const key =
        req.grouping === "month"
          ? `${d.getUTCFullYear()}-${d.getUTCMonth() + 1}`
          : `${d.getUTCFullYear()}-${d.getUTCMonth() + 1}-${d.getUTCDate()}`;
      const last = sections.at(-1);
      if (last && last.key === key) {
        last.count++;
        continue;
      }
      sections.push({
        key,
        title:
          req.grouping === "month"
            ? `${MONTHS[d.getUTCMonth()]} ${d.getUTCFullYear()}`
            : `${d.getUTCDate()} ${MONTHS[d.getUTCMonth()]} ${d.getUTCFullYear()}`,
        subtitle: req.grouping === "month" ? null : (DAYS[d.getUTCDay()] ?? null),
        start: i,
        count: 1,
      });
    }
    return {
      total: all.length,
      offset: req.offset,
      sections,
      items: all.slice(req.offset, req.offset + req.limit).map((i) => ({ ...i })),
    };
  }

  async prioritizeViewport(ids: number[]): Promise<number> {
    return ids.length;
  }

  async inspectorDetail(id: number): Promise<InspectorDetail> {
    const it = this.items[id - 1];
    if (!it) throw Object.assign(new Error("medya bulunamadı"), { code: "NOT_FOUND" });
    const r = rng(id);
    const w = 6000;
    const h = Math.round(w / it.aspect);
    const bins = 64;
    const curve = (shift: number) =>
      Array.from({ length: bins }, (_, i) => Math.round(1000 * Math.exp(-(((i - 30 - shift) / 12) ** 2)) + r() * 60));
    const d = new Date((it.takenAtUtc + 180 * 60) * 1000);
    const pad = (n: number) => String(n).padStart(2, "0");
    const ext = it.raw ?? "JPG";
    return {
      id,
      hash: it.hash,
      fileName: `DSCF${String(id).padStart(4, "0")}.${ext}`,
      originalFilename: `DSCF${String(id).padStart(4, "0")}.${ext}`,
      relPath: `${d.getUTCFullYear()}/${pad(d.getUTCMonth() + 1)}/${pad(d.getUTCDate())}/DSCF${String(id).padStart(4, "0")}.${ext}`,
      path: `/home/oguz/Resimler/Loupe/${d.getUTCFullYear()}/${pad(d.getUTCMonth() + 1)}/${pad(d.getUTCDate())}/DSCF${String(id).padStart(4, "0")}.${ext}`,
      libraryName: "Fotoğraflar",
      libraryOnline: true,
      fileType: it.raw ? "raw" : "image",
      rawFormat: it.raw,
      mime: it.raw ? "image/x-raw" : "image/jpeg",
      sizeBytes: it.raw ? 26_418_112 : 8_204_331,
      width: w,
      height: h,
      megapixels: Math.round((w * h) / 100_000) / 10,
      takenAtUtc: it.takenAtUtc,
      tzOffsetMin: 180,
      takenLocal: `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())} ${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}:${pad(d.getUTCSeconds())} +03:00`,
      takenSource: "exif",
      cameraMake: "FUJIFILM",
      cameraModel: "X-T5",
      lensModel: "XF23mmF1.4 R LM WR",
      focalLength: "23 mm",
      aperture: "f/2.8",
      exposure: "1/500 s",
      iso: 160,
      gps: { lat: 38.642833, lon: 34.828861, altM: 1063, dms: `38° 38' 34.2" N, 034° 49' 43.9" E` },
      place: {
        display: "Göreme, Nevşehir, Türkiye",
        locality: "Göreme",
        admin2: null,
        admin1: "Nevşehir",
        country: "Türkiye",
        countryCode: "TR",
      },
      rating: it.rating,
      label: it.label,
      favorite: it.favorite,
      flag: it.flag,
      sidecarDirty: false,
      verifyState: "ok",
      lastVerifiedAt: NEWEST,
      importedAt: NEWEST,
      placement: "managed",
      sync: [
        {
          target: "B2 Yedek",
          provider: "s3",
          state: it.sync === "synced" ? "synced" : it.sync === "error" ? "error" : "queued",
          encryption: "age",
          keyFingerprint: "age1qyqszqgpqyqszqgpqyqszqgpqyqszqgp",
          remoteChecksumAlgo: "sha256",
          uploadedAt: it.sync === "synced" ? NEWEST : null,
          lastVerifiedAt: null,
          lastErrorCode: it.sync === "error" ? "NETWORK" : null,
        },
      ],
      histogram: {
        bins,
        luma: curve(0),
        red: curve(4),
        green: curve(0),
        blue: curve(-6),
        shadowsClippedPermille: 0,
        highlightsClippedPermille: Math.floor(r() * 4),
        source: "md",
      },
    };
  }

  async setRatingLabelFavorite(req: CullRequest): Promise<CullResult[]> {
    return req.ids.flatMap((id) => {
      const it = this.items[id - 1];
      if (!it) return [];
      if (req.rating !== undefined) it.rating = req.rating;
      if (req.label !== undefined) it.label = req.label === "none" ? null : req.label;
      if (req.favorite !== undefined) it.favorite = req.favorite;
      return [{ id, rating: it.rating, label: it.label, favorite: it.favorite, sidecar: "written" as const }];
    });
  }

  async sidebar(): Promise<SidebarDto> {
    return {
      libraries: this.libraries,
      views: [
        { key: "all", count: this.items.length },
        { key: "favorites", count: this.items.filter((i) => i.favorite).length },
        { key: "events", count: Math.ceil(this.items.length / 40) },
        { key: "people", count: 12 },
        { key: "places", count: 9 },
      ],
      smartAlbums: [
        { id: 1, name: "5 yıldız RAW", pinned: true },
        { id: 2, name: "Yedeklenmemiş", pinned: false },
      ],
    };
  }

  async status(): Promise<StatusDto> {
    return {
      totalMedia: this.items.length,
      queue: { pending: 0, running: 0, failed: 0, dead: 0 },
      workers: "running",
      importsActive: 0,
      librariesOnline: 1,
      librariesOffline: 1,
      freeBytes: this.libraries[0]?.freeBytes ?? null,
      lan: { state: "listening", port: 48231, pairedDevices: 1, activeUploads: 0, bytesPerSec: 0, verifiedFiles: 0, error: null },
      cloud: [],
    };
  }

  async listLibraries(): Promise<LibraryDto[]> {
    return this.libraries;
  }
  async refreshLibraries(): Promise<LibraryDto[]> {
    return this.libraries;
  }
  async pickOrAddLibraryRoot(): Promise<AddedLibrary | null> {
    return null;
  }
  async pickImportSource(): Promise<string | null> {
    return null;
  }
  async startImport(req: ImportRequest): Promise<ImportSessionDto> {
    return {
      id: 1,
      libraryId: req.libraryId,
      mode: req.mode,
      source: req.source,
      state: "done",
      totalFiles: 0,
      totalBytes: 0,
      doneFiles: 0,
      doneBytes: 0,
      skippedDup: 0,
      failedFiles: 0,
      startedAt: NEWEST,
      finishedAt: NEWEST,
      lastError: null,
    };
  }
  async importStatus(id: number): Promise<ImportSessionDto> {
    return { ...(await this.startImport({ libraryId: 1, source: "", mode: "copy" })), id };
  }
  async recentImports(): Promise<ImportSessionDto[]> {
    return [];
  }
  async cancelImport(): Promise<boolean> {
    return false;
  }

  /** Gerçek küçük resim yok: hash'ten türetilmiş düz gri ton (degrade değil). */
  thumbUrl(_size: ThumbSize, hash: string): string {
    const v = 40 + (Number.parseInt(hash.slice(0, 2), 16) % 120);
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="4" height="3"><rect width="4" height="3" fill="rgb(${v},${v},${v})"/></svg>`;
    return `data:image/svg+xml,${encodeURIComponent(svg)}`;
  }
}

/**
 * Tarayıcıda (vite dev) ve testlerde kullanılan bellek içi katalog.
 * Deterministik: aynı tohum aynı kataloğu üretir. `?mock=100000` ile büyük
 * kütüphane davranışı (sanal kaydırma, düzen süresi) Tauri olmadan denenir.
 */
import type { Backend } from "./backend";
import type {
  AddedLibrary,
  AiStatusDto,
  Chip,
  ChipKind,
  CloudSettingsDto,
  CloudTargetDto,
  CloudTargetInput,
  CloudTargetStatusDto,
  ColorLabel,
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
  InspectorDetail,
  LibraryDto,
  PairingQrDto,
  PaletteRequest,
  PaletteResult,
  PeopleDto,
  PersonCard,
  RecoveryAnswer,
  RecoveryDraftDto,
  ResourceMode,
  SearchQuery,
  Section,
  SidebarDto,
  SmartAlbumDto,
  StatusDto,
  Suggestion,
  SyncState,
  TileItem,
  TimelineRequest,
  TimelineSlice,
  VerifyRunDto,
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
const MOCK_NAMES = ["Ayşe Yılmaz", "Mehmet Kaya", "Elif Demir"];
const BASE32 = "QPZRY9X8GF2TVDW0S3JN54KHCE6MUA7L";
/** Paletin tanıdığı belirteç anahtarları (TR/EN). */
const TOKEN_KEYS: Record<string, ChipKind> = {
  person: "person",
  kişi: "person",
  place: "place",
  yer: "place",
  event: "event",
  etkinlik: "event",
  rating: "rating",
  puan: "rating",
  color: "color",
  renk: "color",
  camera: "camera",
  kamera: "camera",
  lens: "lens",
  year: "year",
  yıl: "year",
};

interface MockFace {
  faceId: number;
  mediaId: number;
  cluster: number;
}

interface MockCluster {
  id: number;
  person: number | null;
  ignored: boolean;
}
/** 2025-10-24 12:00 UTC */
const NEWEST = 1_761_307_200;

export class MockBackend implements Backend {
  private items: TileItem[] = [];
  private libraries: LibraryDto[];
  private albums: SmartAlbumDto[] = [
    { id: 1, name: "5 yıldız RAW", pinned: true },
    { id: 2, name: "Yedeklenmemiş", pinned: false },
  ];
  private persons = new Map<number, string>();
  private clusters: MockCluster[] = [];
  private faces: MockFace[] = [];
  private events: EventCard[] = [];
  private eventMedia = new Map<number, number[]>();
  private devices: DeviceDto[] = [];
  private targets: CloudTargetDto[] = [];
  private globalBandwidth: number | null = null;
  private encryption: EncryptionDto = {
    configured: false,
    recipient: null,
    fingerprint: null,
    deviceKey: false,
    createdAt: null,
  };
  private draft: RecoveryDraftDto | null = null;
  private runs: VerifyRunDto[] = [];
  private paused = false;
  private resourceMode: ResourceMode = "balanced";
  private nextId = 100;

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
    this.seedPeople(r);
    this.seedEvents();
    this.devices = [
      {
        id: 1,
        name: "Oğuz'un telefonu",
        platform: "android",
        conflictPolicy: "keep_both",
        pairedAt: (NEWEST - 30 * DAY_S) * 1000,
        lastSeenAt: NEWEST * 1000,
        lastAddress: "192.168.1.42",
        revoked: false,
        received: 1284,
      },
    ];
    this.targets = [
      {
        id: 1,
        name: "B2 Yedek",
        provider: "b2",
        config: { endpoint: "https://s3.eu-central-003.backblazeb2.com", bucket: "loupe-yedek", region: "eu-central-003" },
        keyPrefix: "loupe/",
        encrypted: true,
        bandwidthMbps: 4,
        enabled: true,
        hasCredentials: true,
        user: "003a1b2c3d4e5f60000000001",
      },
    ];
  }

  private seedPeople(r: () => number): void {
    if (this.items.length === 0) return;
    let face = 1;
    for (let c = 1; c <= 6; c++) {
      const named = c <= MOCK_NAMES.length;
      if (named) this.persons.set(c, MOCK_NAMES[c - 1] as string);
      this.clusters.push({ id: c, person: named ? c : null, ignored: false });
      const n = 4 + Math.floor(r() * 12);
      for (let k = 0; k < n; k++) {
        const mediaId = 1 + Math.floor(r() * this.items.length);
        this.faces.push({ faceId: face++, mediaId, cluster: c });
      }
    }
  }

  private seedEvents(): void {
    const per = 40;
    for (let start = 0, id = 1; start + 8 < this.items.length && id <= 60; start += per, id++) {
      const ids = this.items.slice(start, start + 24).map((i) => i.id);
      this.eventMedia.set(id, ids);
      this.events.push(this.eventCard(id, id % 3 === 0 ? `Kapadokya ${id}` : `Gün ${id}`, false, "auto"));
    }
  }

  private eventCard(id: number, title: string, locked: boolean, origin: "auto" | "manual"): EventCard {
    const ids = this.eventMedia.get(id) ?? [];
    const media = ids.map((m) => this.items[m - 1]).filter((x): x is TileItem => x !== undefined);
    const times = media.map((m) => m.takenAtUtc);
    const start = Math.min(...times);
    const end = Math.max(...times);
    const d0 = new Date(start * 1000);
    const d1 = new Date(end * 1000);
    const range =
      d0.getUTCDate() === d1.getUTCDate() && d0.getUTCMonth() === d1.getUTCMonth()
        ? `${d0.getUTCDate()} ${MONTHS[d0.getUTCMonth()]} ${d0.getUTCFullYear()}`
        : d0.getUTCMonth() === d1.getUTCMonth()
          ? `${d0.getUTCDate()}–${d1.getUTCDate()} ${MONTHS[d1.getUTCMonth()]} ${d1.getUTCFullYear()}`
          : `${d0.getUTCDate()} ${MONTHS[d0.getUTCMonth()]} – ${d1.getUTCDate()} ${MONTHS[d1.getUTCMonth()]} ${d1.getUTCFullYear()}`;
    const cover = media[0];
    return {
      id,
      title,
      range,
      startUtc: start * 1000,
      endUtc: end * 1000,
      count: media.length,
      coverHash: cover?.hash ?? null,
      coverAspect: cover?.aspect ?? 1.5,
      place: id % 3 === 0 ? "Göreme, Nevşehir" : null,
      locked,
      origin,
    };
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
      case "search":
        return this.searchItems(req.view.query);
      case "person":
      case "cluster": {
        const v = req.view;
        const clusters = new Set(
          this.clusters.filter((c) => (v.kind === "person" ? c.person === v.id : c.id === v.id)).map((c) => c.id),
        );
        const ids = new Set(this.faces.filter((f) => clusters.has(f.cluster)).map((f) => f.mediaId));
        return this.items.filter((i) => ids.has(i.id));
      }
      case "event": {
        const ids = new Set(this.eventMedia.get(req.view.id) ?? []);
        return this.items.filter((i) => ids.has(i.id));
      }
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
          uploadedAt: it.sync === "synced" ? NEWEST * 1000 : null,
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
        { key: "events", count: this.events.length },
        { key: "people", count: this.persons.size },
        { key: "places", count: 9 },
      ],
      smartAlbums: [...this.albums],
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
      lan: {
        state: "listening",
        port: 48231,
        pairedDevices: this.devices.filter((d) => !d.revoked).length,
        activeUploads: 0,
        bytesPerSec: 0,
        bytesExpected: 0,
        bytesReceived: 0,
        etaSecs: null,
        verifiedFiles: 0,
        skipped: 0,
        conflicts: 0,
        failed: 0,
        error: null,
      },
      cloud: this.cloudStatus(),
      ai: this.aiDto(),
      verifying: this.runs.filter((r) => r.state === "running").length,
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

  // ───────────────────────────── arama paleti ─────────────────────────────

  private parseInput(input: string): { query: SearchQuery; chips: Chip[]; text: string; last: string } {
    const query: SearchQuery = {};
    const chips: Chip[] = [];
    const words: string[] = [];
    const tokens = input.match(/(?:[^\s"]+:"[^"]*"|\S+)/g) ?? [];
    for (const raw of tokens) {
      const colon = raw.indexOf(":");
      const kind = colon > 0 ? TOKEN_KEYS[raw.slice(0, colon).toLocaleLowerCase("tr")] : undefined;
      if (!kind) {
        if (raw === "★" || raw === "fav" || raw === "favori") {
          query.favorite = true;
          chips.push({ kind: "favorite", raw, label: "Favori", ok: true, error: null });
        } else words.push(raw);
        continue;
      }
      const value = raw.slice(colon + 1).replace(/^"|"$/g, "");
      const chip: Chip = { kind, raw, label: value, ok: value.length > 0, error: value ? null : "değer yok" };
      switch (kind) {
        case "person": {
          const hit = [...this.persons].find(([, n]) => n.toLocaleLowerCase("tr").startsWith(value.toLocaleLowerCase("tr")));
          if (hit && value) {
            query.persons = { ids: [...(query.persons?.ids ?? []), hit[0]], mode: "all" };
            chip.label = hit[1];
          } else {
            chip.ok = false;
            chip.error = "kişi bulunamadı";
          }
          break;
        }
        case "rating": {
          const n = Number(value.replace(/\+$/, ""));
          if (Number.isInteger(n) && n >= 1 && n <= 5) {
            query.min_rating = n;
            chip.label = `${"★".repeat(n)} ve üstü`;
          } else {
            chip.ok = false;
            chip.error = "1–5 arası";
          }
          break;
        }
        case "year": {
          const y = Number(value);
          if (Number.isInteger(y) && y > 1900) query.time = { year: y };
          else {
            chip.ok = false;
            chip.error = "yıl YYYY";
          }
          break;
        }
        case "color":
          query.color_labels = [...(query.color_labels ?? []), value];
          break;
        case "camera":
          query.camera_model = value;
          break;
        case "lens":
          query.lens_model = value;
          break;
        case "place":
          query.place = { locality: value };
          break;
        case "event": {
          const ev = this.events.find((e) => e.title.toLocaleLowerCase("tr").includes(value.toLocaleLowerCase("tr")));
          if (ev && value) {
            query.event_id = ev.id;
            chip.label = ev.title;
          } else {
            chip.ok = false;
            chip.error = "etkinlik bulunamadı";
          }
          break;
        }
        default:
          break;
      }
      chips.push(chip);
    }
    const text = words.join(" ");
    if (text) query.text = text;
    return { query, chips, text, last: input.endsWith(" ") ? "" : (tokens.at(-1) ?? "") };
  }

  private suggestionsFor(last: string): Suggestion[] {
    const colon = last.indexOf(":");
    if (colon <= 0) return [];
    const key = last.slice(0, colon);
    const kind = TOKEN_KEYS[key.toLocaleLowerCase("tr")];
    const value = last.slice(colon + 1).replace(/^"/, "").toLocaleLowerCase("tr");
    if (kind === "person") {
      return [...this.persons.values()]
        .filter((n) => n.toLocaleLowerCase("tr").startsWith(value))
        .map((n) => ({ kind, label: n, insert: `${key}:"${n}"`, detail: "kişi" }));
    }
    if (kind === "rating") {
      return [5, 4, 3].map((n) => ({ kind, label: `${"★".repeat(n)} ve üstü`, insert: `${key}:${n}`, detail: null }));
    }
    return [];
  }

  private searchItems(q: SearchQuery): TileItem[] {
    let out = this.items;
    if (q.min_rating) out = out.filter((i) => i.rating >= (q.min_rating ?? 0));
    if (q.favorite) out = out.filter((i) => i.favorite);
    if (q.color_labels?.length) out = out.filter((i) => i.label !== null && q.color_labels?.includes(i.label));
    if (q.persons?.ids.length) {
      const want = new Set(q.persons.ids);
      const clusters = new Set(this.clusters.filter((c) => c.person !== null && want.has(c.person)).map((c) => c.id));
      const ids = new Set(this.faces.filter((f) => clusters.has(f.cluster)).map((f) => f.mediaId));
      out = out.filter((i) => ids.has(i.id));
    }
    if (q.event_id) {
      const ids = new Set(this.eventMedia.get(q.event_id) ?? []);
      out = out.filter((i) => ids.has(i.id));
    }
    if (q.time?.year) out = out.filter((i) => new Date(i.takenAtUtc * 1000).getUTCFullYear() === q.time?.year);
    if (q.text) {
      // Sahte "anlamsal" sıralama: metinden türetilen sabit bir alt küme.
      const seed = [...q.text].reduce((a, c) => a + c.charCodeAt(0), 0);
      out = out.filter((i) => (i.id * 31 + seed) % 7 === 0);
    }
    return out;
  }

  async searchPalette(req: PaletteRequest): Promise<PaletteResult> {
    const t0 = performance.now();
    const { query, chips, text, last } = this.parseInput(req.input);
    const searched = req.input.trim().length > 0 && chips.every((c) => c.ok);
    const all = searched ? this.searchItems(query) : [];
    return {
      query,
      chips,
      suggestions: this.suggestionsFor(last),
      text: text || null,
      searched,
      total: all.length,
      items: all.slice(0, req.preview ?? 12).map((i) => ({ ...i })),
      semantic: Boolean(text),
      elapsedMs: Math.round(performance.now() - t0),
    };
  }

  async saveSmartAlbum(name: string, _query: SearchQuery, pinned = false): Promise<SmartAlbumDto> {
    if (!name.trim()) throw Object.assign(new Error("ad gerekli"), { code: "INVALID" });
    const a = { id: this.nextId++, name: name.trim(), pinned };
    this.albums.push(a);
    return a;
  }

  async deleteSmartAlbum(id: number): Promise<void> {
    this.albums = this.albums.filter((a) => a.id !== id);
  }

  // ───────────────────────────── kişiler ─────────────────────────────

  private faceRef(f: MockFace): FaceRef {
    const it = this.items[f.mediaId - 1];
    return {
      faceId: f.faceId,
      mediaId: f.mediaId,
      hash: it?.hash ?? "",
      aspect: it?.aspect ?? 1.5,
      bbox: { x: 0.4, y: 0.25, w: 0.18, h: 0.27 },
    };
  }

  private card(clusters: MockCluster[], person: number | null): PersonCard {
    const ids = new Set(clusters.map((c) => c.id));
    const faces = this.faces.filter((f) => ids.has(f.cluster));
    const biggest = [...clusters].sort(
      (a, b) => this.faces.filter((f) => f.cluster === b.id).length - this.faces.filter((f) => f.cluster === a.id).length,
    )[0];
    return {
      personId: person,
      clusterId: biggest?.id ?? null,
      name: person === null ? null : (this.persons.get(person) ?? null),
      faceCount: faces.length,
      photoCount: new Set(faces.map((f) => f.mediaId)).size,
      cover: faces[0] ? this.faceRef(faces[0]) : null,
    };
  }

  async listPeople(): Promise<PeopleDto> {
    const named = [...this.persons.keys()].map((p) => this.card(this.clusters.filter((c) => c.person === p && !c.ignored), p));
    const unnamed = this.clusters
      .filter((c) => c.person === null && !c.ignored && this.faces.some((f) => f.cluster === c.id))
      .map((c) => this.card([c], null));
    return {
      named: named.sort((a, b) => b.photoCount - a.photoCount),
      unnamed: unnamed.sort((a, b) => b.faceCount - a.faceCount),
      ignored: this.clusters.filter((c) => c.ignored).length,
    };
  }

  async personFaces(target: { person?: number; cluster?: number }, limit = 500): Promise<FaceRef[]> {
    const ids = new Set(
      this.clusters
        .filter((c) => (target.person !== undefined ? c.person === target.person : c.id === target.cluster))
        .map((c) => c.id),
    );
    return this.faces.filter((f) => ids.has(f.cluster)).slice(0, limit).map((f) => this.faceRef(f));
  }

  private personByName(name: string): number {
    const trimmed = name.trim();
    if (!trimmed) throw Object.assign(new Error("ad gerekli"), { code: "INVALID" });
    const hit = [...this.persons].find(([, n]) => n.toLocaleLowerCase("tr") === trimmed.toLocaleLowerCase("tr"));
    if (hit) return hit[0];
    const id = this.nextId++;
    this.persons.set(id, trimmed);
    return id;
  }

  async nameCluster(cluster: number, name: string): Promise<number> {
    const p = this.personByName(name);
    for (const c of this.clusters) if (c.id === cluster) c.person = p;
    return p;
  }

  async renamePerson(person: number, name: string): Promise<number> {
    const trimmed = name.trim();
    const other = [...this.persons].find(([id, n]) => id !== person && n.toLocaleLowerCase("tr") === trimmed.toLocaleLowerCase("tr"));
    if (other) {
      await this.mergePersons(other[0], [person]);
      return other[0];
    }
    this.persons.set(person, trimmed);
    return person;
  }

  async mergeClusters(into: number, from: number[]): Promise<void> {
    for (const f of this.faces) if (from.includes(f.cluster)) f.cluster = into;
    this.clusters = this.clusters.filter((c) => !from.includes(c.id) || c.id === into);
  }

  async mergePersons(into: number, from: number[]): Promise<void> {
    for (const c of this.clusters) if (c.person !== null && from.includes(c.person)) c.person = into;
    for (const p of from) if (p !== into) this.persons.delete(p);
  }

  async moveFace(face: number, cluster: number): Promise<void> {
    const f = this.faces.find((x) => x.faceId === face);
    if (f) f.cluster = cluster;
  }

  async assignFace(face: number, person: number): Promise<void> {
    const c = this.clusters.find((x) => x.person === person);
    if (c) await this.moveFace(face, c.id);
  }

  async detachFace(face: number): Promise<number> {
    const id = this.nextId++;
    this.clusters.push({ id, person: null, ignored: false });
    await this.moveFace(face, id);
    return id;
  }

  async rejectFace(face: number): Promise<void> {
    this.faces = this.faces.filter((f) => f.faceId !== face);
  }

  async ignoreCluster(cluster: number, ignored: boolean): Promise<void> {
    for (const c of this.clusters) if (c.id === cluster) c.ignored = ignored;
  }

  // ───────────────────────────── etkinlikler ─────────────────────────────

  async listEvents(): Promise<EventCard[]> {
    return this.events.map((e) => ({ ...e }));
  }

  private event(id: number): EventCard {
    const e = this.events.find((x) => x.id === id);
    if (!e) throw Object.assign(new Error("etkinlik bulunamadı"), { code: "NOT_FOUND" });
    return e;
  }

  async renameEvent(id: number, title: string): Promise<EventCard> {
    const e = this.event(id);
    if (!title.trim()) throw Object.assign(new Error("başlık gerekli"), { code: "INVALID" });
    e.title = title.trim();
    e.locked = true;
    return { ...e };
  }

  async excludeFromEvent(id: number, media: number[]): Promise<EventCard> {
    const e = this.event(id);
    this.eventMedia.set(id, (this.eventMedia.get(id) ?? []).filter((m) => !media.includes(m)));
    const fresh = this.eventCard(id, e.title, true, e.origin);
    Object.assign(e, fresh);
    return { ...e };
  }

  async setEventLocked(id: number, locked: boolean): Promise<EventCard> {
    const e = this.event(id);
    e.locked = locked;
    return { ...e };
  }

  // ───────────────────────────── yapay zekâ ─────────────────────────────

  private aiDto(): AiStatusDto {
    return {
      mode: "mock",
      reason: null,
      semantic: true,
      paused: this.paused,
      resourceMode: this.resourceMode,
      pending: { faces: 0, clip: 0, clustering: 0 },
      facesDone: this.items.length,
      clipDone: this.items.length,
      images: this.items.length,
    };
  }

  async aiStatus(): Promise<AiStatusDto> {
    return this.aiDto();
  }

  async setPaused(paused: boolean): Promise<AiStatusDto> {
    this.paused = paused;
    return this.aiDto();
  }

  async setResourceMode(mode: ResourceMode): Promise<AiStatusDto> {
    this.resourceMode = mode;
    return this.aiDto();
  }

  // ───────────────────────────── Wi-Fi ─────────────────────────────

  async pairingQr(): Promise<PairingQrDto> {
    const token = hexHash(rng(this.nextId++)).slice(0, 32);
    // Gerçek QR değil: yalnızca biçim (tek rect + tek path) taklit edilir.
    let d = "";
    const r = rng(token.length + this.nextId);
    for (let y = 0; y < 25; y++) for (let x = 0; x < 25; x++) if (r() < 0.5) d += `M${x + 4},${y + 4}h1v1h-1z`;
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 33 33" shape-rendering="crispEdges" role="img" aria-label="Eşleşme QR kodu"><rect width="33" height="33" fill="white"/><path d="${d}" fill="black"/></svg>`;
    return {
      uri: `loupe://pair?t=${token}`,
      svg,
      expiresAtMs: Date.now() + 60_000,
      ttlSecs: 60,
      addrs: ["192.168.1.20"],
      port: 48231,
      name: "Loupe masaüstü",
    };
  }

  async cancelPairing(): Promise<void> {}

  async listDevices(): Promise<DeviceDto[]> {
    return this.devices.map((d) => ({ ...d }));
  }

  async revokeDevice(id: number): Promise<DeviceDto[]> {
    for (const d of this.devices) if (d.id === id) d.revoked = true;
    return this.listDevices();
  }

  async setConflictPolicy(id: number, policy: ConflictPolicy): Promise<DeviceDto[]> {
    for (const d of this.devices) if (d.id === id) d.conflictPolicy = policy;
    return this.listDevices();
  }

  // ───────────────────────────── bulut ─────────────────────────────

  private cloudStatus(): CloudTargetStatusDto[] {
    return this.targets.map((t) => ({
      id: t.id,
      name: t.name,
      provider: t.provider,
      encrypted: t.encrypted,
      state: !t.enabled ? "disabled" : t.encrypted && !this.encryption.configured ? "needs_setup" : "idle",
      pending: 0,
      synced: 0,
      lastError: null,
    }));
  }

  async cloudSettings(): Promise<CloudSettingsDto> {
    return {
      targets: this.targets.map((t) => ({ ...t })),
      globalBandwidthMbps: this.globalBandwidth,
      encryption: { ...this.encryption },
      secretBackend: "file",
    };
  }

  async saveCloudTarget(id: number | null, input: CloudTargetInput): Promise<CloudSettingsDto> {
    if (!input.name.trim()) throw Object.assign(new Error("ad gerekli"), { code: "INVALID" });
    const existing = id === null ? undefined : this.targets.find((t) => t.id === id);
    if (existing && existing.provider !== input.provider) {
      throw Object.assign(new Error("sağlayıcı değiştirilemez"), { code: "INVALID" });
    }
    const t: CloudTargetDto = {
      id: existing?.id ?? this.nextId++,
      name: input.name.trim(),
      provider: input.provider,
      config: { ...input.config },
      keyPrefix: input.keyPrefix ?? "loupe/",
      encrypted: input.encrypted ?? true,
      bandwidthMbps: input.bandwidthMbps ?? null,
      enabled: input.enabled ?? true,
      hasCredentials: input.provider === "local" || Boolean(input.secret) || (existing?.hasCredentials ?? false),
      user: input.user ?? existing?.user ?? null,
    };
    this.targets = existing ? this.targets.map((x) => (x.id === t.id ? t : x)) : [...this.targets, t];
    return this.cloudSettings();
  }

  async setCloudTargetEnabled(id: number, enabled: boolean): Promise<CloudSettingsDto> {
    for (const t of this.targets) if (t.id === id) t.enabled = enabled;
    return this.cloudSettings();
  }

  async deleteCloudTarget(id: number): Promise<CloudSettingsDto> {
    this.targets = this.targets.filter((t) => t.id !== id);
    return this.cloudSettings();
  }

  async setGlobalBandwidth(mbps: number | null): Promise<CloudSettingsDto> {
    this.globalBandwidth = mbps;
    return this.cloudSettings();
  }

  async startCloudSync(): Promise<CloudTargetStatusDto[]> {
    return this.cloudStatus();
  }

  async generateRecoveryKey(): Promise<RecoveryDraftDto> {
    const r = rng(this.nextId++);
    const body = Array.from({ length: 59 }, () => BASE32[Math.floor(r() * 32)]).join("");
    const groups = body.match(/.{1,4}/g) ?? [];
    this.draft = {
      prefix: "AGE-SECRET-KEY-1",
      groups,
      secret: `AGE-SECRET-KEY-1${body}`,
      recipient: `age1${body.slice(0, 58).toLowerCase()}`,
      challenge: [2, 7, 11],
      replaces: this.encryption.configured,
    };
    return { ...this.draft };
  }

  async exportRecoveryKey(): Promise<string | null> {
    return this.draft ? "/home/oguz/loupe-kurtarma-anahtari.txt" : null;
  }

  async confirmRecoveryKey(answers: RecoveryAnswer[]): Promise<EncryptionDto> {
    const d = this.draft;
    if (!d) throw Object.assign(new Error("bekleyen kurtarma anahtarı yok"), { code: "INVALID" });
    const ok = d.challenge.every((g) => answers.find((a) => a.group === g)?.value.trim().toUpperCase() === d.groups[g]);
    if (!ok) throw Object.assign(new Error("gruplar eşleşmiyor"), { code: "INVALID" });
    this.encryption = {
      configured: true,
      recipient: d.recipient,
      fingerprint: d.recipient.slice(0, 16),
      deviceKey: true,
      createdAt: Date.now(),
    };
    this.draft = null;
    return { ...this.encryption };
  }

  async cancelRecoveryKey(): Promise<void> {
    this.draft = null;
  }

  async checkRecoveryKey(key: string): Promise<boolean> {
    if (!this.encryption.configured) throw Object.assign(new Error("kurtarma anahtarı kurulmamış"), { code: "INVALID" });
    const body = key.replace(/\s+/g, "").toUpperCase().replace(/^AGE-SECRET-KEY-1/, "");
    return `age1${body.slice(0, 58).toLowerCase()}` === this.encryption.recipient;
  }

  // ───────────────────────────── doğrulama ─────────────────────────────

  async verifyLibrary(library?: number): Promise<number[]> {
    const libs = this.libraries.filter((l) => l.state === "online" && (library === undefined || l.id === library));
    const now = Date.now();
    for (const l of libs) {
      this.runs.unshift({
        id: this.nextId++,
        libraryId: l.id,
        library: l.name,
        trigger: "manual",
        state: "done",
        total: this.items.length,
        checked: this.items.length,
        ok: this.items.length - 1,
        missing: 0,
        mismatched: 1,
        unreadable: 0,
        bytesRead: this.items.length * 8_204_331,
        startedAt: now,
        finishedAt: now + 1000,
      });
    }
    return libs.map((l) => l.id);
  }

  async cancelVerify(): Promise<number> {
    return 0;
  }

  async verifyRuns(limit = 10): Promise<VerifyRunDto[]> {
    return this.runs.slice(0, limit).map((r) => ({ ...r }));
  }

  /** Gerçek küçük resim yok: hash'ten türetilmiş düz gri ton (degrade değil). */
  thumbUrl(_size: ImageSize, hash: string): string {
    const v = 40 + (Number.parseInt(hash.slice(0, 2), 16) % 120);
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="4" height="3"><rect width="4" height="3" fill="rgb(${v},${v},${v})"/></svg>`;
    return `data:image/svg+xml,${encodeURIComponent(svg)}`;
  }
}

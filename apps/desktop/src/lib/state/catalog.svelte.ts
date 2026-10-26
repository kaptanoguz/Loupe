/**
 * Katalog durumu (Svelte 5 runes). Tek kaynak: görünüm, zaman çizelgesi
 * sayfaları, seçim, panel durumları ve kürasyon. Bileşenler yalnızca okur ve
 * `dispatch` / yöntemleri çağırır.
 */
import { DEFAULT_ZOOM, ROW_HEIGHTS, type ZoomLevel } from "../design/layout";
import type { Backend } from "../api/backend";
import type {
  Arrange,
  CullRequest,
  EditStateDto,
  ExternalEditorDto,
  Grouping,
  InspectorDetail,
  Section,
  SidebarDto,
  StatusDto,
  TileItem,
  View,
} from "../api/types";
import type { JustifiedLayout } from "../layout/justified";
import { neighbor } from "../layout/virtual";
import { type Action, cullRequestFor } from "./keymap";
import { t } from "../i18n";

/** İlk sayfa küçük (ilk ekran hızlı), sonrakiler çekirdeğin üst sınırında. */
export const FIRST_PAGE = 600;
export const PAGE = 5000;

export function sameView(a: View, b: View): boolean {
  if (a.kind === "search" || b.kind === "search") {
    return a.kind === b.kind && JSON.stringify("query" in a ? a.query : null) === JSON.stringify("query" in b ? b.query : null);
  }
  return a.kind === b.kind && ("id" in a ? a.id : null) === ("id" in b ? b.id : null);
}

/** Izgara yerine kendi sayfası olan görünümler. */
export function isPageView(v: View): boolean {
  return v.kind === "people" || v.kind === "events";
}

/** Kenar çubuğu dışından açılan görünümün başlığı (kişi adı, etkinlik, arama). */
export interface Heading {
  title: string;
  subtitle?: string | null;
  /** Arama görünümünde paletin girdisi (akıllı albüm adı önerisi). */
  input?: string;
}

export type Overlay = "palette" | "wifi" | "settings" | "trash" | "import" | "album" | null;
export type SettingsTab = "libraries" | "phone" | "cloud" | "encryption" | "verify" | "ai";

export const DEFAULT_ARRANGE: Arrange = { sort: "taken", dir: "desc", grouping: "day" };
const ARRANGE_KEY = "loupe.arrange";

/** Tarihe göre olmayan sıralamalar kendi bölümlerini kurar; gruplama uygulanmaz. */
export function groupable(a: Arrange): boolean {
  return a.sort === "taken" || a.sort === "added";
}

function loadArrange(): Arrange {
  try {
    const raw = localStorage.getItem(ARRANGE_KEY);
    if (!raw) return DEFAULT_ARRANGE;
    const a = JSON.parse(raw) as Partial<Arrange>;
    return {
      sort: a.sort ?? DEFAULT_ARRANGE.sort,
      dir: a.dir ?? DEFAULT_ARRANGE.dir,
      grouping: a.grouping ?? DEFAULT_ARRANGE.grouping,
    };
  } catch {
    return DEFAULT_ARRANGE;
  }
}

/** Slayt gösterisi: gösterilecek öğe sırası (dizin) ve konum. */
export interface Slideshow {
  order: number[];
  pos: number;
  paused: boolean;
}

export interface SlideOptions {
  seconds: number;
  shuffle: boolean;
}

export const SLIDE_SECONDS = [3, 5, 10] as const;
export const DEFAULT_SLIDE: SlideOptions = { seconds: 5, shuffle: false };
const SLIDE_KEY = "loupe.slideshow";

function loadSlide(): SlideOptions {
  try {
    const s = JSON.parse(localStorage.getItem(SLIDE_KEY) ?? "{}") as Partial<SlideOptions>;
    return {
      seconds: SLIDE_SECONDS.includes(s.seconds as 3) ? (s.seconds as number) : DEFAULT_SLIDE.seconds,
      shuffle: s.shuffle === true,
    };
  } catch {
    return DEFAULT_SLIDE;
  }
}

/** Bilgi notu durum çubuğunda bu kadar kalır (ms). */
export const NOTICE_MS = 4000;

export class Catalog {
  view = $state<View>({ kind: "all" });
  /** Sıralama ve yön; bölümleme `grouping`. */
  sort = $state<Arrange["sort"]>(DEFAULT_ARRANGE.sort);
  dir = $state<Arrange["dir"]>(DEFAULT_ARRANGE.dir);
  grouping = $state<Grouping>(DEFAULT_ARRANGE.grouping);
  total = $state(0);
  loaded = $state(0);
  /** İlk dilim geldi mi (boş görünüm ile yükleniyor ayrımı). */
  opened = $state(false);
  sections = $state.raw<Section[]>([]);
  /** Öğeler değiştiğinde artar (dizinin kendisi reaktif değil: 100k öğe). */
  rev = $state(0);
  /** En/boy oranları değiştiğinde artar → düzen yeniden hesaplanır. */
  aspectsRev = $state(0);
  selected = $state(-1);
  /** Ctrl/Shift ile işaretlenen medya kimlikleri. Boşsa toplu işlemler seçili öğeye uygulanır. */
  marked = $state.raw<ReadonlySet<number>>(new Set());
  /** Shift ile aralık seçiminin başlangıcı. */
  private anchor = -1;
  inspectorOpen = $state(true);
  loupeOpen = $state(false);
  loupeActual = $state(false);
  /** Düzenleme ekranı seçili fotoğraf için açık. */
  editorOpen = $state(false);
  /** Kurulu dış düzenleyiciler (Darktable, GIMP); ilk istekte yüklenir. */
  editors = $state.raw<ExternalEditorDto[]>([]);
  zoom = $state<ZoomLevel>(DEFAULT_ZOOM);
  detail = $state.raw<InspectorDetail | null>(null);
  sidebar = $state.raw<SidebarDto | null>(null);
  status = $state.raw<StatusDto | null>(null);
  error = $state<string | null>(null);
  /** Başarılı işlem notu (hata değil); kısa süre sonra kaybolur. */
  notice = $state<string | null>(null);
  overlay = $state<Overlay>(null);
  settingsTab = $state<SettingsTab>("libraries");
  heading = $state.raw<Heading | null>(null);
  /** Sağ tık menüsünün konumu (ekran koordinatı). */
  menu = $state.raw<{ x: number; y: number } | null>(null);
  slideshow = $state.raw<Slideshow | null>(null);
  slide = $state.raw<SlideOptions>(DEFAULT_SLIDE);
  /** Son düzen hesabının süresi (ms), durum çubuğunda. */
  layoutMs = $state(0);
  /** Grid'in son hesapladığı düzen (↑/↓ komşu araması için). */
  layout: JustifiedLayout | null = null;

  items: (TileItem | undefined)[] = [];
  aspects = new Float32Array(0);
  private loadToken = 0;
  private detailTimer: ReturnType<typeof setTimeout> | undefined;
  private noticeTimer: ReturnType<typeof setTimeout> | undefined;

  readonly backend: Backend;

  constructor(backend: Backend) {
    this.backend = backend;
    const a = loadArrange();
    this.sort = a.sort;
    this.dir = a.dir;
    this.grouping = a.grouping;
    this.slide = loadSlide();
  }

  get arrange(): Arrange {
    return { sort: this.sort, dir: this.dir, grouping: this.grouping };
  }

  /** Sıralama / yön / bölümlemeyi değiştirir, saklar ve görünümü yeniden açar. */
  async setArrange(patch: Partial<Arrange>): Promise<void> {
    const next = { ...this.arrange, ...patch };
    this.sort = next.sort;
    this.dir = next.dir;
    try {
      localStorage.setItem(ARRANGE_KEY, JSON.stringify(next));
    } catch {
      // Saklanamazsa bu oturumda geçerli kalır.
    }
    await this.open(this.view, next.grouping);
  }

  get targetRowHeight(): number {
    return ROW_HEIGHTS[this.zoom];
  }

  get selectedItem(): TileItem | undefined {
    void this.rev;
    return this.selected >= 0 ? this.items[this.selected] : undefined;
  }

  /** Görünümü açar: sayfaları sırayla çeker, her sayfada düzen güncellenir. */
  async open(view: View, grouping: Grouping = this.grouping, heading: Heading | null = null): Promise<void> {
    const token = ++this.loadToken;
    const same = sameView(view, this.view);
    const keepId = same ? this.selectedItem?.id : undefined;
    if (!same || heading) this.heading = heading;
    if (!same) {
      this.clearMarks();
      this.stopSlideshow();
    }
    this.menu = null;
    this.view = view;
    this.grouping = grouping;
    this.error = null;
    if (isPageView(view)) {
      // Kişiler ve Etkinlikler kendi sayfalarında; ızgara boşaltılır.
      this.items = [];
      this.aspects = new Float32Array(0);
      this.total = 0;
      this.loaded = 0;
      this.sections = [];
      this.selected = -1;
      this.detail = null;
      this.opened = true;
      this.closeLoupe();
      this.rev++;
      this.aspectsRev++;
      return;
    }
    try {
      const order = { sort: this.sort, dir: this.dir, grouping };
      let first = await this.backend.timelineSlice({ view, ...order, offset: 0, limit: FIRST_PAGE });
      if (token !== this.loadToken) return;
      this.items = new Array(first.total);
      this.aspects = new Float32Array(first.total).fill(1.5);
      this.total = first.total;
      this.sections = first.sections;
      this.loaded = 0;
      this.opened = true;
      this.selected = -1;
      this.detail = null;
      for (;;) {
        first.items.forEach((it, k) => {
          this.items[first.offset + k] = it;
          this.aspects[first.offset + k] = it.aspect;
        });
        this.loaded = Math.min(this.total, first.offset + first.items.length);
        this.rev++;
        this.aspectsRev++;
        if (this.selected < 0 && this.total > 0) {
          const keep = keepId === undefined ? -1 : this.items.findIndex((i) => i?.id === keepId);
          if (keep >= 0 || keepId === undefined) this.select(Math.max(0, keep));
        }
        if (this.loaded >= this.total || first.items.length === 0) break;
        first = await this.backend.timelineSlice({ view, ...order, offset: this.loaded, limit: PAGE });
        if (token !== this.loadToken) return;
      }
      if (this.selected < 0 && this.total > 0) this.select(0);
    } catch (e) {
      if (token === this.loadToken) this.error = e instanceof Error ? e.message : String(e);
    }
  }

  async refreshSidebar(): Promise<void> {
    try {
      this.sidebar = await this.backend.sidebar();
    } catch (e) {
      this.error = e instanceof Error ? e.message : String(e);
    }
  }

  async refreshStatus(): Promise<void> {
    try {
      this.status = await this.backend.status();
    } catch {
      // Durum çubuğu bir sonraki yoklamada yenilenir.
    }
  }

  openSettings(tab: SettingsTab): void {
    this.settingsTab = tab;
    this.overlay = "settings";
  }

  flash(message: string): void {
    this.error = null;
    this.notice = message;
    clearTimeout(this.noticeTimer);
    this.noticeTimer = setTimeout(() => (this.notice = null), NOTICE_MS);
  }

  fail(e: unknown): void {
    this.error = e instanceof Error ? e.message : String(e);
  }

  select(idx: number): void {
    if (idx < 0 || idx >= this.total) return;
    this.selected = idx;
    this.scheduleDetail();
  }

  /** Fare tıklaması: Ctrl/⌘ ekler-çıkarır, Shift aralık seçer, düz tık işaretleri temizler. */
  clickSelect(idx: number, mods: { toggle: boolean; range: boolean }): void {
    const it = this.items[idx];
    if (!it) return;
    if (mods.range && this.anchor >= 0) {
      const next = new Set(this.marked);
      const [a, b] = this.anchor < idx ? [this.anchor, idx] : [idx, this.anchor];
      for (let i = a; i <= b; i++) {
        const x = this.items[i];
        if (x) next.add(x.id);
      }
      this.marked = next;
    } else if (mods.toggle) {
      const next = new Set(this.marked);
      // İlk Ctrl+tık, o an seçili olanı da işarete katar.
      if (next.size === 0 && this.selectedItem && this.selectedItem.id !== it.id) next.add(this.selectedItem.id);
      if (next.has(it.id)) next.delete(it.id);
      else next.add(it.id);
      this.marked = next;
      this.anchor = idx;
    } else {
      this.clearMarks();
      this.anchor = idx;
    }
    this.select(idx);
  }

  clearMarks(): void {
    if (this.marked.size > 0) this.marked = new Set();
    this.anchor = -1;
  }

  /** Toplu işlemlerin hedefi: işaretliler, yoksa seçili öğe. */
  targetIds(): number[] {
    if (this.marked.size > 0) return [...this.marked];
    const it = this.selectedItem;
    return it ? [it.id] : [];
  }

  private scheduleDetail(): void {
    clearTimeout(this.detailTimer);
    if (!this.inspectorOpen) return;
    // Ok tuşu basılı tutulunca her kare için IPC atılmaz.
    this.detailTimer = setTimeout(() => void this.loadDetail(), 60);
  }

  async loadDetail(): Promise<void> {
    const it = this.selectedItem;
    if (!it) {
      this.detail = null;
      return;
    }
    try {
      const d = await this.backend.inspectorDetail(it.id);
      if (this.selectedItem?.id === d.id) this.detail = d;
    } catch (e) {
      this.error = e instanceof Error ? e.message : String(e);
    }
  }

  /** Klavye eylemi. Döner: eylem işlendi mi (preventDefault için). */
  dispatch(action: Action): boolean {
    switch (action.type) {
      case "move": {
        if (this.total === 0) return true;
        if (this.selected < 0) {
          this.select(0);
          return true;
        }
        if (this.layout && this.layout.rects.length / 4 === this.total) {
          this.select(neighbor(this.layout, this.selected, action.dir));
        } else if (action.dir === "left" || action.dir === "right") {
          this.select(Math.min(this.total - 1, Math.max(0, this.selected + (action.dir === "left" ? -1 : 1))));
        }
        return true;
      }
      case "favorite":
        this.toggleFavorite();
        return true;
      case "slideshow":
        this.startSlideshow();
        return true;
      case "edit":
        this.openEditor();
        return true;
      case "rate":
      case "label": {
        const it = this.selectedItem;
        const req = it && cullRequestFor(action, it);
        if (req) void this.cull(req);
        return true;
      }
      case "inspector":
        this.inspectorOpen = !this.inspectorOpen;
        if (this.inspectorOpen) this.scheduleDetail();
        return true;
      case "loupe":
        if (this.loupeOpen) this.closeLoupe();
        else if (this.selectedItem) this.loupeOpen = true;
        return true;
      case "loupeZoom":
        this.loupeActual = !this.loupeActual;
        return true;
      case "close":
        if (!this.loupeOpen) {
          if (this.marked.size === 0) return false;
          this.clearMarks();
          return true;
        }
        this.closeLoupe();
        return true;
      case "selectAll": {
        const next = new Set<number>();
        for (const it of this.items) if (it) next.add(it.id);
        this.marked = next;
        return true;
      }
      case "trash":
        if (this.targetIds().length > 0) this.overlay = "trash";
        return true;
      case "zoom": {
        const z = Math.min(ROW_HEIGHTS.length - 1, Math.max(0, this.zoom + action.delta)) as ZoomLevel;
        this.zoom = z;
        return true;
      }
      case "palette":
        this.overlay = this.overlay === "palette" ? null : "palette";
        return true;
    }
  }

  /** Hedeflerin (işaretliler ya da seçili) favorisini değiştirir: hepsi favoriyse çıkarır, değilse ekler. */
  toggleFavorite(ids: number[] = this.targetIds()): void {
    if (ids.length === 0) return;
    const want = new Set(ids);
    const all = this.items.every((it) => !it || !want.has(it.id) || it.favorite);
    void this.cull({ ids, favorite: !all });
  }

  /** Hedefleri elle oluşturulan albüme ekler. */
  async addToAlbum(id: number, ids: number[] = this.targetIds()): Promise<void> {
    if (ids.length === 0) return;
    try {
      const added = await this.backend.addToAlbum(id, ids);
      await this.refreshSidebar();
      const name = this.sidebar?.albums.find((a) => a.id === id)?.name ?? t("catalog.album");
      const skipped = ids.length - added;
      this.flash(t("catalog.addedToAlbum", { n: added, name }) + (skipped > 0 ? t("catalog.alreadyIn", { n: skipped }) : ""));
    } catch (e) {
      this.fail(e);
    }
  }

  /** Açık albümden hedefleri çıkarır (dosyalar yerinde kalır). */
  async removeFromAlbum(ids: number[] = this.targetIds()): Promise<void> {
    const v = this.view;
    if (v.kind !== "album" || ids.length === 0) return;
    try {
      const n = await this.backend.removeFromAlbum(v.id, ids);
      this.clearMarks();
      await this.open(v);
      await this.refreshSidebar();
      this.flash(t("catalog.removedFromAlbum", { n }));
    } catch (e) {
      this.fail(e);
    }
  }

  /** Dosyanın klasörünü dosya yöneticisinde açar. */
  async reveal(id: number): Promise<void> {
    try {
      const dir = await this.backend.revealMedia(id);
      this.flash(t("catalog.folderOpened", { dir }));
    } catch (e) {
      this.fail(e);
    }
  }

  /** Medyayı sistemin varsayılan uygulamasında açar (video oynatıcı). */
  async openExternal(id: number): Promise<void> {
    try {
      await this.backend.openMedia(id);
    } catch (e) {
      this.fail(e);
    }
  }

  /** Seçili fotoğrafı düzenleme ekranında açar (videolar düzenlenmez). */
  openEditor(): void {
    const it = this.selectedItem;
    if (!it) return;
    this.menu = null;
    if (it.video) {
      this.flash(t("edit.noVideo"));
      return;
    }
    this.editorOpen = true;
  }

  /** Kaydedilen (ya da geri alınan) düzenleme ızgaraya ve Loupe'a yansır. */
  applyEdit(s: EditStateDto): void {
    const k = this.items.findIndex((it) => it?.id === s.id);
    const it = this.items[k];
    if (!it) return;
    it.edit = s.revision;
    if (Math.abs(it.aspect - s.aspect) > 1e-4) {
      it.aspect = s.aspect;
      this.aspects[k] = s.aspect;
      this.aspectsRev++;
    }
    this.rev++;
  }

  async loadEditors(): Promise<void> {
    try {
      this.editors = await this.backend.externalEditors();
    } catch {
      // Bulunamazsa menüde dış düzenleyici gösterilmez.
      this.editors = [];
    }
  }

  /** Orijinal dosyayı Darktable / GIMP'te açar. */
  async openIn(id: number, editor: ExternalEditorDto): Promise<void> {
    try {
      await this.backend.openInEditor(id, editor.id);
      this.flash(t("edit.openingIn", { name: editor.name }));
    } catch (e) {
      this.fail(e);
    }
  }

  /** Hedeflerin hepsi favori mi (menü ve düğme metni). */
  allFavorite(ids: number[] = this.targetIds()): boolean {
    void this.rev;
    if (ids.length === 0) return false;
    const want = new Set(ids);
    return this.items.every((it) => !it || !want.has(it.id) || it.favorite);
  }

  /** Gösterinin sırası: işaretliler varsa yalnızca onlar, yoksa görünümün tamamı; `start` ilk sırada. */
  private slideOrder(start: number): number[] {
    let order: number[] = [];
    if (this.marked.size > 1) {
      this.items.forEach((it, k) => {
        if (it && this.marked.has(it.id)) order.push(k);
      });
    } else {
      order = Array.from({ length: this.total }, (_, k) => k);
    }
    const at = Math.max(0, order.indexOf(start));
    order = [...order.slice(at), ...order.slice(0, at)];
    if (this.slide.shuffle) {
      // İlk öğe yerinde kalır, gerisi karıştırılır (Fisher–Yates).
      for (let i = order.length - 1; i > 1; i--) {
        const j = 1 + Math.floor(Math.random() * i);
        [order[i], order[j]] = [order[j] as number, order[i] as number];
      }
    }
    return order;
  }

  /** Slayt gösterisini seçili öğeden başlatır (tam ekran). */
  startSlideshow(): void {
    if (this.total === 0) return;
    this.menu = null;
    const order = this.slideOrder(Math.max(0, this.selected));
    if (order.length === 0) return;
    this.slideshow = { order, pos: 0, paused: false };
    this.select(order[0] as number);
    void this.backend.setFullscreen(true);
  }

  stopSlideshow(): void {
    if (!this.slideshow) return;
    this.slideshow = null;
    void this.backend.setFullscreen(false);
  }

  /** Sonraki (1) ya da önceki (-1) slayt; sonda başa döner. */
  slideStep(delta: 1 | -1): void {
    const s = this.slideshow;
    if (!s) return;
    const pos = (s.pos + delta + s.order.length) % s.order.length;
    this.slideshow = { ...s, pos };
    this.select(s.order[pos] as number);
  }

  slidePause(paused = !this.slideshow?.paused): void {
    if (this.slideshow) this.slideshow = { ...this.slideshow, paused };
  }

  /** Süre ve karıştırma; saklanır. Gösteri sürerken karıştırma o andan itibaren uygulanır. */
  setSlide(patch: Partial<SlideOptions>): void {
    this.slide = { ...this.slide, ...patch };
    try {
      localStorage.setItem(SLIDE_KEY, JSON.stringify(this.slide));
    } catch {
      // Bu oturumda geçerli kalır.
    }
    const s = this.slideshow;
    if (s && patch.shuffle !== undefined) {
      const current = s.order[s.pos] as number;
      this.slideshow = { ...s, order: this.slideOrder(current), pos: 0 };
    }
  }

  /** Gösteri sırasında tuşlar. Döner: tuş işlendi mi. */
  slideKey(key: string): boolean {
    switch (key) {
      case "Escape":
        this.stopSlideshow();
        return true;
      case " ":
        this.slidePause();
        return true;
      case "ArrowRight":
        this.slideStep(1);
        return true;
      case "ArrowLeft":
        this.slideStep(-1);
        return true;
      case "f":
      case "F": {
        const it = this.selectedItem;
        if (it) this.toggleFavorite([it.id]);
        return true;
      }
      default:
        return false;
    }
  }

  closeLoupe(): void {
    this.loupeOpen = false;
    this.loupeActual = false;
  }

  /** İyimser güncelleme; çekirdeğin döndürdüğü değer esastır, hata olursa geri alınır. */
  async cull(req: CullRequest): Promise<void> {
    const before = new Map<number, TileItem>();
    const index = new Map<number, number>();
    this.items.forEach((it, k) => {
      if (it && req.ids.includes(it.id)) {
        before.set(it.id, { ...it });
        index.set(it.id, k);
        if (req.rating !== undefined) it.rating = req.rating;
        if (req.label !== undefined) it.label = req.label === "none" ? null : req.label;
        if (req.favorite !== undefined) it.favorite = req.favorite;
      }
    });
    this.rev++;
    try {
      const results = await this.backend.setRatingLabelFavorite(req);
      for (const r of results) {
        const it = this.items[index.get(r.id) ?? -1];
        if (!it) continue;
        it.rating = r.rating;
        it.label = r.label;
        it.favorite = r.favorite;
        if (r.sidecar === "failed") this.error = t("catalog.sidecarFailed");
      }
      if (this.detail && index.has(this.detail.id)) {
        const r = results.find((x) => x.id === this.detail?.id);
        if (r) this.detail = { ...this.detail, rating: r.rating, label: r.label, favorite: r.favorite };
      }
    } catch (e) {
      for (const [id, prev] of before) {
        const k = index.get(id);
        if (k !== undefined) this.items[k] = prev;
      }
      this.error = e instanceof Error ? e.message : String(e);
    }
    this.rev++;
  }
}

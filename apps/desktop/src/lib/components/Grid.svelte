<script lang="ts" module>
  import type { ColorLabel, SyncState } from "../api/types";
  import type { ThumbClass } from "../design/layout";

  export interface Slot {
    idx: number;
    /** Medya kimliği; öğe henüz yüklenmediyse `undefined`. */
    id: number | undefined;
    x: number;
    y: number;
    w: number;
    h: number;
    src: string;
    cls: ThumbClass;
    ready: boolean;
    rating: number;
    label: ColorLabel | null;
    favorite: boolean;
    raw: string | null;
    video: boolean;
    sync: SyncState;
  }

  const EMPTY: Slot = {
    idx: -1,
    id: undefined,
    x: 0,
    y: -10_000,
    w: 0,
    h: 0,
    src: "",
    cls: "sm",
    ready: false,
    rating: 0,
    label: null,
    favorite: false,
    raw: null,
    video: false,
    sync: "local",
  };

  /** Karo içeriğinin yatay iç boşluğu (px). */
  export const PAD = 8;
  /** Görünür alan değişince çekirdeğe öncelik bildirimi (ms). */
  export const PRIORITIZE_DEBOUNCE_MS = 50;
</script>

<script lang="ts">
  import { onDestroy, untrack } from "svelte";

  import { count } from "../format";
  import type { JustifiedLayout } from "../layout/justified";
  import type { LayoutEngine } from "../layout/engine";
  import { ScrollSpeed, revealTop, thumbClass, visibleRange } from "../layout/virtual";
  import type { Catalog } from "../state/catalog.svelte";
  import Tile from "./Tile.svelte";

  interface Props {
    catalog: Catalog;
    engine: LayoutEngine;
    onAddFolder?: () => void;
  }

  let { catalog, engine, onAddFolder }: Props = $props();

  let scroller = $state<HTMLDivElement>();
  let width = $state(0);
  let layout = $state.raw<JustifiedLayout | null>(null);
  let slots = $state.raw<Slot[]>([]);
  let headers = $state.raw<number[]>([]);

  const speed = new ScrollSpeed();
  let pool = 0;
  let frame = 0;
  let settleTimer: ReturnType<typeof setTimeout> | undefined;
  let prioTimer: ReturnType<typeof setTimeout> | undefined;
  let lastPrio = "";

  /** Yeniden düzen sırasında ekrandaki öğe aynı yerde kalsın. */
  function anchor(): { idx: number; offset: number } | null {
    if (!layout || !scroller) return null;
    const top = scroller.scrollTop;
    const vh = scroller.clientHeight;
    const sel = catalog.selected;
    const ys = layout.rects;
    if (sel >= 0 && sel * 4 < ys.length) {
      const y = ys[sel * 4 + 1] as number;
      if (y >= top && y < top + vh) return { idx: sel, offset: y - top };
    }
    const r = visibleRange(layout, top, vh, 0);
    if (r.end <= r.start) return null;
    return { idx: r.start, offset: (ys[r.start * 4 + 1] as number) - top };
  }

  $effect(() => {
    void catalog.aspectsRev;
    const sections = catalog.sections.map((s) => ({ start: s.start, count: s.count }));
    const w = width - 2 * PAD;
    const ht = catalog.targetRowHeight;
    if (w <= 0) return;
    const keep = untrack(anchor);
    void engine
      .compute({ aspects: catalog.aspects, sections, width: w, targetHeight: ht })
      .then(({ layout: next, ms }) => {
        const geometryChanged = !layout || layout.width !== next.width || untrack(() => catalog.targetRowHeight) !== ht;
        layout = next;
        catalog.layout = next;
        catalog.layoutMs = ms;
        if (keep && geometryChanged && scroller && keep.idx * 4 < next.rects.length) {
          scroller.scrollTop = Math.max(0, (next.rects[keep.idx * 4 + 1] as number) - keep.offset);
        }
        schedule();
      });
  });

  // Kürasyon (yıldız/etiket) karoları yeniden çizer.
  $effect(() => {
    void catalog.rev;
    untrack(schedule);
  });

  // Seçim görünür alana kaydırılır (ok tuşları).
  $effect(() => {
    const sel = catalog.selected;
    untrack(() => {
      if (!layout || !scroller || sel < 0) return;
      const t = revealTop(layout, sel, scroller.scrollTop, scroller.clientHeight);
      if (t !== null) scroller.scrollTop = t;
      schedule();
    });
  });

  function schedule(): void {
    if (frame) return;
    frame = requestAnimationFrame(update);
  }

  function update(): void {
    frame = 0;
    const lay = layout;
    if (!lay || !scroller) return;
    const top = scroller.scrollTop;
    const vh = scroller.clientHeight;
    speed.sample(top, performance.now(), vh);
    const fast = speed.fast;
    const range = visibleRange(lay, top, vh);
    const need = range.end - range.start;
    if (need > pool) pool = Math.max(need, Math.ceil(pool * 1.5), 64);
    const dpr = window.devicePixelRatio || 1;
    const prev = slots;
    const next: Slot[] = new Array(pool).fill(EMPTY);
    const rects = lay.rects;
    for (let idx = range.start; idx < range.end; idx++) {
      const k = idx % pool;
      const o = idx * 4;
      const h = rects[o + 3] as number;
      const cls = thumbClass(h, dpr);
      const item = catalog.items[idx];
      const old = prev.length === pool ? prev[k] : undefined;
      let src = "";
      if (item) {
        const url = catalog.backend.thumbUrl(cls, item.hash);
        if (old && old.idx === idx && old.src) {
          // Aynı öğe: yüklenmiş görüntü korunur; boyut sınıfı yalnızca yavaşken yükselir.
          src = fast ? old.src : url;
        } else {
          // Düğüm başka öğeye geçti: eski src bırakılır, hızlı kaydırmada yeni istek yok.
          src = fast ? "" : url;
        }
      }
      next[k] = {
        idx,
        id: item?.id,
        x: (rects[o] as number) + PAD,
        y: rects[o + 1] as number,
        w: rects[o + 2] as number,
        h,
        src,
        cls,
        ready: item !== undefined,
        rating: item?.rating ?? 0,
        label: item?.label ?? null,
        favorite: item?.favorite ?? false,
        raw: item?.raw ?? null,
        video: item?.video ?? false,
        sync: item?.sync ?? "local",
      };
    }
    slots = next;
    headers = range.headers;

    clearTimeout(settleTimer);
    if (fast) {
      settleTimer = setTimeout(() => {
        speed.reset();
        schedule();
      }, 120);
    }
    clearTimeout(prioTimer);
    prioTimer = setTimeout(() => prioritize(lay), PRIORITIZE_DEBOUNCE_MS);
  }

  /** Görünür (taşmasız) öğelerin küçük resim işlerini öne alır. */
  function prioritize(lay: JustifiedLayout): void {
    if (!scroller || lay !== layout) return;
    const r = visibleRange(lay, scroller.scrollTop, scroller.clientHeight, 0);
    const key = `${r.start}:${r.end}`;
    if (key === lastPrio) return;
    lastPrio = key;
    const ids: number[] = [];
    for (let i = r.start; i < r.end; i++) {
      const it = catalog.items[i];
      if (it) ids.push(it.id);
    }
    if (ids.length > 0) catalog.backend.prioritizeViewport(ids).catch(() => undefined);
  }

  function hit(e: MouseEvent): number {
    const el = (e.target as HTMLElement | null)?.closest<HTMLElement>("[data-idx]");
    return el ? Number(el.dataset.idx) : -1;
  }

  function onclick(e: MouseEvent): void {
    const idx = hit(e);
    if (idx >= 0) catalog.clickSelect(idx, { toggle: e.ctrlKey || e.metaKey, range: e.shiftKey });
  }

  function ondblclick(e: MouseEvent): void {
    const idx = hit(e);
    if (idx < 0) return;
    catalog.select(idx);
    catalog.loupeOpen = true;
  }

  onDestroy(() => {
    cancelAnimationFrame(frame);
    clearTimeout(settleTimer);
    clearTimeout(prioTimer);
  });
</script>

<div
  bind:this={scroller}
  bind:clientWidth={width}
  class="relative h-full min-w-0 flex-1 overflow-y-auto bg-canvas"
  onscroll={schedule}
  role="grid"
  aria-label="Zaman çizelgesi"
  aria-rowcount={layout?.rowTops.length ?? 0}
  tabindex="0"
>
  {#if catalog.opened && catalog.total === 0 && !catalog.error}
    <div class="flex h-full flex-col items-start justify-center gap-3 px-8">
      <p class="label">Bu görünüm boş</p>
      <p class="max-w-96 text-sm text-ink-2">
        Fotoğraf klasörü ekleyin veya bir kartı içe aktarın. Dosyalar taşınmadan da (referans) kataloglanabilir.
      </p>
      {#if onAddFolder}<button class="btn" onclick={onAddFolder}>Klasör ekle</button>{/if}
    </div>
  {:else if layout}
    <!-- svelte-ignore a11y_click_events_have_key_events (klavye: App.svelte genel kısayolları) -->
    <div class="relative" style:height="{layout.height + PAD}px" {onclick} {ondblclick} role="presentation">
      {#each headers as s (s)}
        {@const sec = catalog.sections[s]}
        {#if sec}
          <div
            class="absolute flex h-section items-end justify-between pb-2"
            style:transform="translate3d({PAD}px, {layout.headerTops[s]}px, 0)"
            style:width="{layout.width}px"
          >
            <span class="flex items-baseline gap-2">
              <span class="text-sm font-medium text-ink">{sec.title}</span>
              {#if sec.subtitle}<span class="text-xs text-ink-3">{sec.subtitle}</span>{/if}
            </span>
            <span class="data text-2xs text-ink-3">{count(sec.count)}</span>
          </div>
        {/if}
      {/each}
      {#each slots as s, k (k)}
        <Tile
          idx={s.idx}
          src={s.src}
          x={s.x}
          y={s.y}
          w={s.w}
          h={s.h}
          selected={s.idx >= 0 && (s.idx === catalog.selected || (s.id !== undefined && catalog.marked.has(s.id)))}
          ready={s.ready}
          rating={s.rating}
          label={s.label}
          favorite={s.favorite}
          raw={s.raw}
          video={s.video}
          sync={s.sync}
        />
      {/each}
    </div>
  {/if}
</div>

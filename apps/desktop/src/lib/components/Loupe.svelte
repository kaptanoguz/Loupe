<script lang="ts">
  import Heart from "@lucide/svelte/icons/heart";
  import Play from "@lucide/svelte/icons/play";

  import { THUMB_RETRIES, THUMB_RETRY_MS } from "../design/layout";
  import { count } from "../format";
  import { t } from "../i18n";
  import type { Catalog } from "../state/catalog.svelte";

  interface Props {
    catalog: Catalog;
  }

  let { catalog }: Props = $props();

  const item = $derived(catalog.selectedItem);

  function play(): void {
    if (item) void catalog.openExternal(item.id);
  }
  // Izgarada zaten yüklenmiş küçük boyut anında görünür; lg gelince üstüne geçer.
  const low = $derived(item ? catalog.backend.thumbUrl("sm", item.hash) : "");
  const highBase = $derived(item ? catalog.backend.thumbUrl("lg", item.hash) : "");
  // lg henüz üretilmediyse (404) çekirdek onu öne alır; kısa aralıkla yeniden denenir.
  let attempt = $state(0);
  let retryTimer: ReturnType<typeof setTimeout> | undefined;
  const high = $derived(attempt > 0 && highBase ? `${highBase}?r=${attempt}` : highBase);

  function onerror(): void {
    if (attempt >= THUMB_RETRIES) return;
    clearTimeout(retryTimer);
    retryTimer = setTimeout(() => (attempt += 1), THUMB_RETRY_MS * 2 ** attempt);
  }

  // 1:1'de tam çözünürlük (thumb://orig) arka planda yüklenir; gelene dek lg
  // ekranda kalır, gelince aynı nokta merkezde kalacak şekilde değiştirilir.
  let orig = $state.raw<{ id: number; src: string; w: number; h: number } | null>(null);
  let origFailed = $state(false);

  let stage = $state<HTMLDivElement>();
  let stageW = $state(0);
  let stageH = $state(0);
  let natural = $state({ w: 0, h: 0 });
  let loadedSrc = $state("");
  let pan = $state({ x: 0, y: 0 });
  let drag: { x: number; y: number; px: number; py: number; moved: boolean } | null = null;

  const dpr = typeof window === "undefined" ? 1 : window.devicePixelRatio || 1;
  const full = $derived(orig && orig.id === item?.id ? orig : null);
  /** 1:1 = görüntünün her pikseli bir ekran pikseli (tam çözünürlük geldiyse onun). */
  const actual = $derived(full ? { w: full.w / dpr, h: full.h / dpr } : { w: natural.w / dpr, h: natural.h / dpr });
  const fitScale = $derived(
    natural.w > 0 && stageW > 0 ? Math.min((stageW * dpr) / natural.w, (stageH * dpr) / natural.h) : 1,
  );
  const percent = $derived(
    Math.round((catalog.loupeActual ? 1 : full ? (fitScale * natural.w) / full.w : fitScale) * 100),
  );

  // Öğe değişince kaydırma ve yeniden deneme sıfırlanır.
  $effect(() => {
    void item?.id;
    pan = { x: 0, y: 0 };
    attempt = 0;
    origFailed = false;
    return () => clearTimeout(retryTimer);
  });

  $effect(() => {
    const it = item;
    if (!catalog.loupeActual || !it || natural.w === 0 || (orig && orig.id === it.id)) return;
    const src = catalog.backend.thumbUrl("orig", it.hash);
    const img = new Image();
    img.decoding = "async";
    img.onload = () => {
      if (catalog.selectedItem?.id !== it.id) return;
      // Kaydırma lg piksellerindeydi: aynı noktayı korumak için ölçeklenir.
      const k = img.naturalWidth / Math.max(1, natural.w);
      orig = { id: it.id, src, w: img.naturalWidth, h: img.naturalHeight };
      pan = clampPan(pan.x * k, pan.y * k);
    };
    // Kütüphane çevrimdışı, desteklenmeyen biçim: lg ile devam edilir.
    img.onerror = () => (origFailed = true);
    img.src = src;
    return () => {
      img.onload = null;
      img.onerror = null;
    };
  });

  function clampPan(x: number, y: number): { x: number; y: number } {
    const mx = Math.max(0, (actual.w - stageW) / 2);
    const my = Math.max(0, (actual.h - stageH) / 2);
    return { x: Math.min(mx, Math.max(-mx, x)), y: Math.min(my, Math.max(-my, y)) };
  }

  function onload(e: Event): void {
    const img = e.currentTarget as HTMLImageElement;
    natural = { w: img.naturalWidth, h: img.naturalHeight };
    loadedSrc = img.src;
  }

  function onpointerdown(e: PointerEvent): void {
    drag = { x: e.clientX, y: e.clientY, px: pan.x, py: pan.y, moved: false };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }

  function onpointermove(e: PointerEvent): void {
    if (!drag) return;
    const dx = e.clientX - drag.x;
    const dy = e.clientY - drag.y;
    if (Math.abs(dx) + Math.abs(dy) > 3) drag.moved = true;
    if (catalog.loupeActual && drag.moved) pan = clampPan(drag.px + dx, drag.py + dy);
  }

  function onpointerup(e: PointerEvent): void {
    const d = drag;
    drag = null;
    if (!d || d.moved || !stage) return;
    // Tıklama: sığdır ↔ 1:1, tıklanan nokta imlecin altında kalır.
    if (!catalog.loupeActual && natural.w > 0) {
      const r = stage.getBoundingClientRect();
      const cx = e.clientX - r.left - r.width / 2;
      const cy = e.clientY - r.top - r.height / 2;
      // Sığdırılmış görüntüden 1:1'e büyüme oranı (tam çözünürlük önbellekteyse ona göre).
      const k = actual.w / ((natural.w / dpr) * fitScale);
      catalog.loupeActual = true;
      pan = clampPan(-cx * k + cx, -cy * k + cy);
    } else {
      catalog.loupeActual = false;
      pan = { x: 0, y: 0 };
    }
  }

  function onwheel(e: WheelEvent): void {
    if (!catalog.loupeActual) return;
    e.preventDefault();
    pan = clampPan(pan.x - e.deltaX, pan.y - e.deltaY);
  }
</script>

<div class="absolute inset-0 z-10 flex flex-col bg-canvas" role="dialog" aria-label="Loupe" aria-modal="true">
  <div
    bind:this={stage}
    bind:clientWidth={stageW}
    bind:clientHeight={stageH}
    class="relative min-h-0 flex-1 overflow-hidden select-none {catalog.loupeActual ? 'cursor-grab' : 'cursor-zoom-in'}"
    {onpointerdown}
    {onpointermove}
    {onpointerup}
    {onwheel}
    oncontextmenu={(e) => {
      e.preventDefault();
      catalog.menu = { x: e.clientX, y: e.clientY };
    }}
    role="presentation"
  >
    {#if item}
      {#if loadedSrc !== high}
        <img src={low} alt="" class="absolute inset-0 h-full w-full object-contain" draggable="false" />
      {/if}
      {#if catalog.loupeActual && natural.w > 0}
        <img
          src={full ? full.src : high}
          alt=""
          onload={full ? undefined : onload}
          onerror={full ? undefined : onerror}
          draggable="false"
          class="absolute top-1/2 left-1/2 max-w-none"
          style:width="{actual.w}px"
          style:height="{actual.h}px"
          style:transform="translate3d(calc(-50% + {pan.x}px), calc(-50% + {pan.y}px), 0)"
        />
      {:else}
        <img
          src={high}
          alt=""
          {onload}
          {onerror}
          draggable="false"
          class="absolute inset-0 h-full w-full object-contain"
          class:invisible={loadedSrc !== high}
        />
      {/if}
    {/if}
    {#if item?.video}
      <button
        class="btn btn-primary absolute top-1/2 left-1/2 h-10 -translate-x-1/2 -translate-y-1/2 px-4"
        onpointerdown={(e) => e.stopPropagation()}
        onclick={play}
        title={t("loupe.playTitle")}
      >
        <Play size={16} strokeWidth={1.5} />{t("menu.play")}
      </button>
    {/if}
  </div>
  <div class="data flex h-6 shrink-0 items-center justify-between border-t border-line bg-surface px-3 text-xs text-ink-2">
    <span>
      {#if item}{count(catalog.selected + 1)} / {count(catalog.total)}{/if}
    </span>
    <span class="flex items-center gap-2">
      {#if item}
        <button
          class="flex items-center gap-1 hover:text-ink {item.favorite ? 'text-ink' : ''}"
          title={t("insp.favoriteKey")}
          aria-label={item.favorite ? t("menu.unfavorite") : t("menu.favorite")}
          aria-pressed={item.favorite}
          onclick={() => catalog.toggleFavorite([item.id])}
        >
          <Heart size={12} strokeWidth={1.5} class={item.favorite ? "fill-ink" : ""} />{item.favorite ? t("tile.favorite") : t("loupe.fFavorite")}
        </button>
      {/if}
      {#if item && item.rating > 0}<span class="text-ink">{"★".repeat(item.rating)}</span>{/if}
    </span>
    <span>
      <span class={catalog.loupeActual ? "text-amber" : ""}>{catalog.loupeActual ? "1:1" : t("loupe.fit")}</span>
      {#if catalog.loupeActual}
        · {full ? `${full.w}×${full.h}` : origFailed ? t("loupe.origFailed") : t("loupe.origLoading")}
      {/if}
      · {percent}% · {t("loupe.keys")}
    </span>
  </div>
</div>

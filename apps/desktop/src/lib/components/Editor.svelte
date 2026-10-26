<script lang="ts">
  import Crop from "@lucide/svelte/icons/crop";
  import ExternalLink from "@lucide/svelte/icons/external-link";
  import FileOutput from "@lucide/svelte/icons/file-output";
  import RotateCcw from "@lucide/svelte/icons/rotate-ccw";
  import RotateCw from "@lucide/svelte/icons/rotate-cw";
  import SunMedium from "@lucide/svelte/icons/sun-medium";
  import Undo2 from "@lucide/svelte/icons/undo-2";
  import { onMount } from "svelte";

  import type { EditCrop, EditRecipe, EditStateDto, TileItem } from "../api/types";
  import {
    FULL,
    IDENTITY,
    MAX_STRAIGHTEN,
    MIN_CROP,
    clampCrop,
    coverScale,
    fitRatio,
    isIdentity,
    normalized,
    rotateCrop,
    sameRecipe,
    toneTables,
  } from "../edit/recipe";
  import { t } from "../i18n";
  import type { Catalog } from "../state/catalog.svelte";

  interface Props {
    catalog: Catalog;
  }

  let { catalog }: Props = $props();

  // Düzenleme açıkken seçim değişmez; açılıştaki öğe sabitlenir.
  // svelte-ignore state_referenced_locally
  const item = catalog.selectedItem as TileItem;

  let saved = $state.raw<EditStateDto | null>(null);
  let recipe = $state.raw<EditRecipe>(IDENTITY);
  let cropMode = $state(false);
  let compare = $state(false);
  let busy = $state(false);
  /** Kaydedilmemiş değişiklik varken ilk Esc uyarır, ikincisi vazgeçer. */
  let confirmDiscard = $state(false);
  let discardTimer: ReturnType<typeof setTimeout> | undefined;

  type RatioKey = "free" | "orig" | "1:1" | "4:5" | "3:2" | "16:9";
  const RATIOS: { key: RatioKey; value: number | null }[] = [
    { key: "free", value: null },
    { key: "orig", value: null },
    { key: "1:1", value: 1 },
    { key: "4:5", value: 4 / 5 },
    { key: "3:2", value: 3 / 2 },
    { key: "16:9", value: 16 / 9 },
  ];
  let ratio = $state<RatioKey>("free");
  let portrait = $state(false);

  let img = $state.raw<HTMLImageElement | null>(null);
  let canvas = $state<HTMLCanvasElement>();
  let stageW = $state(0);
  let stageH = $state(0);
  const dpr = typeof window === "undefined" ? 1 : window.devicePixelRatio || 1;
  /** Kırpma modunda çerçevenin çevresinde bırakılan boşluk (CSS px). */
  const MARGIN = 24;

  const dirty = $derived(saved !== null && !sameRecipe(normalized(recipe), saved.recipe));
  const edited = $derived(saved !== null && saved.revision > 0);

  // Döndürülmüş çerçevenin piksel boyutu (lg küçük resim uzayında).
  const frame = $derived.by(() => {
    if (!img) return { w: 0, h: 0 };
    const odd = recipe.quarter % 2 === 1;
    return odd ? { w: img.naturalHeight, h: img.naturalWidth } : { w: img.naturalWidth, h: img.naturalHeight };
  });
  const crop = $derived(recipe.crop ?? FULL);

  /** Ekranda gösterilen çerçeve bölgesi: kırpmada tüm çerçeve, değilse kırpılan alan. */
  const view = $derived.by(() => {
    const { w, h } = frame;
    if (w === 0 || stageW === 0) return null;
    const whole = cropMode || compare;
    const r = whole ? { x: 0, y: 0, w, h } : { x: crop.x * w, y: crop.y * h, w: crop.w * w, h: crop.h * h };
    const m = cropMode ? MARGIN : 0;
    const scale = Math.min((stageW - 2 * m) / r.w, (stageH - 2 * m) / r.h);
    return { r, scale };
  });

  /** Kırpma dikdörtgeni ekran (CSS px) koordinatlarında. */
  const cropBox = $derived.by(() => {
    if (!view) return null;
    const { r, scale } = view;
    const toX = (fx: number) => stageW / 2 + (fx - (r.x + r.w / 2)) * scale;
    const toY = (fy: number) => stageH / 2 + (fy - (r.y + r.h / 2)) * scale;
    const left = toX(crop.x * frame.w);
    const top = toY(crop.y * frame.h);
    return {
      left,
      top,
      width: toX((crop.x + crop.w) * frame.w) - left,
      height: toY((crop.y + crop.h) * frame.h) - top,
      frame: { left: toX(0), top: toY(0), width: toX(frame.w) - toX(0), height: toY(frame.h) - toY(0) },
    };
  });

  const tables = $derived(toneTables(recipe));
  const toned = $derived(!compare && (recipe.brightness !== 0 || recipe.contrast !== 0 || recipe.temperature !== 0));

  /** Kilitli oran (genişlik / yükseklik, piksel); serbestse `null`. */
  const ratioValue = $derived.by(() => {
    if (ratio === "free") return null;
    const base = ratio === "orig" ? (img ? img.naturalWidth / img.naturalHeight : 1.5) : (RATIOS.find((x) => x.key === ratio)?.value ?? 1);
    const long = Math.max(base, 1 / base);
    return portrait ? 1 / long : long;
  });

  /** Çıktının piksel boyutu (lg önizlemesinde; dışa aktarma orijinal çözünürlükte). */
  const outSize = $derived(frame.w > 0 ? `${Math.round(crop.w * frame.w)}×${Math.round(crop.h * frame.h)}` : "");

  onMount(() => {
    void load();
    const im = new Image();
    im.decoding = "async";
    im.onload = () => (img = im);
    im.onerror = () => catalog.fail(t("edit.loadFailed"));
    // Düzenlenmemiş önizleme: tarif üstüne canlı uygulanır.
    im.src = catalog.backend.thumbUrl("lg", item.hash);
    return () => {
      im.onload = null;
      im.onerror = null;
      clearTimeout(discardTimer);
    };
  });

  async function load(): Promise<void> {
    try {
      const s = await catalog.backend.editState(item.id);
      saved = s;
      recipe = s.recipe;
    } catch (e) {
      catalog.fail(e);
      close();
    }
  }

  // Canvas: döndürme + düzleştirme + kırpma. Ton, CSS'teki SVG süzgecinde.
  $effect(() => {
    const c = canvas;
    const v = view;
    const im = img;
    if (!c || !v || !im) return;
    const w = Math.max(1, Math.round(stageW * dpr));
    const h = Math.max(1, Math.round(stageH * dpr));
    if (c.width !== w) c.width = w;
    if (c.height !== h) c.height = h;
    const ctx = c.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, w, h);
    ctx.imageSmoothingEnabled = true;
    ctx.imageSmoothingQuality = "high";
    const s = v.scale * dpr;
    if (compare) {
      // Önce: orijinal, döndürülmeden, aynı bölgeye sığdırılmış.
      const k = Math.min(w / im.naturalWidth, h / im.naturalHeight);
      ctx.drawImage(im, (w - im.naturalWidth * k) / 2, (h - im.naturalHeight * k) / 2, im.naturalWidth * k, im.naturalHeight * k);
      return;
    }
    const { r } = v;
    // Kırpma bölgesi (clip) save/restore ile bu çizimle sınırlı; yoksa sonraki clearRect de onunla kısıtlanır.
    ctx.save();
    ctx.translate(w / 2, h / 2);
    ctx.scale(s, s);
    ctx.translate(-(r.x + r.w / 2), -(r.y + r.h / 2));
    // Kırpmada tüm çerçeve (dışı karartılır), değilse yalnızca kırpılan alan.
    ctx.beginPath();
    ctx.rect(r.x, r.y, r.w, r.h);
    ctx.clip();
    ctx.translate(frame.w / 2, frame.h / 2);
    ctx.rotate((recipe.straighten * Math.PI) / 180);
    const cover = coverScale(frame.w, frame.h, recipe.straighten);
    ctx.scale(cover, cover);
    ctx.rotate((recipe.quarter * Math.PI) / 2);
    ctx.drawImage(im, -im.naturalWidth / 2, -im.naturalHeight / 2);
    ctx.restore();
  });

  function set(patch: Partial<EditRecipe>): void {
    recipe = { ...recipe, ...patch };
    confirmDiscard = false;
  }

  function rotate(dir: 1 | -1): void {
    set({ quarter: (recipe.quarter + dir + 4) % 4, crop: rotateCrop(recipe.crop, dir) });
    portrait = !portrait;
  }

  function chooseRatio(key: RatioKey): void {
    const c = recipe.crop ?? FULL;
    ratio = key;
    portrait = c.h * frame.h > c.w * frame.w;
    cropMode = true;
    applyRatio();
  }

  function setTone(key: "brightness" | "contrast" | "temperature", value: number): void {
    set({ [key]: value } as Pick<EditRecipe, typeof key>);
  }

  function applyRatio(): void {
    const v = ratioValue;
    if (v === null || frame.w === 0) return;
    set({ crop: fitRatio(recipe.crop ?? FULL, v, frame.w / frame.h) });
  }

  function flipRatio(): void {
    portrait = !portrait;
    if (ratio === "free") {
      // Serbestte mevcut dikdörtgenin oranı çevrilir.
      const c = recipe.crop ?? FULL;
      const px = (c.w * frame.w) / (c.h * frame.h);
      set({ crop: fitRatio(FULL, 1 / px, frame.w / frame.h) });
    } else applyRatio();
  }

  async function auto(): Promise<void> {
    try {
      const a = await catalog.backend.autoTone(item.id);
      set({ brightness: a.brightness, contrast: a.contrast, temperature: a.temperature });
    } catch (e) {
      catalog.fail(e);
    }
  }

  function close(): void {
    catalog.editorOpen = false;
  }

  async function save(): Promise<EditStateDto | null> {
    if (busy) return null;
    busy = true;
    try {
      const s = await catalog.backend.saveEdit(item.id, normalized(recipe));
      saved = s;
      recipe = s.recipe;
      catalog.applyEdit(s);
      if (s.sidecar === "failed") catalog.error = t("edit.sidecarFailed");
      return s;
    } catch (e) {
      catalog.fail(e);
      return null;
    } finally {
      busy = false;
    }
  }

  async function saveAndClose(): Promise<void> {
    if (!dirty) {
      close();
      return;
    }
    const s = await save();
    if (!s) return;
    if (s.sidecar !== "failed") catalog.flash(s.revision > 0 ? t("edit.saved") : t("edit.reverted"));
    close();
  }

  async function revert(): Promise<void> {
    if (busy) return;
    busy = true;
    try {
      const s = await catalog.backend.revertEdit(item.id);
      saved = s;
      recipe = s.recipe;
      cropMode = false;
      ratio = "free";
      catalog.applyEdit(s);
      catalog.flash(t("edit.reverted"));
    } catch (e) {
      catalog.fail(e);
    } finally {
      busy = false;
    }
  }

  /** Dışa aktarma kayıtlı tarifi kullanır: değişiklik varsa önce kaydedilir. */
  async function exportJpeg(): Promise<void> {
    if (dirty && !(await save())) return;
    try {
      const r = await catalog.backend.exportEdit(item.id);
      if (!r) return;
      catalog.flash(
        r.lowRes
          ? t("edit.exportedLowRes", { path: r.path, size: `${r.width}×${r.height}` })
          : t("edit.exported", { path: r.path }),
      );
    } catch (e) {
      catalog.fail(e);
    }
  }

  function cancel(): void {
    if (cropMode) {
      cropMode = false;
      return;
    }
    if (dirty && !confirmDiscard) {
      confirmDiscard = true;
      clearTimeout(discardTimer);
      discardTimer = setTimeout(() => (confirmDiscard = false), 3000);
      return;
    }
    close();
  }

  // ───────────────────────────── Kırpma sürükleme ─────────────────────────────

  type Handle = "move" | "n" | "s" | "e" | "w" | "ne" | "nw" | "se" | "sw";
  const CORNERS: Handle[] = ["nw", "ne", "sw", "se"];
  const EDGES: Handle[] = ["n", "s", "e", "w"];
  let drag: { handle: Handle; x: number; y: number; start: EditCrop } | null = null;

  function startDrag(e: PointerEvent, handle: Handle): void {
    e.stopPropagation();
    e.preventDefault();
    drag = { handle, x: e.clientX, y: e.clientY, start: recipe.crop ?? FULL };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }

  function onDrag(e: PointerEvent): void {
    if (!drag || !view) return;
    const dx = (e.clientX - drag.x) / (view.scale * frame.w);
    const dy = (e.clientY - drag.y) / (view.scale * frame.h);
    const s = drag.start;
    if (drag.handle === "move") {
      set({ crop: clampCrop({ ...s, x: s.x + dx, y: s.y + dy }) });
      return;
    }
    const hd = drag.handle;
    let x0 = s.x;
    let y0 = s.y;
    let x1 = s.x + s.w;
    let y1 = s.y + s.h;
    if (hd.includes("w")) x0 = Math.min(x1 - MIN_CROP, Math.max(0, x0 + dx));
    if (hd.includes("e")) x1 = Math.max(x0 + MIN_CROP, Math.min(1, x1 + dx));
    if (hd.includes("n")) y0 = Math.min(y1 - MIN_CROP, Math.max(0, y0 + dy));
    if (hd.includes("s")) y1 = Math.max(y0 + MIN_CROP, Math.min(1, y1 + dy));
    let w = x1 - x0;
    let h = y1 - y0;
    const v = ratioValue;
    if (v !== null) {
      // Oran kilitli: köşe sürüklemesi, karşı köşe sabit kalacak şekilde küçültülür.
      const want = v / (frame.w / frame.h);
      if (w / h > want) w = h * want;
      else h = w / want;
      if (hd.includes("w")) x0 = x1 - w;
      if (hd.includes("n")) y0 = y1 - h;
    }
    set({ crop: { x: x0, y: y0, w, h } });
  }

  function endDrag(): void {
    drag = null;
  }

  // ───────────────────────────── Klavye ─────────────────────────────

  function onkeydown(e: KeyboardEvent): void {
    const target = e.target as HTMLElement | null;
    const typing = target instanceof HTMLInputElement && target.type !== "range";
    if (typing) return;
    const mod = e.ctrlKey || e.metaKey;
    let handled = true;
    if (mod && (e.key === "s" || e.key === "S")) void saveAndClose();
    else if (mod || e.altKey) handled = false;
    else if (e.key === "Escape") cancel();
    else if (e.key === "Enter") void saveAndClose();
    else if (e.key === "r") rotate(1);
    else if (e.key === "R") rotate(-1);
    else if (e.key === "c" || e.key === "C") cropMode = !cropMode;
    else if (e.key === "x" || e.key === "X") flipRatio();
    else if (e.key === "a" || e.key === "A") void auto();
    else if (e.key === "\\") compare = !compare;
    else handled = false;
    if (handled) {
      e.preventDefault();
      e.stopPropagation();
    }
  }

  const sliders = $derived([
    { key: "brightness", label: t("edit.brightness"), value: recipe.brightness },
    { key: "contrast", label: t("edit.contrast"), value: recipe.contrast },
    { key: "temperature", label: t("edit.temperature"), value: recipe.temperature },
  ] as const);

  function signed(v: number, digits = 0): string {
    const s = v.toFixed(digits);
    return v > 0 ? `+${s}` : s;
  }
</script>

<svelte:window {onkeydown} />

<!-- Ton önizlemesi: çekirdekteki piksel tablosunun aynısı, sRGB uzayında. -->
<svg width="0" height="0" class="absolute" aria-hidden="true">
  <filter id="loupe-tone" color-interpolation-filters="sRGB">
    <feComponentTransfer>
      <feFuncR type="table" tableValues={tables[0]} />
      <feFuncG type="table" tableValues={tables[1]} />
      <feFuncB type="table" tableValues={tables[2]} />
    </feComponentTransfer>
  </filter>
</svg>

<div class="fixed inset-0 z-30 flex bg-canvas text-ink" role="dialog" aria-label={t("edit.title")} aria-modal="true">
  <div class="flex min-w-0 flex-1 flex-col">
    <div
      class="relative min-h-0 flex-1 overflow-hidden select-none"
      bind:clientWidth={stageW}
      bind:clientHeight={stageH}
      role="presentation"
    >
      <canvas
        bind:this={canvas}
        class="absolute inset-0 h-full w-full"
        style:filter={toned ? "url(#loupe-tone)" : "none"}
      ></canvas>
      {#if cropMode && cropBox && !compare}
        {@const b = cropBox}
        <!-- Kırpma dışı karartılır (dört şerit, gölge yok). -->
        <div class="pointer-events-none absolute inset-0">
          <div class="scrim absolute" style:left="{b.frame.left}px" style:top="{b.frame.top}px" style:width="{b.frame.width}px" style:height="{b.top - b.frame.top}px"></div>
          <div class="scrim absolute" style:left="{b.frame.left}px" style:top="{b.top + b.height}px" style:width="{b.frame.width}px" style:height="{b.frame.top + b.frame.height - b.top - b.height}px"></div>
          <div class="scrim absolute" style:left="{b.frame.left}px" style:top="{b.top}px" style:width="{b.left - b.frame.left}px" style:height="{b.height}px"></div>
          <div class="scrim absolute" style:left="{b.left + b.width}px" style:top="{b.top}px" style:width="{b.frame.left + b.frame.width - b.left - b.width}px" style:height="{b.height}px"></div>
        </div>
        <div
          class="absolute cursor-move border border-ink"
          style:left="{b.left}px"
          style:top="{b.top}px"
          style:width="{b.width}px"
          style:height="{b.height}px"
          role="presentation"
          onpointerdown={(e) => startDrag(e, "move")}
          onpointermove={onDrag}
          onpointerup={endDrag}
        >
          <!-- Üçler kuralı -->
          <div class="pointer-events-none absolute inset-y-0 left-1/3 border-l border-ink/40"></div>
          <div class="pointer-events-none absolute inset-y-0 left-2/3 border-l border-ink/40"></div>
          <div class="pointer-events-none absolute inset-x-0 top-1/3 border-t border-ink/40"></div>
          <div class="pointer-events-none absolute inset-x-0 top-2/3 border-t border-ink/40"></div>
          {#each ratio === "free" ? [...CORNERS, ...EDGES] : CORNERS as hd (hd)}
            <div
              class="absolute size-2.5 border border-canvas bg-ink"
              class:cursor-nwse-resize={hd === "nw" || hd === "se"}
              class:cursor-nesw-resize={hd === "ne" || hd === "sw"}
              class:cursor-ns-resize={hd === "n" || hd === "s"}
              class:cursor-ew-resize={hd === "e" || hd === "w"}
              style:left={hd.includes("w") ? "-5px" : hd.includes("e") ? "calc(100% - 5px)" : "calc(50% - 5px)"}
              style:top={hd.includes("n") ? "-5px" : hd.includes("s") ? "calc(100% - 5px)" : "calc(50% - 5px)"}
              role="presentation"
              onpointerdown={(e) => startDrag(e, hd)}
              onpointermove={onDrag}
              onpointerup={endDrag}
            ></div>
          {/each}
        </div>
      {/if}
      {#if compare}
        <span class="data absolute top-3 left-3 bg-surface px-2 py-0.5 text-2xs text-ink-2">{t("edit.before")}</span>
      {/if}
    </div>
    <div class="data flex h-6 shrink-0 items-center justify-between border-t border-line bg-surface px-3 text-xs text-ink-2">
      <span>
        {outSize}
        {#if edited}· <span class="text-ink">{t("edit.edited")}</span>{/if}
        {#if dirty}· <span class="text-amber">{t("edit.unsaved")}</span>{/if}
      </span>
      <span class={confirmDiscard ? "text-amber" : ""}>{confirmDiscard ? t("edit.confirmDiscard") : t("edit.keys")}</span>
    </div>
  </div>

  <aside class="flex w-72 shrink-0 flex-col border-l border-line bg-surface">
    <div class="min-h-0 flex-1 space-y-5 overflow-y-auto p-3">
      <section class="space-y-2">
        <h2 class="label">{t("edit.geometry")}</h2>
        <div class="flex gap-1">
          <button class="btn flex-1 justify-center" title="{t('edit.rotateLeft')} (Shift+R)" aria-label={t("edit.rotateLeft")} onclick={() => rotate(-1)}>
            <RotateCcw size={14} strokeWidth={1.5} />
          </button>
          <button class="btn flex-1 justify-center" title="{t('edit.rotateRight')} (R)" aria-label={t("edit.rotateRight")} onclick={() => rotate(1)}>
            <RotateCw size={14} strokeWidth={1.5} />
          </button>
          <button
            class="btn flex-1 justify-center {cropMode ? 'border-amber text-amber' : ''}"
            title="{t('edit.crop')} (C)"
            aria-pressed={cropMode}
            onclick={() => (cropMode = !cropMode)}
          >
            <Crop size={14} strokeWidth={1.5} />{t("edit.crop")}
          </button>
        </div>
        {#if cropMode}
          <div class="flex flex-wrap gap-1" role="group" aria-label={t("edit.ratio")}>
            {#each RATIOS as r (r.key)}
              <button
                class="btn h-6 px-2 text-xs {ratio === r.key ? 'border-amber text-amber' : ''}"
                aria-pressed={ratio === r.key}
                onclick={() => chooseRatio(r.key)}
              >
                {r.key === "free" ? t("edit.ratioFree") : r.key === "orig" ? t("edit.ratioOriginal") : r.key}
              </button>
            {/each}
            <button class="btn h-6 px-2 text-xs" title="X" onclick={flipRatio}>{t("edit.ratioFlip")}</button>
          </div>
        {/if}
        <label class="block space-y-1">
          <span class="flex justify-between text-xs text-ink-2">
            {t("edit.straighten")}<span class="data text-ink">{signed(recipe.straighten, 1)}°</span>
          </span>
          <input
            type="range"
            class="slider w-full"
            min={-MAX_STRAIGHTEN}
            max={MAX_STRAIGHTEN}
            step="0.1"
            value={recipe.straighten}
            title={t("edit.resetHint")}
            oninput={(e) => set({ straighten: Number(e.currentTarget.value) })}
            ondblclick={() => set({ straighten: 0 })}
          />
        </label>
      </section>

      <section class="space-y-2">
        <div class="flex items-center justify-between">
          <h2 class="label">{t("edit.light")}</h2>
          <button class="btn h-6 px-2 text-xs" title="A" onclick={auto}>
            <SunMedium size={12} strokeWidth={1.5} />{t("edit.auto")}
          </button>
        </div>
        {#each sliders as s (s.key)}
          <label class="block space-y-1">
            <span class="flex justify-between text-xs text-ink-2">{s.label}<span class="data text-ink">{signed(s.value)}</span></span>
            <input
              type="range"
              class="slider w-full"
              min="-100"
              max="100"
              step="1"
              value={s.value}
              title={t("edit.resetHint")}
              oninput={(e) => setTone(s.key, Number(e.currentTarget.value))}
              ondblclick={() => setTone(s.key, 0)}
            />
          </label>
        {/each}
        <button class="btn h-6 w-full justify-center text-xs {compare ? 'border-amber text-amber' : ''}" aria-pressed={compare} title="\" onclick={() => (compare = !compare)}>
          {t("edit.compare")}
        </button>
      </section>

      <section class="space-y-2">
        <h2 class="label">{t("edit.output")}</h2>
        <button class="btn w-full" onclick={exportJpeg} disabled={busy}>
          <FileOutput size={14} strokeWidth={1.5} />{t("edit.export")}
        </button>
        {#each catalog.editors as ed (ed.id)}
          <button class="btn w-full" onclick={() => catalog.openIn(item.id, ed)}>
            <ExternalLink size={14} strokeWidth={1.5} />{t("edit.openIn", { name: ed.name })}
          </button>
        {/each}
        <button class="btn w-full" disabled={busy || (!edited && isIdentity(recipe))} onclick={revert}>
          <Undo2 size={14} strokeWidth={1.5} />{t("edit.revert")}
        </button>
        <p class="text-2xs text-ink-3">{t("edit.nonDestructive")}</p>
      </section>
    </div>
    <div class="flex gap-2 border-t border-line p-3">
      <button class="btn flex-1 justify-center" onclick={cancel}>{t("edit.cancel")}</button>
      <button class="btn btn-primary flex-1 justify-center" disabled={busy || saved === null} onclick={saveAndClose}>
        {t("edit.save")}
      </button>
    </div>
  </aside>
</div>

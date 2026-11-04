<script lang="ts">
  import Maximize from "@lucide/svelte/icons/maximize";
  import Minus from "@lucide/svelte/icons/minus";
  import Plus from "@lucide/svelte/icons/plus";
  import { onMount } from "svelte";

  import type { PlacePin } from "../api/types";
  import { count } from "../format";
  import { t } from "../i18n";
  import {
    type Cluster,
    type Pin,
    type Viewport,
    WORLD,
    cluster,
    fit,
    markerSize,
    pan,
    project,
    tiles,
    zoomAt,
  } from "../map/mercator";
  import world from "../map/world.json";
  import type { Catalog } from "../state/catalog.svelte";

  interface Props {
    catalog: Catalog;
  }

  let { catalog }: Props = $props();

  type MapPin = Pin & { place: PlacePin };

  let pins = $state.raw<MapPin[] | null>(null);
  let w = $state(0);
  let h = $state(0);
  let v = $state.raw<Viewport | null>(null);
  let hover = $state.raw<Cluster<MapPin> | null>(null);
  /** Sürükleme: başlangıç noktası ve tıklamayı bastıracak kadar oynadı mı. */
  let drag: { x: number; y: number; moved: boolean } | null = null;

  onMount(() => void load());

  async function load(): Promise<void> {
    try {
      const list = await catalog.backend.placePins();
      pins = list.map((p) => ({ id: p.id, count: p.count, at: project(p.lat, p.lon), place: p }));
    } catch (e) {
      catalog.fail(e);
      pins = [];
    }
  }

  // İlk yerleşim: işaretler ve boyut belli olunca hepsini sığdır.
  $effect(() => {
    if (!v && pins && w > 0 && h > 0) v = fit(pins.map((p) => p.at), w, h);
  });

  const osm = $derived(catalog.mapOptions.osm);
  const groups = $derived(v && pins ? cluster(pins, v, w, h) : []);
  const tileList = $derived(v && osm ? tiles(v, w, h) : []);
  // world.json kendi ölçüsünde (`size`); görünüm WORLD biriminde.
  const transform = $derived(
    v ? `translate(${w / 2 - v.x * v.k} ${h / 2 - v.y * v.k}) scale(${(v.k * WORLD) / world.size})` : "",
  );

  function zoomBy(factor: number, sx = w / 2, sy = h / 2): void {
    if (v) v = zoomAt(v, factor, sx, sy, w, h);
  }

  function refit(): void {
    if (pins) v = fit(pins.map((p) => p.at), w, h);
  }

  function onwheel(e: WheelEvent): void {
    e.preventDefault();
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    zoomBy(Math.exp(-e.deltaY * 0.002), e.clientX - r.left, e.clientY - r.top);
  }

  function onpointerdown(e: PointerEvent): void {
    if (e.button !== 0) return;
    drag = { x: e.clientX, y: e.clientY, moved: false };
  }

  function onpointermove(e: PointerEvent): void {
    if (!drag || !v) return;
    const dx = e.clientX - drag.x;
    const dy = e.clientY - drag.y;
    if (!drag.moved && Math.hypot(dx, dy) < 4) return;
    if (!drag.moved) (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    drag = { x: e.clientX, y: e.clientY, moved: true };
    hover = null;
    v = pan(v, dx, dy, w, h);
  }

  function onpointerup(e: PointerEvent): void {
    const el = e.currentTarget as HTMLElement;
    if (el.hasPointerCapture(e.pointerId)) el.releasePointerCapture(e.pointerId);
    // Sürükleme bittiğinde gelen tıklama bir işareti açmasın.
    if (drag?.moved) setTimeout(() => (drag = null));
    else drag = null;
  }

  function choose(c: Cluster<MapPin>): void {
    if (drag?.moved || !v) return;
    const first = c.members[0];
    if (c.members.length === 1 && first) {
      void catalog.open({ kind: "place", id: first.id }, catalog.grouping, {
        title: first.place.title,
        subtitle: first.place.subtitle,
      });
      return;
    }
    // Küme: üyelerine yakınlaş; zaten en yakındaysa imlecin çevresine.
    const next = fit(c.members.map((m) => m.at), w, h);
    v = next.k > v.k * 1.05 ? next : zoomAt(v, 4, c.x, c.y, w, h);
    hover = null;
  }

  function onkeydown(e: KeyboardEvent): void {
    const step = 80;
    const moves: Record<string, [number, number]> = {
      ArrowLeft: [step, 0],
      ArrowRight: [-step, 0],
      ArrowUp: [0, step],
      ArrowDown: [0, -step],
    };
    const m = moves[e.key];
    if (m && v) {
      e.preventDefault();
      v = pan(v, m[0], m[1], w, h);
    } else if (e.key === "+" || e.key === "=") {
      e.preventDefault();
      zoomBy(2);
    } else if (e.key === "-") {
      e.preventDefault();
      zoomBy(0.5);
    }
  }

  const label = (c: Cluster<MapPin>): string => {
    const first = c.members[0];
    return c.members.length === 1 && first
      ? `${first.place.title}: ${t("map.photos", { n: count(c.count) })}`
      : t("map.cluster", { n: count(c.members.length), photos: count(c.count) });
  };
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions (harita: sürükle, tekerlek, ok tuşları) -->
<div
  class="relative min-h-0 flex-1 cursor-grab overflow-hidden bg-canvas select-none active:cursor-grabbing"
  role="application"
  aria-label={t("map.label")}
  tabindex="0"
  bind:clientWidth={w}
  bind:clientHeight={h}
  {onwheel}
  {onpointerdown}
  {onpointermove}
  {onpointerup}
  onpointercancel={onpointerup}
  {onkeydown}
>
  {#if v}
    {#if osm}
      {#each tileList as tile (`${tile.z}/${tile.x}/${tile.y}`)}
        <img
          src="https://tile.openstreetmap.org/{tile.z}/{tile.x}/{tile.y}.png"
          alt=""
          draggable="false"
          class="map-tile pointer-events-none absolute max-w-none"
          style:left="{tile.left}px"
          style:top="{tile.top}px"
          style:width="{tile.size}px"
          style:height="{tile.size}px"
        />
      {/each}
    {:else}
      <svg class="pointer-events-none absolute inset-0" width={w} height={h} aria-hidden="true">
        <g {transform}>
          <path d={world.land} class="fill-raised" />
          <path d={world.borders} class="fill-none stroke-line-strong" stroke-width="1" vector-effect="non-scaling-stroke" />
        </g>
      </svg>
    {/if}

    {#each groups as c (c.members[0]?.id)}
      {@const s = markerSize(c.count)}
      <button
        class="absolute flex items-center justify-center border border-canvas bg-amber text-2xs font-medium text-amber-ink hover:outline hover:outline-1 hover:outline-ink"
        style:left="{c.x - s / 2}px"
        style:top="{c.y - s / 2}px"
        style:width="{s}px"
        style:height="{s}px"
        aria-label={label(c)}
        onclick={() => choose(c)}
        onpointerenter={() => (hover = c)}
        onpointerleave={() => (hover = hover === c ? null : hover)}
        onfocus={() => (hover = c)}
        onblur={() => (hover = hover === c ? null : hover)}
      >
        {#if c.members.length > 1 && s >= 18}<span class="data leading-none">{c.members.length}</span>{/if}
      </button>
    {/each}

    {#if hover}
      {@const first = hover.members[0]}
      {@const s = markerSize(hover.count)}
      <div
        class="pointer-events-none absolute z-10 w-52 border border-line bg-surface"
        style:left="{Math.min(Math.max(8, hover.x - 104), w - 216)}px"
        style:top="{hover.y - s / 2 - 8 > 190 ? hover.y - s / 2 - 8 : hover.y + s / 2 + 8}px"
        style:transform={hover.y - s / 2 - 8 > 190 ? "translateY(-100%)" : ""}
      >
        {#if hover.members.length === 1 && first}
          {#if first.place.cover}
            <img src={catalog.backend.thumbUrl("sm", first.place.cover)} alt="" class="aspect-[3/2] w-full bg-raised object-cover" />
          {/if}
          <div class="flex flex-col gap-0.5 p-2">
            <span class="truncate text-sm text-ink">{first.place.title}</span>
            {#if first.place.subtitle}<span class="truncate text-xs text-ink-2">{first.place.subtitle}</span>{/if}
            <span class="data text-2xs text-ink-3">{t("map.photos", { n: count(first.count) })}</span>
          </div>
        {:else}
          <div class="flex flex-col gap-1 p-2">
            <span class="data text-2xs text-ink-3">{label(hover)}</span>
            {#each [...hover.members].sort((a, b) => b.count - a.count).slice(0, 5) as m (m.id)}
              <div class="flex justify-between gap-2 text-xs">
                <span class="truncate text-ink">{m.place.title}</span>
                <span class="data text-ink-3">{count(m.count)}</span>
              </div>
            {/each}
            <span class="text-2xs text-ink-3">{t("map.zoomHint")}</span>
          </div>
        {/if}
      </div>
    {/if}
  {/if}

  {#if pins && pins.length === 0}
    <div class="absolute inset-0 flex items-center justify-center">
      <div class="flex max-w-sm flex-col items-start gap-2 border border-line bg-surface p-4">
        <p class="text-sm text-ink">{t("map.empty")}</p>
        <p class="text-xs text-ink-2">{t("map.emptyHint")}</p>
        <button
          class="btn"
          onclick={() => {
            catalog.settingsTab = "ai";
            catalog.overlay = "settings";
          }}>{t("map.openSettings")}</button
        >
      </div>
    </div>
  {/if}

  <div class="absolute top-3 right-3 flex flex-col border border-line bg-surface">
    <button class="btn btn-ghost h-7 px-2" onclick={() => zoomBy(2)} aria-label={t("map.zoomIn")} title={t("map.zoomIn")}>
      <Plus size={14} strokeWidth={1.5} />
    </button>
    <button class="btn btn-ghost h-7 px-2" onclick={() => zoomBy(0.5)} aria-label={t("map.zoomOut")} title={t("map.zoomOut")}>
      <Minus size={14} strokeWidth={1.5} />
    </button>
    <button class="btn btn-ghost h-7 px-2" onclick={refit} aria-label={t("map.fit")} title={t("map.fit")}>
      <Maximize size={14} strokeWidth={1.5} />
    </button>
  </div>

  <span class="pointer-events-none absolute right-2 bottom-1 text-2xs text-ink-3">
    {osm ? t("map.osmCredit") : t("map.offlineCredit")}
  </span>
</div>

<style>
  /* Açık renkli OSM karoları koyu arayüzde göz almasın. */
  .map-tile {
    filter: brightness(0.8) contrast(0.95);
  }
</style>

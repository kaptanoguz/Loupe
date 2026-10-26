<script lang="ts">
  import SlidersHorizontal from "@lucide/svelte/icons/sliders-horizontal";

  import type { Grouping, SortDir, SortKey } from "../api/types";
  import { ROW_HEIGHTS, type ZoomLevel } from "../design/layout";
  import { groupable, type Catalog } from "../state/catalog.svelte";

  interface Props {
    catalog: Catalog;
  }

  let { catalog }: Props = $props();
  let open = $state(false);
  let root: HTMLDivElement | undefined = $state();

  const SORTS: { value: SortKey; label: string }[] = [
    { value: "taken", label: "Çekim tarihi" },
    { value: "added", label: "Eklenme tarihi" },
    { value: "name", label: "Dosya adı" },
    { value: "size", label: "Dosya boyutu" },
    { value: "rating", label: "Puan" },
  ];
  const GROUPS: { value: Grouping; label: string }[] = [
    { value: "day", label: "Gün" },
    { value: "month", label: "Ay" },
    { value: "year", label: "Yıl" },
    { value: "none", label: "Yok" },
  ];
  const SIZES: { value: ZoomLevel; label: string }[] = [
    { value: 0, label: "Küçük" },
    { value: 1, label: "Orta" },
    { value: 2, label: "Büyük" },
    { value: 3, label: "Çok büyük" },
  ];

  /** Yönün okunuşu sıralamaya göre değişir. */
  const dirs = $derived.by((): { value: SortDir; label: string }[] => {
    switch (catalog.sort) {
      case "name":
        return [
          { value: "asc", label: "A → Z" },
          { value: "desc", label: "Z → A" },
        ];
      case "size":
        return [
          { value: "desc", label: "Büyükten küçüğe" },
          { value: "asc", label: "Küçükten büyüğe" },
        ];
      case "rating":
        return [{ value: "desc", label: "Yüksekten düşüğe" }];
      default:
        return [
          { value: "desc", label: "Yeniden eskiye" },
          { value: "asc", label: "Eskiden yeniye" },
        ];
    }
  });
  const sortLabel = $derived(SORTS.find((s) => s.value === catalog.sort)?.label ?? "");
  const canGroup = $derived(groupable(catalog.arrange));

  function pickSort(sort: SortKey): void {
    // Ada göre doğal yön A → Z; diğerlerinde büyük / yeni önce.
    void catalog.setArrange({ sort, dir: sort === "name" ? "asc" : "desc" });
  }

  function onwindowdown(e: MouseEvent): void {
    if (open && root && !root.contains(e.target as Node)) open = false;
  }

  function onkeydown(e: KeyboardEvent): void {
    if (e.key === "Escape" && open) {
      e.stopPropagation();
      open = false;
    }
  }
</script>

<svelte:window onmousedown={onwindowdown} />

<div class="relative" bind:this={root} role="presentation" {onkeydown}>
  <button
    class="btn btn-ghost"
    onclick={() => (open = !open)}
    aria-expanded={open}
    aria-haspopup="dialog"
    title="Sıralama, gruplama ve döşeme boyutu"
  >
    <SlidersHorizontal size={14} strokeWidth={1.5} />{sortLabel}
  </button>
  {#if open}
    <div
      class="absolute right-0 top-full z-20 mt-1 flex w-72 flex-col gap-3 rounded-sm border border-line bg-surface p-3"
      role="dialog"
      aria-label="Görünüm seçenekleri"
    >
      <fieldset class="flex flex-col gap-1">
        <legend class="label mb-1">Sırala</legend>
        <div class="grid grid-cols-2 gap-1">
          {#each SORTS as s (s.value)}
            <button
              class="btn h-6 justify-start {catalog.sort === s.value ? 'border-amber text-ink' : 'text-ink-2'}"
              aria-pressed={catalog.sort === s.value}
              onclick={() => pickSort(s.value)}>{s.label}</button
            >
          {/each}
        </div>
      </fieldset>
      <fieldset class="flex flex-col gap-1">
        <legend class="label mb-1">Yön</legend>
        <div class="flex gap-1">
          {#each dirs as d (d.value)}
            <button
              class="btn h-6 flex-1 {catalog.dir === d.value ? 'border-amber text-ink' : 'text-ink-2'}"
              aria-pressed={catalog.dir === d.value}
              onclick={() => catalog.setArrange({ dir: d.value })}>{d.label}</button
            >
          {/each}
        </div>
      </fieldset>
      <fieldset class="flex flex-col gap-1" disabled={!canGroup}>
        <legend class="label mb-1">Grupla</legend>
        <div class="flex gap-1">
          {#each GROUPS as g (g.value)}
            <button
              class="btn h-6 flex-1 {canGroup && catalog.grouping === g.value ? 'border-amber text-ink' : 'text-ink-2'}"
              aria-pressed={canGroup && catalog.grouping === g.value}
              onclick={() => catalog.setArrange({ grouping: g.value })}>{g.label}</button
            >
          {/each}
        </div>
        {#if !canGroup}
          <p class="text-2xs text-ink-3">
            {catalog.sort === "rating" ? "Puana göre sıralamada her puan ayrı bölümdür." : "Bu sıralamada tek bölüm gösterilir."}
          </p>
        {/if}
      </fieldset>
      <fieldset class="flex flex-col gap-1">
        <legend class="label mb-1">Döşeme boyutu</legend>
        <div class="flex gap-1">
          {#each SIZES as z (z.value)}
            <button
              class="btn h-6 flex-1 px-1 {catalog.zoom === z.value ? 'border-amber text-ink' : 'text-ink-2'}"
              aria-pressed={catalog.zoom === z.value}
              title="{ROW_HEIGHTS[z.value]} px satır (Ctrl + / Ctrl −)"
              onclick={() => (catalog.zoom = z.value)}>{z.label}</button
            >
          {/each}
        </div>
      </fieldset>
    </div>
  {/if}
</div>

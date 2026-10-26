<script lang="ts">
  import SlidersHorizontal from "@lucide/svelte/icons/sliders-horizontal";

  import type { Grouping, SortDir, SortKey } from "../api/types";
  import { ROW_HEIGHTS, type ZoomLevel } from "../design/layout";
  import { t } from "../i18n";
  import { groupable, type Catalog } from "../state/catalog.svelte";

  interface Props {
    catalog: Catalog;
  }

  let { catalog }: Props = $props();
  let open = $state(false);
  let root: HTMLDivElement | undefined = $state();

  const SORTS: { value: SortKey; label: string }[] = [
    { value: "taken", label: t("arrange.taken") },
    { value: "added", label: t("arrange.added") },
    { value: "name", label: t("arrange.name") },
    { value: "size", label: t("arrange.size") },
    { value: "rating", label: t("arrange.rating") },
  ];
  const GROUPS: { value: Grouping; label: string }[] = [
    { value: "day", label: t("arrange.day") },
    { value: "month", label: t("arrange.month") },
    { value: "year", label: t("arrange.year") },
    { value: "none", label: t("arrange.none") },
  ];
  const SIZES: { value: ZoomLevel; label: string }[] = [
    { value: 0, label: t("arrange.small") },
    { value: 1, label: t("arrange.medium") },
    { value: 2, label: t("arrange.large") },
    { value: 3, label: t("arrange.xlarge") },
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
          { value: "desc", label: t("arrange.bigFirst") },
          { value: "asc", label: t("arrange.smallFirst") },
        ];
      case "rating":
        return [{ value: "desc", label: t("arrange.highFirst") }];
      default:
        return [
          { value: "desc", label: t("arrange.newFirst") },
          { value: "asc", label: t("arrange.oldFirst") },
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
    title={t("arrange.title")}
  >
    <SlidersHorizontal size={14} strokeWidth={1.5} />{sortLabel}
  </button>
  {#if open}
    <div
      class="absolute right-0 top-full z-20 mt-1 flex w-72 flex-col gap-3 rounded-sm border border-line bg-surface p-3"
      role="dialog"
      aria-label={t("arrange.label")}
    >
      <fieldset class="flex flex-col gap-1">
        <legend class="label mb-1">{t("arrange.sort")}</legend>
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
        <legend class="label mb-1">{t("arrange.dir")}</legend>
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
        <legend class="label mb-1">{t("arrange.group")}</legend>
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
            {catalog.sort === "rating" ? t("arrange.ratingSections") : t("arrange.oneSection")}
          </p>
        {/if}
      </fieldset>
      <fieldset class="flex flex-col gap-1">
        <legend class="label mb-1">{t("arrange.tileSize")}</legend>
        <div class="flex gap-1">
          {#each SIZES as z (z.value)}
            <button
              class="btn h-6 flex-1 px-1 {catalog.zoom === z.value ? 'border-amber text-ink' : 'text-ink-2'}"
              aria-pressed={catalog.zoom === z.value}
              title={t("arrange.rowTitle", { px: ROW_HEIGHTS[z.value] })}
              onclick={() => (catalog.zoom = z.value)}>{z.label}</button
            >
          {/each}
        </div>
      </fieldset>
    </div>
  {/if}
</div>

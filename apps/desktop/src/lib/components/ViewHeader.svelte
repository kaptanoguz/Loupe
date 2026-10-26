<script lang="ts">
  import ArrowLeft from "@lucide/svelte/icons/arrow-left";
  import Save from "@lucide/svelte/icons/save";
  import Trash from "@lucide/svelte/icons/trash";
  import { tick } from "svelte";

  import type { View } from "../api/types";
  import { count } from "../format";
  import type { Catalog } from "../state/catalog.svelte";
  import ArrangeMenu from "./ArrangeMenu.svelte";

  interface Props {
    catalog: Catalog;
  }

  let { catalog }: Props = $props();

  let naming = $state(false);
  let name = $state("");
  let field = $state<HTMLInputElement>();

  const view = $derived(catalog.view);
  const album = $derived(
    view.kind === "smart_album" ? catalog.sidebar?.smartAlbums.find((a) => a.id === view.id) : undefined,
  );
  /** Kenar çubuğundan açılan kök görünümler: geri düğmesi yok. */
  const root = $derived(["all", "favorites", "videos", "places", "library"].includes(view.kind));
  const rootTitle = $derived.by(() => {
    switch (view.kind) {
      case "all":
        return "Tüm Fotoğraflar";
      case "favorites":
        return "Favoriler";
      case "videos":
        return "Videolar";
      case "places":
        return "Yerler";
      case "library":
        return catalog.sidebar?.libraries.find((l) => l.id === view.id)?.name ?? "Klasör";
      default:
        return "";
    }
  });
  const title = $derived(root ? rootTitle : (catalog.heading?.title ?? album?.name ?? ""));
  const parent = $derived.by((): { view: View; label: string } => {
    switch (view.kind) {
      case "person":
      case "cluster":
        return { view: { kind: "people" }, label: "Kişiler" };
      case "event":
        return { view: { kind: "events" }, label: "Etkinlikler" };
      default:
        return { view: { kind: "all" }, label: "Tüm Fotoğraflar" };
    }
  });

  async function startSave(): Promise<void> {
    naming = true;
    name = catalog.heading?.input ?? "";
    await tick();
    field?.select();
  }

  async function save(): Promise<void> {
    if (view.kind !== "search" || !name.trim()) return;
    try {
      const a = await catalog.backend.saveSmartAlbum(name.trim(), view.query, true);
      naming = false;
      await catalog.refreshSidebar();
      catalog.flash(`Akıllı albüm kaydedildi: ${a.name}`);
      void catalog.open({ kind: "smart_album", id: a.id }, catalog.grouping, { title: a.name });
    } catch (e) {
      catalog.fail(e);
    }
  }

  async function removeAlbum(): Promise<void> {
    if (view.kind !== "smart_album") return;
    try {
      await catalog.backend.deleteSmartAlbum(view.id);
      await catalog.refreshSidebar();
      catalog.flash("Akıllı albüm silindi (fotoğraflar yerinde)");
      void catalog.open({ kind: "all" });
    } catch (e) {
      catalog.fail(e);
    }
  }

  async function exclude(): Promise<void> {
    const it = catalog.selectedItem;
    if (view.kind !== "event" || !it) return;
    try {
      const card = await catalog.backend.excludeFromEvent(view.id, [it.id]);
      catalog.heading = { title: card.title, subtitle: [card.range, card.place].filter(Boolean).join(" · ") };
      catalog.flash("Fotoğraf etkinlikten çıkarıldı; etkinlik kilitlendi");
      await catalog.open(view);
    } catch (e) {
      catalog.fail(e);
    }
  }

  function onkeydown(e: KeyboardEvent): void {
    if (e.key === "Enter") {
      e.preventDefault();
      void save();
    } else if (e.key === "Escape") {
      e.preventDefault();
      naming = false;
    }
  }
</script>

<header class="flex h-toolbar shrink-0 items-center gap-3 border-b border-line bg-canvas px-3" aria-label="Görünüm başlığı">
  {#if !root}
    <button class="btn btn-ghost px-2" onclick={() => catalog.open(parent.view)} title={parent.label} aria-label="Geri: {parent.label}">
      <ArrowLeft size={14} strokeWidth={1.5} />
    </button>
  {/if}
  <div class="flex min-w-0 flex-1 items-baseline gap-2">
    <h1 class="truncate text-sm font-medium text-ink">{title}</h1>
    {#if catalog.heading?.subtitle}<span class="truncate text-xs text-ink-3">{catalog.heading.subtitle}</span>{/if}
    <span class="data text-2xs text-ink-3">{count(catalog.total)}</span>
  </div>
  {#if view.kind === "search"}
    {#if naming}
      <input
        bind:this={field}
        bind:value={name}
        {onkeydown}
        class="input w-56"
        placeholder="Albüm adı"
        aria-label="Akıllı albüm adı"
      />
      <button class="btn btn-primary" onclick={save} disabled={!name.trim()}>Kaydet</button>
    {:else}
      <button class="btn" onclick={startSave}><Save size={14} strokeWidth={1.5} />Akıllı albüm olarak kaydet</button>
    {/if}
  {:else if view.kind === "event"}
    <button class="btn" onclick={exclude} disabled={!catalog.selectedItem}>Seçili fotoğrafı çıkar</button>
  {:else if view.kind === "smart_album"}
    <button class="btn" onclick={removeAlbum}><Trash size={14} strokeWidth={1.5} />Albümü sil</button>
  {/if}
  <ArrangeMenu {catalog} />
</header>

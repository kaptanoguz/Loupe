<script lang="ts">
  import ArrowLeft from "@lucide/svelte/icons/arrow-left";
  import Pencil from "@lucide/svelte/icons/pencil";
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
  const root = $derived(["all", "favorites", "videos", "places", "library", "album"].includes(view.kind));
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
      case "album":
        return catalog.sidebar?.albums.find((a) => a.id === view.id)?.name ?? "Albüm";
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
    name = view.kind === "album" ? rootTitle : (catalog.heading?.input ?? "");
    await tick();
    field?.select();
  }

  async function save(): Promise<void> {
    if (view.kind === "album") return renameManual(view.id);
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

  async function renameManual(id: number): Promise<void> {
    if (!name.trim()) return;
    try {
      const a = await catalog.backend.renameAlbum(id, name.trim());
      naming = false;
      await catalog.refreshSidebar();
      catalog.flash(`Albüm adı: ${a.name}`);
    } catch (e) {
      catalog.fail(e);
    }
  }

  /** Elle oluşturulan albümün silinme onayı bekleniyor. */
  let confirmDelete = $state(false);

  async function deleteManual(id: number): Promise<void> {
    try {
      await catalog.backend.deleteAlbum(id);
      confirmDelete = false;
      await catalog.refreshSidebar();
      catalog.flash("Albüm silindi (fotoğraflar yerinde)");
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
  {:else if view.kind === "album"}
    {@const id = view.id}
    {#if naming}
      <input bind:this={field} bind:value={name} {onkeydown} class="input w-56" aria-label="Albüm adı" />
      <button class="btn btn-primary" onclick={save} disabled={!name.trim()}>Kaydet</button>
    {:else if confirmDelete}
      <span class="text-xs text-ink-2">Albüm silinsin mi? Fotoğraflar yerinde kalır.</span>
      <button class="btn" onclick={() => (confirmDelete = false)}>Vazgeç</button>
      <button class="btn border-amber text-amber" onclick={() => deleteManual(id)}>Sil</button>
    {:else}
      <button class="btn" onclick={startSave}><Pencil size={14} strokeWidth={1.5} />Yeniden adlandır</button>
      <button class="btn" onclick={() => (confirmDelete = true)}><Trash size={14} strokeWidth={1.5} />Albümü sil</button>
    {/if}
  {:else if view.kind === "smart_album"}
    <button class="btn" onclick={removeAlbum}><Trash size={14} strokeWidth={1.5} />Albümü sil</button>
  {/if}
  <ArrangeMenu {catalog} />
</header>

<script lang="ts">
  import Calendar from "@lucide/svelte/icons/calendar";
  import CommandIcon from "@lucide/svelte/icons/command";
  import FolderMinus from "@lucide/svelte/icons/folder-minus";
  import FolderPlus from "@lucide/svelte/icons/folder-plus";
  import Film from "@lucide/svelte/icons/film";
  import Heart from "@lucide/svelte/icons/heart";
  import GalleryVerticalEnd from "@lucide/svelte/icons/gallery-vertical-end";
  import MapIcon from "@lucide/svelte/icons/map";
  import Plus from "@lucide/svelte/icons/plus";
  import ScanFace from "@lucide/svelte/icons/scan-face";
  import Smartphone from "@lucide/svelte/icons/smartphone";
  import SlidersHorizontal from "@lucide/svelte/icons/sliders-horizontal";
  import SquareLibrary from "@lucide/svelte/icons/square-library";
  import WandSparkles from "@lucide/svelte/icons/wand-sparkles";
  import Wifi from "@lucide/svelte/icons/wifi";
  import type { Component } from "svelte";

  import type { ImportSessionDto, LibraryDto, View, ViewKey } from "../api/types";
  import { bytes, count, shortUuid } from "../format";
  import { t } from "../i18n";
  import { type Catalog, sameView } from "../state/catalog.svelte";

  interface Props {
    catalog: Catalog;
    onAddFolder: () => void;
    onImport: () => void;
    onImportIphone: () => void;
    onRemoveLibrary: (id: number) => Promise<void>;
    searchingIphone: boolean;
    importing: ImportSessionDto | null;
  }

  let { catalog, onAddFolder, onImport, onImportIphone, onRemoveLibrary, searchingIphone, importing }: Props =
    $props();
  /** Kaldırma onayı bekleyen kütüphane. */
  let confirming = $state<number | null>(null);
  let removing = $state(false);

  async function remove(id: number): Promise<void> {
    removing = true;
    try {
      await onRemoveLibrary(id);
    } finally {
      removing = false;
      confirming = null;
    }
  }

  const VIEWS: { key: ViewKey; name: string; icon: Component }[] = [
    { key: "all", name: t("view.all"), icon: GalleryVerticalEnd },
    { key: "favorites", name: t("view.favorites"), icon: Heart },
    { key: "videos", name: t("view.videos"), icon: Film },
    { key: "events", name: t("view.events"), icon: Calendar },
    { key: "people", name: t("view.people"), icon: ScanFace },
    { key: "places", name: t("view.places"), icon: MapIcon },
  ];


  const counts = $derived(new Map(catalog.sidebar?.views.map((v) => [v.key, v.count]) ?? []));

  const libraryView = (id: number): View => ({ kind: "library", id });
  const albumView = (id: number): View => ({ kind: "smart_album", id });
  const manualView = (id: number): View => ({ kind: "album", id });

  /** Yeni albüm adı girilirken açık. */
  let naming = $state(false);
  let newName = $state("");

  async function createAlbum(): Promise<void> {
    const name = newName.trim();
    if (!name) {
      naming = false;
      return;
    }
    try {
      const a = await catalog.backend.createAlbum(name);
      naming = false;
      newName = "";
      await catalog.refreshSidebar();
      void catalog.open(manualView(a.id));
    } catch (e) {
      catalog.fail(e);
    }
  }

  function onNameKey(e: KeyboardEvent): void {
    if (e.key === "Enter") {
      e.preventDefault();
      void createAlbum();
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      naming = false;
      newName = "";
    }
  }
  const simpleView = (kind: ViewKey): View => ({ kind });

  function active(v: View): boolean {
    const cur = catalog.view;
    // Tek kişi / tek etkinlik açıkken üst görünüm vurgulu kalır.
    if (v.kind === "people" && (cur.kind === "person" || cur.kind === "cluster")) return true;
    if (v.kind === "events" && cur.kind === "event") return true;
    return sameView(v, cur);
  }

  const lan = $derived(catalog.status?.lan);
  const mac = typeof navigator !== "undefined" && /Mac/.test(navigator.platform);

  function libTitle(l: LibraryDto): string {
    const parts = [l.root ?? t("sidebar.notMounted"), `${t("sidebar.disk")} ${l.volumeUuid ?? "?"}`, `${t("app.library")} ${l.uuid}`];
    if (l.stateReason) parts.push(l.stateReason);
    return parts.join("\n");
  }

  const progress = $derived(
    importing && importing.totalFiles > 0 ? `${count(importing.doneFiles)}/${count(importing.totalFiles)}` : null,
  );
</script>

<nav class="flex h-full w-sidebar shrink-0 flex-col border-r border-line bg-surface" aria-label={t("sidebar.label")}>
  <div class="flex-1 overflow-y-auto py-2">
    <h2 class="label px-3 pt-1 pb-1">{t("settings.libraries")}</h2>
    <ul>
      {#each catalog.sidebar?.libraries ?? [] as lib (lib.id)}
        {@const view = libraryView(lib.id)}
        <li class="group relative">
          <button
            class="flex w-full items-center gap-2 px-3 py-1 text-left hover:bg-raised"
            class:bg-raised={active(view)}
            onclick={() => catalog.open(view)}
            title={libTitle(lib)}
          >
            <span
              class="size-1.5 shrink-0 border {lib.state === 'online'
                ? 'border-ink bg-ink'
                : lib.state === 'error'
                  ? 'border-amber bg-amber'
                  : 'border-ink-3'}"
              aria-label={lib.state}
            ></span>
            <span class="min-w-0 flex-1">
              <span class="block truncate text-sm {lib.state === 'online' ? 'text-ink' : 'text-ink-3'}">{lib.name}</span>
              <span class="data block truncate text-2xs text-ink-3">
                {shortUuid(lib.volumeUuid ?? lib.uuid)} · {lib.state === "online"
                  ? t("sidebar.free", { size: bytes(lib.freeBytes) })
                  : t("sidebar.offline")}
              </span>
            </span>
            <span class="data text-2xs text-ink-3 group-hover:invisible">{count(lib.mediaCount)}</span>
          </button>
          <button
            class="absolute top-1 right-2 hidden p-1 text-ink-3 group-hover:block hover:text-ink"
            onclick={() => (confirming = lib.id)}
            title={t("sidebar.removeTitle")}
            aria-label={t("sidebar.removeLabel", { name: lib.name })}
          >
            <FolderMinus size={14} strokeWidth={1.5} />
          </button>
          {#if confirming === lib.id}
            <div class="border-y border-line bg-raised px-3 py-2 text-xs" role="alertdialog" aria-label={t("sidebar.removeFolder")}>
              <p class="text-ink">
                {t("sidebar.removeBody", { n: count(lib.mediaCount) })}
              </p>
              <div class="mt-2 flex gap-2">
                <button class="btn btn-primary" disabled={removing} onclick={() => remove(lib.id)}>
                  {removing ? t("sidebar.removing") : t("common.remove")}
                </button>
                <button class="btn" disabled={removing} onclick={() => (confirming = null)}>{t("common.cancel")}</button>
              </div>
            </div>
          {/if}
        </li>
      {:else}
        <li class="px-3 py-1 text-xs text-ink-3">{t("sidebar.noLibrary")}</li>
      {/each}
    </ul>

    <h2 class="label px-3 pt-4 pb-1">{t("sidebar.views")}</h2>
    <ul>
      {#each VIEWS as v (v.key)}
        {@const view = simpleView(v.key)}
        <li>
          <button
            class="flex h-7 w-full items-center gap-2 px-3 text-left text-sm hover:bg-raised"
            class:bg-raised={active(view)}
            class:text-ink={active(view)}
            class:text-ink-2={!active(view)}
            onclick={() => catalog.open(view)}
          >
            <v.icon size={14} strokeWidth={1.5} />
            <span class="flex-1">{v.name}</span>
            <span class="data text-2xs text-ink-3">{count(counts.get(v.key) ?? 0)}</span>
          </button>
        </li>
      {/each}
    </ul>

    <div class="flex items-center justify-between pt-4 pr-2 pb-1 pl-3">
      <h2 class="label">{t("sidebar.albums")}</h2>
      <button
        class="p-1 text-ink-3 hover:text-ink"
        onclick={() => (naming = true)}
        title={t("sidebar.newAlbum")}
        aria-label={t("sidebar.newAlbum")}
      >
        <Plus size={14} strokeWidth={1.5} />
      </button>
    </div>
    <ul>
      {#if naming}
        <li class="px-3 py-1">
          <!-- svelte-ignore a11y_autofocus -->
          <input
            class="input h-6 w-full text-sm"
            bind:value={newName}
            onkeydown={onNameKey}
            onblur={() => void createAlbum()}
            placeholder={t("sidebar.albumName")}
            aria-label={t("sidebar.newAlbumName")}
            autofocus
          />
        </li>
      {/if}
      {#each catalog.sidebar?.albums ?? [] as a (a.id)}
        {@const view = manualView(a.id)}
        <li>
          <button
            class="flex h-7 w-full items-center gap-2 px-3 text-left text-sm hover:bg-raised"
            class:bg-raised={active(view)}
            class:text-ink={active(view)}
            class:text-ink-2={!active(view)}
            onclick={() => catalog.open(view)}
          >
            <SquareLibrary size={14} strokeWidth={1.5} />
            <span class="flex-1 truncate">{a.name}</span>
            <span class="data text-2xs text-ink-3">{count(a.mediaCount)}</span>
          </button>
        </li>
      {:else}
        {#if !naming}
          <li class="px-3 py-1 text-xs text-ink-3">{t("sidebar.noAlbum")}</li>
        {/if}
      {/each}
    </ul>

    <h2 class="label px-3 pt-4 pb-1">{t("sidebar.smartAlbums")}</h2>
    <ul>
      {#each catalog.sidebar?.smartAlbums ?? [] as a (a.id)}
        {@const view = albumView(a.id)}
        <li>
          <button
            class="flex h-7 w-full items-center gap-2 px-3 text-left text-sm hover:bg-raised"
            class:bg-raised={active(view)}
            class:text-ink={active(view)}
            class:text-ink-2={!active(view)}
            onclick={() => catalog.open(view)}
          >
            <WandSparkles size={14} strokeWidth={1.5} />
            <span class="truncate">{a.name}</span>
          </button>
        </li>
      {:else}
        <li class="px-3 py-1 text-xs text-ink-3">{t("sidebar.noSmartAlbum")}</li>
      {/each}
    </ul>
  </div>

  <div class="flex flex-col gap-2 border-t border-line p-3">
    {#if importing && (importing.state === "running" || importing.state === "scanning")}
      <div class="data text-2xs text-ink-2" aria-live="polite">
        <div class="flex justify-between">
          <span>{t("sidebar.importing")}</span><span>{progress ?? t("sidebar.scanning")}</span>
        </div>
        <div class="mt-1 h-px w-full bg-line">
          <div
            class="h-px bg-amber"
            style:width="{importing.totalFiles > 0 ? (100 * importing.doneFiles) / importing.totalFiles : 0}%"
          ></div>
        </div>
        <div class="mt-1 flex justify-between text-ink-3">
          <span>{bytes(importing.doneBytes)}</span><span>{t("sidebar.dupSkipped", { n: count(importing.skippedDup) })}</span>
        </div>
      </div>
    {/if}
    <div class="flex gap-2">
      <button class="btn flex-1 justify-center" onclick={onAddFolder} title={t("sidebar.addRoot")}>
        <FolderPlus size={14} strokeWidth={1.5} />{t("sidebar.folder")}
      </button>
      <button class="btn btn-primary flex-1 justify-center" onclick={onImport} title={t("sidebar.importTitle")}>{t("sidebar.import")}</button>
    </div>
    <button
      class="btn w-full justify-center"
      onclick={onImportIphone}
      disabled={searchingIphone}
      title={t("sidebar.iphoneTitle")}
    >
      <Smartphone size={14} strokeWidth={1.5} />{searchingIphone ? t("sidebar.iphoneSearching") : t("cmd.iphone")}
    </button>
  </div>

  <div class="flex h-8 shrink-0 items-stretch border-t border-line text-xs" aria-label={t("sidebar.quick")}>
    <button
      class="flex flex-1 items-center gap-2 px-3 text-ink-2 hover:bg-raised hover:text-ink"
      onclick={() => (catalog.overlay = "wifi")}
      title={t("cmd.wifi")}
    >
      <Wifi size={14} strokeWidth={1.5} />
      <span>{t("settings.phone")}</span>
      <span class="data text-2xs {lan && lan.activeUploads > 0 ? 'text-amber' : 'text-ink-3'}">
        {lan ? (lan.activeUploads > 0 ? `↓${lan.activeUploads}` : lan.pairedDevices) : ""}
      </span>
    </button>
    <button
      class="flex items-center border-l border-line px-3 text-ink-2 hover:bg-raised hover:text-ink"
      onclick={() => catalog.openSettings("libraries")}
      title={t("settings.title")}
      aria-label={t("settings.title")}
    >
      <SlidersHorizontal size={14} strokeWidth={1.5} />
    </button>
    <button
      class="data flex items-center gap-1 border-l border-line px-3 text-2xs whitespace-nowrap text-ink-3 hover:bg-raised hover:text-ink"
      onclick={() => (catalog.overlay = "palette")}
      title={t("sidebar.palette")}
      aria-label={t("sidebar.palette")}
    >
      <CommandIcon size={12} strokeWidth={1.5} />{mac ? "⌘K" : "Ctrl+K"}
    </button>
  </div>
</nav>

<script lang="ts">
  import Calendar from "@lucide/svelte/icons/calendar";
  import CommandIcon from "@lucide/svelte/icons/command";
  import FolderPlus from "@lucide/svelte/icons/folder-plus";
  import Heart from "@lucide/svelte/icons/heart";
  import GalleryVerticalEnd from "@lucide/svelte/icons/gallery-vertical-end";
  import MapIcon from "@lucide/svelte/icons/map";
  import ScanFace from "@lucide/svelte/icons/scan-face";
  import SlidersHorizontal from "@lucide/svelte/icons/sliders-horizontal";
  import WandSparkles from "@lucide/svelte/icons/wand-sparkles";
  import Wifi from "@lucide/svelte/icons/wifi";
  import type { Component } from "svelte";

  import type { ImportMode, ImportSessionDto, LibraryDto, View, ViewKey } from "../api/types";
  import { bytes, count, shortUuid } from "../format";
  import { type Catalog, sameView } from "../state/catalog.svelte";

  interface Props {
    catalog: Catalog;
    onAddFolder: () => void;
    onImport: (mode: ImportMode) => void;
    importing: ImportSessionDto | null;
  }

  let { catalog, onAddFolder, onImport, importing }: Props = $props();
  let mode = $state<ImportMode>("copy");

  const VIEWS: { key: ViewKey; name: string; icon: Component }[] = [
    { key: "all", name: "Tüm Fotoğraflar", icon: GalleryVerticalEnd },
    { key: "favorites", name: "Favoriler", icon: Heart },
    { key: "events", name: "Etkinlikler", icon: Calendar },
    { key: "people", name: "Kişiler", icon: ScanFace },
    { key: "places", name: "Yerler", icon: MapIcon },
  ];

  const MODES: { key: ImportMode; name: string; hint: string }[] = [
    { key: "copy", name: "Kopyala", hint: "Kaynak dokunulmadan kalır" },
    { key: "move", name: "Taşı", hint: "Doğrulandıktan sonra kaynaktan silinir" },
    { key: "hardlink", name: "Bağla", hint: "Aynı diskte hardlink, ek yer kaplamaz" },
  ];

  const counts = $derived(new Map(catalog.sidebar?.views.map((v) => [v.key, v.count]) ?? []));

  const libraryView = (id: number): View => ({ kind: "library", id });
  const albumView = (id: number): View => ({ kind: "smart_album", id });
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
    const parts = [l.root ?? "bağlı değil", `disk ${l.volumeUuid ?? "?"}`, `kütüphane ${l.uuid}`];
    if (l.stateReason) parts.push(l.stateReason);
    return parts.join("\n");
  }

  const progress = $derived(
    importing && importing.totalFiles > 0 ? `${count(importing.doneFiles)}/${count(importing.totalFiles)}` : null,
  );
</script>

<nav class="flex h-full w-sidebar shrink-0 flex-col border-r border-line bg-surface" aria-label="Kenar çubuğu">
  <div class="flex-1 overflow-y-auto py-2">
    <h2 class="label px-3 pt-1 pb-1">Kütüphaneler</h2>
    <ul>
      {#each catalog.sidebar?.libraries ?? [] as lib (lib.id)}
        {@const view = libraryView(lib.id)}
        <li>
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
                  ? `${bytes(lib.freeBytes)} boş`
                  : "çevrimdışı"}
              </span>
            </span>
            <span class="data text-2xs text-ink-3">{count(lib.mediaCount)}</span>
          </button>
        </li>
      {:else}
        <li class="px-3 py-1 text-xs text-ink-3">Kütüphane yok</li>
      {/each}
    </ul>

    <h2 class="label px-3 pt-4 pb-1">Görünümler</h2>
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

    <h2 class="label px-3 pt-4 pb-1">Akıllı Albümler</h2>
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
        <li class="px-3 py-1 text-xs text-ink-3">Akıllı albüm yok</li>
      {/each}
    </ul>
  </div>

  <div class="flex flex-col gap-2 border-t border-line p-3">
    {#if importing && (importing.state === "running" || importing.state === "scanning")}
      <div class="data text-2xs text-ink-2" aria-live="polite">
        <div class="flex justify-between">
          <span>içe aktarılıyor</span><span>{progress ?? "taranıyor"}</span>
        </div>
        <div class="mt-1 h-px w-full bg-line">
          <div
            class="h-px bg-amber"
            style:width="{importing.totalFiles > 0 ? (100 * importing.doneFiles) / importing.totalFiles : 0}%"
          ></div>
        </div>
        <div class="mt-1 flex justify-between text-ink-3">
          <span>{bytes(importing.doneBytes)}</span><span>{count(importing.skippedDup)} kopya atlandı</span>
        </div>
      </div>
    {/if}
    <div class="grid grid-cols-3 border border-line" role="radiogroup" aria-label="İçe aktarma modu">
      {#each MODES as m, i (m.key)}
        <button
          class="h-6 text-xs {i > 0 ? 'border-l border-line' : ''} {mode === m.key
            ? 'bg-raised text-ink'
            : 'text-ink-3 hover:text-ink-2'}"
          role="radio"
          aria-checked={mode === m.key}
          title={m.hint}
          onclick={() => (mode = m.key)}>{m.name}</button
        >
      {/each}
    </div>
    <div class="flex gap-2">
      <button class="btn flex-1 justify-center" onclick={onAddFolder} title="Kütüphane kökü ekle">
        <FolderPlus size={14} strokeWidth={1.5} />Klasör
      </button>
      <button class="btn btn-primary flex-1 justify-center" onclick={() => onImport(mode)}>İçe Aktar</button>
    </div>
  </div>

  <div class="flex h-8 shrink-0 items-stretch border-t border-line text-xs" aria-label="Hızlı erişim">
    <button
      class="flex flex-1 items-center gap-2 px-3 text-ink-2 hover:bg-raised hover:text-ink"
      onclick={() => (catalog.overlay = "wifi")}
      title="Wi-Fi Telefon Eşleştir"
    >
      <Wifi size={14} strokeWidth={1.5} />
      <span>Telefon</span>
      <span class="data text-2xs {lan && lan.activeUploads > 0 ? 'text-amber' : 'text-ink-3'}">
        {lan ? (lan.activeUploads > 0 ? `↓${lan.activeUploads}` : lan.pairedDevices) : ""}
      </span>
    </button>
    <button
      class="flex items-center border-l border-line px-3 text-ink-2 hover:bg-raised hover:text-ink"
      onclick={() => catalog.openSettings("cloud")}
      title="Ayarlar"
      aria-label="Ayarlar"
    >
      <SlidersHorizontal size={14} strokeWidth={1.5} />
    </button>
    <button
      class="data flex items-center gap-1 border-l border-line px-3 text-2xs whitespace-nowrap text-ink-3 hover:bg-raised hover:text-ink"
      onclick={() => (catalog.overlay = "palette")}
      title="Komut paleti"
      aria-label="Komut paleti"
    >
      <CommandIcon size={12} strokeWidth={1.5} />{mac ? "⌘K" : "Ctrl+K"}
    </button>
  </div>
</nav>

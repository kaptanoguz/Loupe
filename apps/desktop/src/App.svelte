<script lang="ts">
  import { onDestroy, onMount } from "svelte";

  import { type Backend, createBackend } from "./lib/api/backend";
  import type { ImportRequest, ImportSessionDto } from "./lib/api/types";
  import Events from "./lib/components/Events.svelte";
  import Grid from "./lib/components/Grid.svelte";
  import Inspector from "./lib/components/Inspector.svelte";
  import Loupe from "./lib/components/Loupe.svelte";
  import Palette, { type CommandId, type PaletteCommand } from "./lib/components/Palette.svelte";
  import People from "./lib/components/People.svelte";
  import Settings from "./lib/components/Settings.svelte";
  import Sidebar from "./lib/components/Sidebar.svelte";
  import StatusBar from "./lib/components/StatusBar.svelte";
  import TrashDialog from "./lib/components/TrashDialog.svelte";
  import ViewHeader from "./lib/components/ViewHeader.svelte";
  import ImportDialog, { type ImportSource } from "./lib/components/ImportDialog.svelte";
  import AlbumPicker from "./lib/components/AlbumPicker.svelte";
  import ContextMenu from "./lib/components/ContextMenu.svelte";
  import Slideshow from "./lib/components/Slideshow.svelte";
  import WifiModal from "./lib/components/WifiModal.svelte";
  import { LayoutEngine } from "./lib/layout/engine";
  import { Catalog, isPageView } from "./lib/state/catalog.svelte";
  import { isTypingTarget, keyToAction } from "./lib/state/keymap";

  interface Props {
    backend?: Backend;
    engine?: LayoutEngine;
  }

  let { backend = createBackend(), engine = new LayoutEngine() }: Props = $props();

  // Uygulama ömrü boyunca tek katalog ve tek düzen motoru.
  // svelte-ignore state_referenced_locally
  const catalog = new Catalog(backend);
  // svelte-ignore state_referenced_locally
  const layoutEngine = engine;
  let importing = $state<ImportSessionDto | null>(null);
  let statusTimer: ReturnType<typeof setInterval> | undefined;
  let importTimer: ReturnType<typeof setTimeout> | undefined;

  const STATUS_POLL_MS = 2000;
  const IMPORT_POLL_MS = 400;
  /** İçe aktarma sürerken yeni karolar bu aralıkla görünür. */
  const IMPORT_REFRESH_MS = 3000;

  onMount(() => {
    void catalog.refreshSidebar();
    void catalog.open({ kind: "all" });
    void catalog.refreshStatus();
    statusTimer = setInterval(() => void catalog.refreshStatus(), STATUS_POLL_MS);
  });

  onDestroy(() => {
    clearInterval(statusTimer);
    clearTimeout(importTimer);
    layoutEngine.dispose();
  });

  function fail(e: unknown): void {
    catalog.error = e instanceof Error ? e.message : String(e);
  }

  function track(session: ImportSessionDto): void {
    importing = session;
    let lastRefresh = performance.now();
    const poll = async () => {
      try {
        const s = await backend.importStatus(session.id);
        importing = s;
        const running = s.state === "running" || s.state === "scanning";
        if (!running || performance.now() - lastRefresh > IMPORT_REFRESH_MS) {
          lastRefresh = performance.now();
          void catalog.open(catalog.view);
          void catalog.refreshSidebar();
        }
        if (running) importTimer = setTimeout(poll, IMPORT_POLL_MS);
        else if (s.lastError) catalog.error = s.lastError;
      } catch (e) {
        fail(e);
      }
    };
    importTimer = setTimeout(poll, IMPORT_POLL_MS);
  }

  async function addFolder(): Promise<void> {
    try {
      const added = await backend.pickOrAddLibraryRoot();
      if (!added) return;
      await catalog.refreshSidebar();
      if (added.scan) track(added.scan);
    } catch (e) {
      fail(e);
    }
  }

  function importTarget(): number | null {
    const libs = catalog.sidebar?.libraries ?? [];
    const target = libs.find((l) => l.primary && l.managed && l.state === "online") ?? libs.find((l) => l.managed && l.state === "online");
    if (!target) catalog.error = "Önce yönetilen bir kütüphane klasörü ekleyin (bağlı diskte).";
    return target?.id ?? null;
  }

  /** İçe aktarma penceresinin kaynağı; pencere hedefi, düzeni ve modu sorar. */
  let importSource = $state<ImportSource | null>(null);

  async function startImport(): Promise<void> {
    if (importTarget() === null) return;
    try {
      const path = await backend.pickImportSource();
      if (!path) return;
      importSource = { kind: "folder", path };
      catalog.overlay = "import";
    } catch (e) {
      fail(e);
    }
  }

  async function runImport(req: ImportRequest): Promise<void> {
    try {
      const s = await backend.startImport(req);
      track(s);
      const lib = catalog.sidebar?.libraries.find((l) => l.id === req.libraryId);
      catalog.flash(`İçe aktarma başladı → ${lib?.name ?? "kütüphane"}${req.layout ? ` / ${req.layout}` : ""}`);
    } catch (e) {
      fail(e);
    }
  }

  /** Onaylanan fotoğrafları Çöp Kutusu'na taşır. */
  async function trashMedia(ids: number[]): Promise<void> {
    try {
      const r = await backend.trashMedia(ids);
      catalog.clearMarks();
      void catalog.open(catalog.view);
      void catalog.refreshSidebar();
      const local = r.loupeTrash > 0 ? ` (${r.loupeTrash} tanesi bu diskte Çöp Kutusu olmadığından ${r.loupeTrashDir} klasörüne)` : "";
      if (r.failed.length > 0) catalog.error = `${r.trashed} fotoğraf taşındı · ${r.failed.length} taşınamadı: ${r.failed[0]?.reason}`;
      else catalog.flash(`${r.trashed} fotoğraf Çöp Kutusu'na taşındı${local}`);
    } catch (e) {
      fail(e);
    }
  }

  /** Klasörü Loupe'den kaldırır; dosyalar diskte kalır. */
  async function removeLibrary(id: number): Promise<void> {
    try {
      const removed = await backend.removeLibrary(id);
      if (catalog.view.kind === "library" && catalog.view.id === id) void catalog.open({ kind: "all" });
      else void catalog.open(catalog.view);
      await catalog.refreshSidebar();
      catalog.flash(`Klasör kaldırıldı · ${removed} kayıt (dosyalar diskte duruyor)`);
    } catch (e) {
      fail(e);
    }
  }

  let searchingIphone = $state(false);

  /** Kabloyla bağlı iPhone'un DCIM klasörünü kopyalar; aktarılmış olanlar özetle atlanır. */
  async function importIphone(): Promise<void> {
    if (importTarget() === null || searchingIphone) return;
    searchingIphone = true;
    try {
      const [phone] = await backend.findIphone();
      if (!phone) {
        catalog.error = "iPhone bulunamadı: kabloyla bağlayın, kilidini açın ve \"Bu bilgisayara güven\" deyin.";
        return;
      }
      importSource = { kind: "iphone", path: phone.dcim };
      catalog.overlay = "import";
    } catch (e) {
      fail(e);
    } finally {
      searchingIphone = false;
    }
  }

  const commands = $derived.by((): PaletteCommand[] => [
    { id: "verify", label: "Kütüphaneyi Doğrula (Bit-Rot)", hint: "BLAKE3 ile tüm dosyaları denetle" },
    { id: "wifi", label: "Wi-Fi Telefon Eşleştir", hint: "QR kodu göster, cihazları yönet" },
    { id: "iphone", label: "iPhone'dan İçe Aktar", hint: "kabloyla bağlı iPhone'un fotoğraflarını kopyala" },
    { id: "cloud", label: "Bulut Senkronizasyonunu Başlat", hint: "şifreli yedeklemeyi şimdi çalıştır" },
    {
      id: "ai-pause",
      label: catalog.status?.ai.paused ? "Yapay Zekâyı Sürdür" : "Yapay Zekâyı Duraklat",
      hint: "yüz ve CLIP analizi",
    },
    { id: "slideshow", label: "Slayt Gösterisi", hint: "seçili fotoğraftan ya da işaretlilerle, tam ekran (S)" },
    { id: "people", label: "Kişiler", hint: "yüzleri adlandır, birleştir" },
    { id: "events", label: "Etkinlikler", hint: "ad ver, kilitle" },
    { id: "settings", label: "Ayarlar", hint: "kütüphaneler, telefon, bulut, doğrulama, yapay zekâ" },
  ]);

  async function runCommand(id: CommandId): Promise<void> {
    try {
      switch (id) {
        case "verify": {
          const started = await backend.verifyLibrary();
          catalog.flash(
            started.length ? `Doğrulama başladı · ${started.length} kütüphane` : "Doğrulanacak çevrimiçi kütüphane yok",
          );
          catalog.openSettings("verify");
          break;
        }
        case "wifi":
          catalog.overlay = "wifi";
          break;
        case "iphone":
          await importIphone();
          break;
        case "cloud": {
          const st = await backend.startCloudSync();
          const setup = st.filter((t) => t.state === "needs_setup");
          if (st.length === 0) catalog.openSettings("cloud");
          else if (setup.length > 0) catalog.openSettings("encryption");
          catalog.flash(
            st.length === 0
              ? "Bulut hedefi yok: önce bir hedef ekleyin"
              : setup.length > 0
                ? "Şifreli hedef için önce kurtarma anahtarı oluşturun"
                : `Bulut senkronizasyonu başladı · ${st.reduce((n, t) => n + t.pending, 0)} dosya sırada`,
          );
          break;
        }
        case "ai-pause": {
          const ai = await backend.setPaused(!(catalog.status?.ai.paused ?? false));
          catalog.flash(ai.paused ? "Arka plan analizi duraklatıldı" : "Arka plan analizi sürüyor");
          break;
        }
        case "slideshow":
          catalog.overlay = null;
          catalog.startSlideshow();
          break;
        case "people":
        case "events":
          void catalog.open({ kind: id });
          break;
        case "settings":
          catalog.openSettings("libraries");
          break;
      }
      void catalog.refreshStatus();
    } catch (e) {
      fail(e);
    }
  }

  function onkeydown(e: KeyboardEvent): void {
    if (e.defaultPrevented) return;
    if (catalog.slideshow) {
      if (catalog.slideKey(e.key)) e.preventDefault();
      return;
    }
    // Ctrl/⌘+K metin alanındayken de çalışır (palet açıkken onu kapatır).
    if ((e.ctrlKey || e.metaKey) && (e.key === "k" || e.key === "K")) {
      e.preventDefault();
      catalog.dispatch({ type: "palette" });
      return;
    }
    if (catalog.menu) {
      // Menü kendi okları ve Enter'ı işler; Esc kapatır.
      if (e.key === "Escape") {
        e.preventDefault();
        catalog.menu = null;
      }
      return;
    }
    if (catalog.overlay) {
      // Modallar kendi tuşlarını işler; Esc hepsini kapatır.
      if (e.key === "Escape" && catalog.overlay !== "palette") {
        e.preventDefault();
        catalog.overlay = null;
      }
      return;
    }
    if (isTypingTarget(e.target)) return;
    const action = keyToAction(e, { loupeOpen: catalog.loupeOpen });
    if (action && catalog.dispatch(action)) e.preventDefault();
  }

  const page = $derived(isPageView(catalog.view));
</script>

<svelte:window {onkeydown} />

<div class="flex h-full flex-col bg-canvas text-ink">
  <div class="relative flex min-h-0 flex-1">
    <div class="relative flex min-w-0 flex-1">
      <Sidebar
        {catalog}
        onAddFolder={addFolder}
        onImport={startImport}
        onImportIphone={importIphone}
        onRemoveLibrary={removeLibrary}
        {searchingIphone}
        {importing}
      />
      {#if catalog.view.kind === "people"}
        <People {catalog} />
      {:else if catalog.view.kind === "events"}
        <Events {catalog} />
      {:else}
        <div class="flex min-w-0 flex-1 flex-col">
          <ViewHeader {catalog} />
          <div class="flex min-h-0 flex-1">
            <Grid {catalog} engine={layoutEngine} onAddFolder={addFolder} />
          </div>
        </div>
      {/if}
      {#if catalog.loupeOpen}
        <Loupe {catalog} />
      {/if}
    </div>
    {#if catalog.inspectorOpen && !page}
      <Inspector
        detail={catalog.detail}
        onFavorite={(id) => catalog.toggleFavorite([id])}
        onReveal={(id) => void catalog.reveal(id)}
      />
    {/if}
  </div>
  <StatusBar {catalog} />
</div>

{#if catalog.slideshow}
  <Slideshow {catalog} />
{/if}
{#if catalog.menu}
  <ContextMenu {catalog} x={catalog.menu.x} y={catalog.menu.y} />
{/if}

{#if catalog.overlay === "palette"}
  <Palette {catalog} {commands} onCommand={runCommand} />
{:else if catalog.overlay === "wifi"}
  <WifiModal {catalog} />
{:else if catalog.overlay === "settings"}
  <Settings {catalog} />
{:else if catalog.overlay === "import" && importSource}
  <ImportDialog {catalog} source={importSource} onPickSource={() => backend.pickImportSource()} onStart={runImport} />
{:else if catalog.overlay === "album"}
  <AlbumPicker {catalog} ids={catalog.targetIds()} />
{:else if catalog.overlay === "trash"}
  <TrashDialog {catalog} ids={catalog.targetIds()} onConfirm={trashMedia} />
{/if}

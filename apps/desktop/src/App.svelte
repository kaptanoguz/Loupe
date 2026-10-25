<script lang="ts">
  import { onDestroy, onMount } from "svelte";

  import { type Backend, createBackend } from "./lib/api/backend";
  import type { ImportMode, ImportSessionDto } from "./lib/api/types";
  import Grid from "./lib/components/Grid.svelte";
  import Inspector from "./lib/components/Inspector.svelte";
  import Loupe from "./lib/components/Loupe.svelte";
  import Sidebar from "./lib/components/Sidebar.svelte";
  import StatusBar from "./lib/components/StatusBar.svelte";
  import { LayoutEngine } from "./lib/layout/engine";
  import { Catalog } from "./lib/state/catalog.svelte";
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

  async function startImport(mode: ImportMode): Promise<void> {
    const libs = catalog.sidebar?.libraries ?? [];
    const target = libs.find((l) => l.primary && l.managed && l.state === "online") ?? libs.find((l) => l.managed && l.state === "online");
    if (!target) {
      catalog.error = "Önce yönetilen bir kütüphane klasörü ekleyin (bağlı diskte).";
      return;
    }
    try {
      const source = await backend.pickImportSource();
      if (!source) return;
      track(await backend.startImport({ libraryId: target.id, source, mode }));
    } catch (e) {
      fail(e);
    }
  }

  function onkeydown(e: KeyboardEvent): void {
    if (e.defaultPrevented || isTypingTarget(e.target)) return;
    const action = keyToAction(e, { loupeOpen: catalog.loupeOpen });
    if (action && catalog.dispatch(action)) e.preventDefault();
  }
</script>

<svelte:window {onkeydown} />

<div class="flex h-full flex-col bg-canvas text-ink">
  <div class="relative flex min-h-0 flex-1">
    <div class="relative flex min-w-0 flex-1">
      <Sidebar {catalog} onAddFolder={addFolder} onImport={startImport} {importing} />
      <Grid {catalog} engine={layoutEngine} onAddFolder={addFolder} />
      {#if catalog.loupeOpen}
        <Loupe {catalog} />
      {/if}
    </div>
    {#if catalog.inspectorOpen}
      <Inspector detail={catalog.detail} />
    {/if}
  </div>
  <StatusBar {catalog} />
</div>

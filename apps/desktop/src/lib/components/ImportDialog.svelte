<script lang="ts" module>
  /** İçe aktarma kaynağı: seçilen klasör ya da kabloyla bağlı iPhone. */
  export interface ImportSource {
    kind: "folder" | "iphone";
    path: string;
  }
</script>

<script lang="ts">
  import FolderOpen from "@lucide/svelte/icons/folder-open";
  import Import from "@lucide/svelte/icons/import";
  import { onMount } from "svelte";

  import type { ImportMode, ImportRequest, LibraryDto } from "../api/types";
  import { DATE_PATTERNS, joinLayout, layoutError, previewPath, splitLayout } from "../importLayout";
  import type { Catalog } from "../state/catalog.svelte";

  interface Props {
    catalog: Catalog;
    source: ImportSource;
    /** Klasör kaynağını değiştir (yerel seçici). */
    onPickSource: () => Promise<string | null>;
    onStart: (req: ImportRequest) => Promise<void>;
  }

  let { catalog, source, onPickSource, onStart }: Props = $props();

  const SUB_KEY = "loupe.import.sub";
  const MODES: { key: ImportMode; name: string; hint: string }[] = [
    { key: "copy", name: "Kopyala", hint: "Kaynak olduğu gibi kalır." },
    { key: "move", name: "Taşı", hint: "Her dosya kopyası doğrulandıktan sonra kaynaktan silinir." },
    { key: "hardlink", name: "Bağla", hint: "Aynı diskte ek yer kaplamaz; kaynak dosyayla aynı veridir." },
  ];

  const targets = $derived<LibraryDto[]>(
    (catalog.sidebar?.libraries ?? []).filter((l) => l.managed && l.state === "online" && l.root),
  );
  let path = $state("");
  let libraryId = $state<number | null>(null);
  let sub = $state("");
  let date = $state("{yyyy}/{mm}/{dd}");
  let mode = $state<ImportMode>("copy");
  let busy = $state(false);

  const iphone = $derived(source.kind === "iphone");
  const target = $derived(targets.find((l) => l.id === libraryId));
  const layout = $derived(joinLayout(sub, date));
  const problem = $derived(layoutError(layout));
  const sample = $derived(iphone ? "IMG_1234.HEIC" : "DSC_0412.JPG");

  function loadSub(): string {
    try {
      const all = JSON.parse(localStorage.getItem(SUB_KEY) ?? "{}") as Record<string, string>;
      return all[source.kind] ?? (iphone ? "iPhone" : "");
    } catch {
      return iphone ? "iPhone" : "";
    }
  }

  function saveSub(): void {
    try {
      const all = JSON.parse(localStorage.getItem(SUB_KEY) ?? "{}") as Record<string, string>;
      all[source.kind] = sub.trim();
      localStorage.setItem(SUB_KEY, JSON.stringify(all));
    } catch {
      // Bir sonraki pencerede varsayılan gelir.
    }
  }

  function pickLibrary(id: number): void {
    libraryId = id;
    const lib = targets.find((l) => l.id === id);
    if (!lib) return;
    date = splitLayout(lib.layoutTemplate).date || "{yyyy}/{mm}/{dd}";
    // Telefondan taşıma yok: iPhone'dan hiçbir şey silinmez.
    mode = iphone ? "copy" : lib.defaultImportMode;
  }

  onMount(() => {
    path = source.path;
    sub = loadSub();
    const first = targets.find((l) => l.primary) ?? targets[0];
    if (first) pickLibrary(first.id);
  });

  async function changeSource(): Promise<void> {
    const p = await onPickSource();
    if (p) path = p;
  }

  function close(): void {
    if (!busy) catalog.overlay = null;
  }

  async function start(): Promise<void> {
    if (libraryId === null || problem || !path) return;
    busy = true;
    saveSub();
    try {
      await onStart({ libraryId, source: path, mode, layout });
    } finally {
      busy = false;
      catalog.overlay = null;
    }
  }

  function onscrim(e: MouseEvent): void {
    if (e.target === e.currentTarget) close();
  }

  function onkeydown(e: KeyboardEvent): void {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    }
  }
</script>

<div class="scrim fixed inset-0 z-30 flex items-center justify-center" role="presentation" onmousedown={onscrim}>
  <div
    class="flex w-[560px] max-w-[calc(100vw-32px)] flex-col gap-4 rounded-sm border border-line bg-surface p-4"
    role="dialog"
    aria-label="İçe aktar"
    aria-modal="true"
    tabindex="-1"
    {onkeydown}
  >
    <h1 class="flex items-center gap-2 text-sm font-medium text-ink">
      <Import size={14} strokeWidth={1.5} />{iphone ? "iPhone'dan içe aktar" : "Klasörden içe aktar"}
    </h1>

    <section class="flex flex-col gap-1">
      <span class="label">Kaynak</span>
      <div class="flex items-center gap-2">
        <span class="data min-w-0 flex-1 truncate text-xs text-ink-2" title={path}>{path || "seçilmedi"}</span>
        {#if !iphone}
          <button class="btn h-6" onclick={changeSource}><FolderOpen size={12} strokeWidth={1.5} />Değiştir</button>
        {/if}
      </div>
      {#if iphone}
        <p class="text-2xs text-ink-3">
          Kabloyla iPhone yalnızca Kamera klasörünü (DCIM) verir; albümler gelmez. Albüm göndermek için telefondaki
          Loupe sayfasındaki Kestirme bölümüne bakın.
        </p>
      {/if}
    </section>

    <section class="flex flex-col gap-1">
      <label class="label" for="import-target">Hedef kütüphane</label>
      {#if targets.length === 0}
        <p class="text-sm text-amber">Bağlı, yönetilen bir kütüphane yok. Önce kenar çubuğundan klasör ekleyin.</p>
      {:else}
        <select
          id="import-target"
          class="input"
          value={libraryId}
          onchange={(e) => pickLibrary(Number((e.currentTarget as HTMLSelectElement).value))}
        >
          {#each targets as l (l.id)}
            <option value={l.id}>{l.name} · {l.root}{l.primary ? " (birincil)" : ""}</option>
          {/each}
        </select>
      {/if}
    </section>

    <section class="flex flex-col gap-1">
      <label class="label" for="import-sub">Alt klasör (isteğe bağlı)</label>
      <input
        id="import-sub"
        class="input"
        bind:value={sub}
        placeholder={iphone ? "iPhone" : "boş bırakılırsa doğrudan kütüphane klasörü"}
      />
    </section>

    <section class="flex flex-col gap-1">
      <span class="label">Klasör düzeni (çekim tarihine göre)</span>
      <div class="flex gap-1">
        {#each DATE_PATTERNS as p (p.value)}
          <button
            class="btn h-6 flex-1 px-1 {date === p.value ? 'border-amber text-ink' : 'text-ink-2'}"
            aria-pressed={date === p.value}
            onclick={() => (date = p.value)}>{p.label}</button
          >
        {/each}
      </div>
    </section>

    <section class="flex flex-col gap-1">
      <span class="label">Mod</span>
      {#if iphone}
        <p class="text-xs text-ink-2">Kopyala. Telefondan hiçbir şey silinmez.</p>
      {:else}
        <div class="flex gap-1">
          {#each MODES as m (m.key)}
            <button
              class="btn h-6 flex-1 {mode === m.key ? 'border-amber text-ink' : 'text-ink-2'}"
              aria-pressed={mode === m.key}
              title={m.hint}
              onclick={() => (mode = m.key)}>{m.name}</button
            >
          {/each}
        </div>
        <p class="text-2xs text-ink-3">{MODES.find((m) => m.key === mode)?.hint}</p>
      {/if}
    </section>

    <section class="flex flex-col gap-1 border-t border-line pt-3">
      <span class="label">Örnek hedef</span>
      {#if problem}
        <p class="text-xs text-amber">{problem}</p>
      {:else if target?.root}
        <p class="data text-xs break-all text-ink" aria-label="Örnek hedef yol">{previewPath(target.root, layout, sample)}</p>
      {/if}
      <p class="text-2xs text-ink-3">Daha önce aktarılmış fotoğraflar içerikten tanınır ve atlanır.</p>
    </section>

    <div class="flex justify-end gap-2">
      <button class="btn" disabled={busy} onclick={close}>Vazgeç</button>
      <button class="btn btn-primary" disabled={busy || !target || !!problem || !path} onclick={start}>
        {busy ? "Başlatılıyor…" : "İçe aktar"}
      </button>
    </div>
  </div>
</div>

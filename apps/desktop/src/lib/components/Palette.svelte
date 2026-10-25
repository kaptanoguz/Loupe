<script lang="ts" module>
  /** Paletten çalıştırılan uygulama komutları (App yürütür). */
  export type CommandId = "verify" | "wifi" | "cloud" | "settings" | "people" | "events" | "ai-pause";

  export interface PaletteCommand {
    id: CommandId;
    label: string;
    hint: string;
  }

  /** Yazılan girdiyle komut eşleşmesi: her sözcük etikette ya da ipucunda geçmeli. */
  export function matchCommand(cmd: PaletteCommand, input: string): boolean {
    const words = input.toLocaleLowerCase("tr").split(/\s+/).filter(Boolean);
    const hay = `${cmd.label} ${cmd.hint}`.toLocaleLowerCase("tr");
    return words.every((w) => hay.includes(w));
  }

  /** Arama yazılırken girdinin bu süre durulması beklenir (ms). */
  export const SEARCH_DEBOUNCE_MS = 120;
  /** Önizlemede gösterilen sonuç sayısı. */
  export const PREVIEW = 12;
</script>

<script lang="ts">
  import CommandIcon from "@lucide/svelte/icons/command";
  import CornerDownLeft from "@lucide/svelte/icons/corner-down-left";
  import Save from "@lucide/svelte/icons/save";
  import { onDestroy, onMount, tick } from "svelte";

  import type { ChipKind, PaletteResult, Suggestion } from "../api/types";
  import { count } from "../format";
  import type { Catalog } from "../state/catalog.svelte";

  interface Props {
    catalog: Catalog;
    commands: PaletteCommand[];
    onCommand: (id: CommandId) => void;
  }

  let { catalog, commands, onCommand }: Props = $props();

  const KIND_NAME: Record<ChipKind, string> = {
    person: "kişi",
    place: "yer",
    event: "etkinlik",
    rating: "puan",
    color: "renk",
    camera: "kamera",
    lens: "lens",
    year: "yıl",
    favorite: "favori",
  };

  type Entry =
    | { type: "suggestion"; s: Suggestion }
    | { type: "open" }
    | { type: "save" }
    | { type: "command"; cmd: PaletteCommand };

  let input = $state("");
  let result = $state.raw<PaletteResult | null>(null);
  let active = $state(0);
  let saving = $state(false);
  let albumName = $state("");
  let busy = $state(false);
  let field = $state<HTMLInputElement>();
  let nameField = $state<HTMLInputElement>();
  let timer: ReturnType<typeof setTimeout> | undefined;
  let token = 0;

  const searchable = $derived(Boolean(result?.searched && result.chips.every((c) => c.ok)));
  const entries = $derived.by((): Entry[] => {
    const out: Entry[] = (result?.suggestions ?? []).map((s) => ({ type: "suggestion", s }));
    if (searchable && result && result.total > 0) out.push({ type: "open" }, { type: "save" });
    for (const cmd of commands) if (matchCommand(cmd, input)) out.push({ type: "command", cmd });
    return out;
  });

  onMount(() => {
    field?.focus();
    void run("");
  });

  onDestroy(() => clearTimeout(timer));

  async function run(text: string): Promise<void> {
    const mine = ++token;
    busy = true;
    try {
      const r = await catalog.backend.searchPalette({ input: text, preview: PREVIEW });
      if (mine !== token) return;
      result = r;
      active = 0;
    } catch (e) {
      if (mine === token) catalog.fail(e);
    } finally {
      if (mine === token) busy = false;
    }
  }

  function oninput(): void {
    clearTimeout(timer);
    timer = setTimeout(() => void run(input), SEARCH_DEBOUNCE_MS);
  }

  function close(): void {
    catalog.overlay = null;
  }

  /** Son belirteci önerinin metniyle değiştirir. */
  function accept(s: Suggestion): void {
    const m = /(?:[^\s"]+:"[^"]*"?|\S+)$/.exec(input);
    const head = m && !input.endsWith(" ") ? input.slice(0, m.index) : input;
    input = `${head}${s.insert} `;
    clearTimeout(timer);
    void run(input);
    field?.focus();
  }

  function openResults(): void {
    if (!result) return;
    const title = result.text ? `“${result.text}”` : "Arama";
    const subtitle = result.chips.map((c) => c.label).join(" · ") || null;
    void catalog.open({ kind: "search", query: result.query }, catalog.grouping, { title, subtitle, input: input.trim() });
    close();
  }

  async function startSave(): Promise<void> {
    saving = true;
    albumName = result?.text ?? result?.chips.map((c) => c.label).join(", ") ?? "";
    await tick();
    nameField?.select();
  }

  async function save(): Promise<void> {
    if (!result || !albumName.trim()) return;
    try {
      const album = await catalog.backend.saveSmartAlbum(albumName.trim(), result.query, true);
      await catalog.refreshSidebar();
      void catalog.open({ kind: "smart_album", id: album.id }, catalog.grouping, { title: album.name });
      catalog.flash(`Akıllı albüm kaydedildi: ${album.name}`);
      close();
    } catch (e) {
      catalog.fail(e);
    }
  }

  function execute(e: Entry | undefined): void {
    if (!e) {
      if (searchable && result && result.total > 0) openResults();
      return;
    }
    switch (e.type) {
      case "suggestion":
        accept(e.s);
        return;
      case "open":
        openResults();
        return;
      case "save":
        void startSave();
        return;
      case "command":
        close();
        onCommand(e.cmd.id);
        return;
    }
  }

  function onkeydown(e: KeyboardEvent): void {
    if (saving) {
      if (e.key === "Escape") {
        e.preventDefault();
        saving = false;
        field?.focus();
      } else if (e.key === "Enter") {
        e.preventDefault();
        void save();
      }
      return;
    }
    switch (e.key) {
      case "Escape":
        e.preventDefault();
        close();
        return;
      case "ArrowDown":
        e.preventDefault();
        active = entries.length ? (active + 1) % entries.length : 0;
        return;
      case "ArrowUp":
        e.preventDefault();
        active = entries.length ? (active - 1 + entries.length) % entries.length : 0;
        return;
      case "Tab": {
        const first = entries.find((x) => x.type === "suggestion");
        if (first && first.type === "suggestion") {
          e.preventDefault();
          accept(first.s);
        }
        return;
      }
      case "Enter":
        e.preventDefault();
        if (e.ctrlKey || e.metaKey) {
          if (searchable) void startSave();
          return;
        }
        execute(entries[active]);
        return;
      case "k":
      case "K":
        if (e.ctrlKey || e.metaKey) {
          e.preventDefault();
          close();
        }
        return;
    }
  }

  function onscrim(e: MouseEvent): void {
    if (e.target === e.currentTarget) close();
  }
</script>

<div class="scrim fixed inset-0 z-30 flex items-start justify-center" role="presentation" onmousedown={onscrim}>
  <div class="palette flex max-h-[70vh] flex-col overflow-hidden" role="dialog" aria-label="Komut paleti" aria-modal="true">
    {#if saving}
      <div class="flex h-11 shrink-0 items-center gap-2 border-b border-line px-3">
        <Save size={14} strokeWidth={1.5} class="text-ink-3" />
        <input
          bind:this={nameField}
          bind:value={albumName}
          {onkeydown}
          class="min-w-0 flex-1 bg-transparent text-base text-ink outline-none placeholder:text-ink-3"
          placeholder="Akıllı albüm adı"
          aria-label="Akıllı albüm adı"
        />
        <button class="btn btn-primary" onclick={save} disabled={!albumName.trim()}>Kaydet</button>
      </div>
      <p class="px-3 py-2 text-xs text-ink-3">
        Sorgu saklanır, sonuçlar değil: yeni fotoğraflar eşleştikçe albüme kendiliğinden girer.
      </p>
    {:else}
      <div class="flex h-11 shrink-0 items-center gap-2 border-b border-line px-3">
        <CommandIcon size={14} strokeWidth={1.5} class="text-ink-3" />
        <input
          bind:this={field}
          bind:value={input}
          {oninput}
          {onkeydown}
          class="min-w-0 flex-1 bg-transparent text-base text-ink outline-none placeholder:text-ink-3"
          placeholder="Ara ya da komut yaz: kumsalda gün batımı person:ayşe year:2024 rating:4"
          aria-label="Arama ve komutlar"
          role="combobox"
          aria-expanded={entries.length > 0}
          aria-controls="palette-list"
          spellcheck="false"
          autocomplete="off"
        />
        {#if busy}<span class="data text-2xs text-ink-3">…</span>{/if}
      </div>

      {#if result && result.chips.length > 0}
        <div class="flex flex-wrap gap-1 border-b border-line px-3 py-2" aria-label="Süzgeçler">
          {#each result.chips as chip, i (i)}
            <span
              class="data inline-flex h-5 items-center gap-1 rounded-xs border px-1.5 text-2xs {chip.ok
                ? 'border-line-strong text-ink'
                : 'border-amber text-amber'}"
              title={chip.error ?? chip.raw}
            >
              <span class="text-ink-3">{KIND_NAME[chip.kind]}</span>{chip.label}{#if chip.error}<span>· {chip.error}</span>{/if}
            </span>
          {/each}
        </div>
      {/if}

      {#if searchable && result}
        <div class="border-b border-line px-3 py-2">
          <div class="data mb-2 flex justify-between text-2xs text-ink-3">
            <span>{count(result.total)} sonuç</span>
            <span>{result.text ? (result.semantic ? "anlamsal + sözcük" : "yalnızca sözcük") : "süzgeç"} · {result.elapsedMs} ms</span>
          </div>
          {#if result.items.length > 0}
            <div class="flex h-14 gap-px overflow-hidden">
              {#each result.items as it (it.id)}
                <img
                  src={catalog.backend.thumbUrl("sm", it.hash)}
                  alt=""
                  class="h-14 shrink-0 bg-surface object-cover"
                  style:width="{Math.round(56 * Math.min(Math.max(it.aspect, 0.5), 2))}px"
                  loading="lazy"
                />
              {/each}
            </div>
          {:else}
            <p class="text-xs text-ink-3">Eşleşen fotoğraf yok.</p>
          {/if}
        </div>
      {/if}

      <ul id="palette-list" class="min-h-0 flex-1 overflow-y-auto py-1" role="listbox" aria-label="Seçenekler">
        {#each entries as e, i (i)}
          <li role="option" aria-selected={i === active}>
            <button
              class="flex h-8 w-full items-center gap-2 px-3 text-left text-sm {i === active ? 'bg-line text-ink' : 'text-ink-2'}"
              onmousemove={() => (active = i)}
              onclick={() => execute(e)}
            >
              {#if e.type === "suggestion"}
                <span class="label w-16 shrink-0">{KIND_NAME[e.s.kind]}</span>
                <span class="flex-1 truncate text-ink">{e.s.label}</span>
                {#if e.s.detail}<span class="data text-2xs text-ink-3">{e.s.detail}</span>{/if}
              {:else if e.type === "open"}
                <span class="label w-16 shrink-0">sonuç</span>
                <span class="flex-1">Tüm sonuçları ızgarada aç</span>
                <span class="data text-2xs text-ink-3">{count(result?.total ?? 0)}</span>
              {:else if e.type === "save"}
                <span class="label w-16 shrink-0">albüm</span>
                <span class="flex-1">Akıllı albüm olarak kaydet</span>
                <span class="data text-2xs text-ink-3">Ctrl+↵</span>
              {:else}
                <span class="label w-16 shrink-0">komut</span>
                <span class="flex-1 truncate">{e.cmd.label}</span>
                <span class="truncate text-xs text-ink-3">{e.cmd.hint}</span>
              {/if}
              {#if i === active}<CornerDownLeft size={12} strokeWidth={1.5} class="shrink-0 text-ink-3" />{/if}
            </button>
          </li>
        {:else}
          <li class="px-3 py-2 text-xs text-ink-3">Eşleşen komut yok.</li>
        {/each}
      </ul>

      <div class="data flex h-6 shrink-0 items-center gap-3 border-t border-line px-3 text-2xs text-ink-3">
        <span>↑↓ seç</span><span>↵ çalıştır</span><span>Tab öneriyi al</span><span>Esc kapat</span>
        {#if result?.semantic === false && result.text}
          <span class="ml-auto text-amber">CLIP modeli yok: yalnızca sözcük araması</span>
        {/if}
      </div>
    {/if}
  </div>
</div>

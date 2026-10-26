<script lang="ts">
  import BookmarkPlus from "@lucide/svelte/icons/bookmark-plus";
  import Plus from "@lucide/svelte/icons/plus";
  import SquareLibrary from "@lucide/svelte/icons/square-library";
  import { onMount } from "svelte";

  import { count } from "../format";
  import type { Catalog } from "../state/catalog.svelte";

  interface Props {
    catalog: Catalog;
    ids: number[];
  }

  let { catalog, ids }: Props = $props();
  let query = $state("");
  let busy = $state(false);
  let field = $state<HTMLInputElement>();

  const albums = $derived(catalog.sidebar?.albums ?? []);
  const shown = $derived.by(() => {
    const q = query.trim().toLocaleLowerCase("tr");
    return q ? albums.filter((a) => a.name.toLocaleLowerCase("tr").includes(q)) : albums;
  });
  /** Yazılan ad hiçbir albümle birebir eşleşmiyorsa yeni albüm önerilir. */
  const fresh = $derived.by(() => {
    const q = query.trim();
    return q && !albums.some((a) => a.name.toLocaleLowerCase("tr") === q.toLocaleLowerCase("tr")) ? q : null;
  });

  onMount(() => field?.focus());

  function close(): void {
    if (!busy) catalog.overlay = null;
  }

  async function pick(id: number): Promise<void> {
    busy = true;
    try {
      await catalog.addToAlbum(id, ids);
    } finally {
      busy = false;
      catalog.overlay = null;
    }
  }

  async function create(name: string): Promise<void> {
    busy = true;
    try {
      const a = await catalog.backend.createAlbum(name);
      await catalog.addToAlbum(a.id, ids);
      catalog.overlay = null;
    } catch (e) {
      catalog.fail(e);
    } finally {
      busy = false;
    }
  }

  function onkeydown(e: KeyboardEvent): void {
    if (e.key !== "Enter") return;
    e.preventDefault();
    const exact = albums.find((a) => a.name.toLocaleLowerCase("tr") === query.trim().toLocaleLowerCase("tr"));
    if (exact) void pick(exact.id);
    else if (fresh) void create(fresh);
    else if (shown.length === 1 && shown[0]) void pick(shown[0].id);
  }

  function onscrim(e: MouseEvent): void {
    if (e.target === e.currentTarget) close();
  }
</script>

<div class="scrim fixed inset-0 z-30 flex items-center justify-center" role="presentation" onmousedown={onscrim}>
  <div
    class="flex w-[420px] max-w-[calc(100vw-32px)] flex-col gap-3 rounded-sm border border-line bg-surface p-4"
    role="dialog"
    aria-label="Albüme ekle"
    aria-modal="true"
  >
    <h1 class="flex items-center gap-2 text-sm font-medium text-ink">
      <BookmarkPlus size={14} strokeWidth={1.5} />{count(ids.length)} öğeyi albüme ekle
    </h1>
    <input
      bind:this={field}
      bind:value={query}
      {onkeydown}
      class="input"
      placeholder="Albüm ara ya da yeni ad yaz"
      aria-label="Albüm adı"
      disabled={busy}
    />
    <ul class="flex max-h-72 flex-col overflow-y-auto" aria-label="Albümler">
      {#if fresh}
        <li>
          <button
            class="flex h-7 w-full items-center gap-2 px-2 text-left text-sm text-ink hover:bg-raised"
            disabled={busy}
            onclick={() => create(fresh)}
          >
            <Plus size={14} strokeWidth={1.5} /><span class="flex-1 truncate">Yeni albüm: {fresh}</span>
          </button>
        </li>
      {/if}
      {#each shown as a (a.id)}
        <li>
          <button
            class="flex h-7 w-full items-center gap-2 px-2 text-left text-sm text-ink-2 hover:bg-raised hover:text-ink"
            disabled={busy}
            onclick={() => pick(a.id)}
          >
            <SquareLibrary size={14} strokeWidth={1.5} /><span class="flex-1 truncate">{a.name}</span>
            <span class="data text-2xs text-ink-3">{count(a.mediaCount)}</span>
          </button>
        </li>
      {:else}
        {#if !fresh}<li class="px-2 py-1 text-xs text-ink-3">Henüz albüm yok. Bir ad yazıp Enter'a basın.</li>{/if}
      {/each}
    </ul>
    <div class="flex justify-end">
      <button class="btn" disabled={busy} onclick={close}>Vazgeç</button>
    </div>
  </div>
</div>

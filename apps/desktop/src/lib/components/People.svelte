<script lang="ts" module>
  import type { PersonCard } from "../api/types";

  /** Kart anahtarı: adlandırılmış kişi `p<id>`, isimsiz küme `c<id>`. */
  export function cardKey(c: PersonCard): string {
    return c.personId !== null ? `p${c.personId}` : `c${c.clusterId ?? 0}`;
  }

  export type MergePlan =
    | { kind: "persons"; into: number; from: number[]; clusters: number[] }
    | { kind: "clusters"; into: number; from: number[] };

  /**
   * Seçili kartların birleştirme planı. Hedef: en çok fotoğraflı adlandırılmış
   * kişi; hiç adlı yoksa en büyük küme. İsimsiz kümeler hedef kişiye adıyla katılır.
   */
  export function mergePlan(cards: PersonCard[]): MergePlan | null {
    if (cards.length < 2) return null;
    const named = cards.filter((c) => c.personId !== null).sort((a, b) => b.photoCount - a.photoCount);
    const unnamed = cards.filter((c) => c.personId === null && c.clusterId !== null);
    const target = named[0];
    if (target && target.personId !== null) {
      return {
        kind: "persons",
        into: target.personId,
        from: named.slice(1).flatMap((c) => (c.personId === null ? [] : [c.personId])),
        clusters: unnamed.flatMap((c) => (c.clusterId === null ? [] : [c.clusterId])),
      };
    }
    const sorted = [...unnamed].sort((a, b) => b.faceCount - a.faceCount);
    const into = sorted[0]?.clusterId;
    if (into === null || into === undefined) return null;
    return { kind: "clusters", into, from: sorted.slice(1).flatMap((c) => (c.clusterId === null ? [] : [c.clusterId])) };
  }
</script>

<script lang="ts">
  import EyeOff from "@lucide/svelte/icons/eye-off";
  import Merge from "@lucide/svelte/icons/merge";
  import Unlink from "@lucide/svelte/icons/unlink";
  import X from "@lucide/svelte/icons/x";
  import { onMount } from "svelte";

  import type { FaceRef, PeopleDto } from "../api/types";
  import { count } from "../format";
  import type { Catalog } from "../state/catalog.svelte";
  import FaceThumb from "./FaceThumb.svelte";

  interface Props {
    catalog: Catalog;
  }

  let { catalog }: Props = $props();

  let people = $state.raw<PeopleDto | null>(null);
  let selected = $state<string[]>([]);
  let focus = $state<string | null>(null);
  let faces = $state.raw<FaceRef[]>([]);
  let face = $state<number | null>(null);
  let name = $state("");
  let working = $state(false);

  const all = $derived([...(people?.named ?? []), ...(people?.unnamed ?? [])]);
  const byKey = $derived(new Map(all.map((c) => [cardKey(c), c])));
  const focused = $derived(focus ? byKey.get(focus) : undefined);
  const plan = $derived(mergePlan(selected.flatMap((k) => byKey.get(k) ?? [])));
  const ai = $derived(catalog.status?.ai);

  onMount(() => void load());

  async function load(): Promise<void> {
    try {
      people = await catalog.backend.listPeople();
      selected = selected.filter((k) => byKey.has(k));
      if (focus && !byKey.has(focus)) focus = null;
      if (focused) await loadFaces(focused);
      else faces = [];
    } catch (e) {
      catalog.fail(e);
    }
  }

  async function loadFaces(c: PersonCard): Promise<void> {
    faces = await catalog.backend.personFaces(
      c.personId !== null ? { person: c.personId } : { cluster: c.clusterId ?? undefined },
      300,
    );
    if (face !== null && !faces.some((f) => f.faceId === face)) face = null;
  }

  function pick(c: PersonCard, e: MouseEvent): void {
    const key = cardKey(c);
    if (e.ctrlKey || e.metaKey || e.shiftKey) {
      selected = selected.includes(key) ? selected.filter((k) => k !== key) : [...selected, key];
      return;
    }
    selected = [key];
    focus = key;
    name = c.name ?? "";
    face = null;
    void loadFaces(c).catch((err: unknown) => catalog.fail(err));
  }

  function openPhotos(c: PersonCard): void {
    const view =
      c.personId !== null ? { kind: "person" as const, id: c.personId } : { kind: "cluster" as const, id: c.clusterId ?? 0 };
    void catalog.open(view, catalog.grouping, {
      title: c.name ?? "İsimsiz kişi",
      subtitle: `${count(c.photoCount)} fotoğraf`,
    });
  }

  /** Değiştiren her işlem: hata durum çubuğuna, sonra liste ve kenar çubuğu yenilenir. */
  async function act(fn: () => Promise<unknown>, done?: string): Promise<void> {
    if (working) return;
    working = true;
    try {
      await fn();
      if (done) catalog.flash(done);
    } catch (e) {
      catalog.fail(e);
    } finally {
      working = false;
    }
    await load();
    void catalog.refreshSidebar();
  }

  function saveName(): void {
    const c = focused;
    const n = name.trim();
    if (!c || !n || n === c.name) return;
    void act(async () => {
      const id =
        c.personId !== null
          ? await catalog.backend.renamePerson(c.personId, n)
          : await catalog.backend.nameCluster(c.clusterId ?? 0, n);
      focus = `p${id}`;
      selected = [focus];
    }, `Adlandırıldı: ${n}`);
  }

  function merge(): void {
    const p = plan;
    if (!p) return;
    void act(async () => {
      if (p.kind === "clusters") {
        await catalog.backend.mergeClusters(p.into, p.from);
        focus = `c${p.into}`;
      } else {
        if (p.from.length) await catalog.backend.mergePersons(p.into, p.from);
        const target = all.find((c) => c.personId === p.into);
        for (const cl of p.clusters) await catalog.backend.nameCluster(cl, target?.name ?? "");
        focus = `p${p.into}`;
      }
      selected = focus ? [focus] : [];
    }, "Birleştirildi");
  }

  function ignore(c: PersonCard): void {
    if (c.clusterId === null) return;
    void act(() => catalog.backend.ignoreCluster(c.clusterId ?? 0, true), "Küme yoksayıldı");
  }

  function detach(id: number): void {
    void act(() => catalog.backend.detachFace(id), "Yüz ayrıldı");
  }

  function reject(id: number): void {
    void act(() => catalog.backend.rejectFace(id), "Yüz değil olarak işaretlendi");
  }

  function move(id: number, target: string): void {
    const c = byKey.get(target);
    if (!c) return;
    void act(
      () =>
        c.personId !== null
          ? catalog.backend.assignFace(id, c.personId)
          : catalog.backend.moveFace(id, c.clusterId ?? 0),
      `Yüz taşındı: ${c.name ?? "isimsiz küme"}`,
    );
  }

  function onkeydown(e: KeyboardEvent): void {
    if (e.key === "Enter") {
      e.preventDefault();
      saveName();
    } else if (e.key === "Escape") {
      name = focused?.name ?? "";
      (e.currentTarget as HTMLInputElement).blur();
    }
  }
</script>

{#snippet card(c: PersonCard)}
  {@const key = cardKey(c)}
  <li>
    <button
      class="flex w-full flex-col gap-1 p-1 text-left hover:bg-raised"
      class:selected={selected.includes(key)}
      onclick={(e) => pick(c, e)}
      ondblclick={() => openPhotos(c)}
      title="Tıkla: seç · Ctrl+tıkla: çoklu seçim · Çift tıkla: fotoğraflar"
    >
      <FaceThumb {catalog} face={c.cover} width={104} height={128} />
      <span class="truncate text-sm {c.name ? 'text-ink' : 'text-ink-3'}">{c.name ?? "İsimsiz"}</span>
      <span class="data text-2xs text-ink-3">{count(c.photoCount)} fotoğraf</span>
    </button>
  </li>
{/snippet}

<section class="flex h-full min-w-0 flex-1" aria-label="Kişiler">
  <div class="flex min-w-0 flex-1 flex-col">
    <header class="flex h-toolbar shrink-0 items-center gap-3 border-b border-line px-4">
      <h1 class="text-sm font-medium text-ink">Kişiler</h1>
      <span class="data text-2xs text-ink-3">
        {count(people?.named.length ?? 0)} kişi · {count(people?.unnamed.length ?? 0)} isimsiz{people?.ignored
          ? ` · ${count(people.ignored)} yoksayıldı`
          : ""}
      </span>
      {#if ai && ai.mode !== "off" && ai.pending.faces + ai.pending.clustering > 0}
        <span class="data text-2xs text-amber">
          {ai.paused ? "duraklatıldı" : "taranıyor"} · {count(ai.pending.faces)} fotoğraf kaldı
        </span>
      {/if}
      <span class="flex-1"></span>
      {#if plan}
        <span class="data text-2xs text-ink-2">{selected.length} seçili</span>
        <button class="btn" onclick={merge} disabled={working}>
          <Merge size={14} strokeWidth={1.5} />Birleştir
        </button>
        <button class="btn btn-ghost" onclick={() => (selected = focus ? [focus] : [])} aria-label="Seçimi temizle">
          <X size={14} strokeWidth={1.5} />
        </button>
      {/if}
    </header>

    <div class="min-h-0 flex-1 overflow-y-auto px-4 py-3">
      {#if ai?.mode === "off"}
        <p class="mb-3 border border-line px-3 py-2 text-xs text-ink-2">
          Yüz tanıma kapalı{ai.reason ? `: ${ai.reason}` : ""}. Modeller kurulunca tarama kendiliğinden başlar.
        </p>
      {/if}
      {#if people && all.length === 0}
        <p class="label pt-6">Henüz yüz bulunmadı</p>
      {/if}
      {#if people && people.named.length > 0}
        <h2 class="label pb-2">Adlandırılmış</h2>
        <ul class="grid grid-cols-[repeat(auto-fill,minmax(112px,1fr))] gap-2 pb-4">
          {#each people.named as c (cardKey(c))}{@render card(c)}{/each}
        </ul>
      {/if}
      {#if people && people.unnamed.length > 0}
        <h2 class="label pb-2">İsimsiz</h2>
        <ul class="grid grid-cols-[repeat(auto-fill,minmax(112px,1fr))] gap-2">
          {#each people.unnamed as c (cardKey(c))}{@render card(c)}{/each}
        </ul>
      {/if}
    </div>
  </div>

  {#if focused}
    {@const c = focused}
    <aside class="flex w-inspector shrink-0 flex-col border-l border-line bg-surface" aria-label="Kişi ayrıntısı">
      <div class="flex gap-3 border-b border-line p-3">
        <FaceThumb {catalog} face={c.cover} width={72} height={88} />
        <div class="flex min-w-0 flex-1 flex-col gap-2">
          <input
            class="input w-full"
            bind:value={name}
            {onkeydown}
            onblur={saveName}
            placeholder="Ad ver"
            aria-label="Kişinin adı"
          />
          <span class="data text-2xs text-ink-3">{count(c.faceCount)} yüz · {count(c.photoCount)} fotoğraf</span>
        </div>
      </div>
      <div class="flex gap-2 border-b border-line p-3">
        <button class="btn flex-1 justify-center" onclick={() => openPhotos(c)}>Fotoğrafları aç</button>
        {#if c.personId === null}
          <button class="btn" onclick={() => ignore(c)} title="Bu kümeyi gizle" aria-label="Yoksay">
            <EyeOff size={14} strokeWidth={1.5} />
          </button>
        {/if}
      </div>

      <h2 class="label px-3 pt-3 pb-2">Yüzler</h2>
      <ul class="grid min-h-0 flex-1 auto-rows-min grid-cols-3 gap-1 overflow-y-auto px-3 pb-3">
        {#each faces as f (f.faceId)}
          <li>
            <button
              class="block"
              class:selected={face === f.faceId}
              onclick={() => (face = face === f.faceId ? null : f.faceId)}
              aria-label="Yüz {f.faceId}"
              aria-pressed={face === f.faceId}
            >
              <FaceThumb {catalog} face={f} width={80} height={96} size="md" />
            </button>
          </li>
        {/each}
      </ul>

      {#if face !== null}
        {@const id = face}
        <div class="flex flex-col gap-2 border-t border-line p-3" aria-label="Yüz işlemleri">
          <select
            class="input w-full"
            aria-label="Yüzü taşı"
            onchange={(e) => {
              const v = (e.currentTarget as HTMLSelectElement).value;
              if (v) move(id, v);
            }}
          >
            <option value="">Başka kişiye taşı…</option>
            {#each all.filter((x) => cardKey(x) !== focus) as x (cardKey(x))}
              <option value={cardKey(x)}>{x.name ?? `İsimsiz küme ${x.clusterId}`} · {x.faceCount}</option>
            {/each}
          </select>
          <div class="flex gap-2">
            <button class="btn flex-1 justify-center" onclick={() => detach(id)} disabled={working}>
              <Unlink size={14} strokeWidth={1.5} />Ayır
            </button>
            <button class="btn flex-1 justify-center" onclick={() => reject(id)} disabled={working}>Yüz değil</button>
          </div>
        </div>
      {/if}
    </aside>
  {/if}
</section>

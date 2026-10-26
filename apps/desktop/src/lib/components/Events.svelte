<script lang="ts">
  import Lock from "@lucide/svelte/icons/lock";
  import LockOpen from "@lucide/svelte/icons/lock-open";
  import Pencil from "@lucide/svelte/icons/pencil";
  import { onMount, tick } from "svelte";

  import type { EventCard } from "../api/types";
  import { count } from "../format";
  import { coreText, t } from "../i18n";
  import type { Catalog } from "../state/catalog.svelte";

  interface Props {
    catalog: Catalog;
  }

  let { catalog }: Props = $props();

  let events = $state.raw<EventCard[] | null>(null);
  let editing = $state<number | null>(null);
  let draft = $state("");
  let field = $state<HTMLInputElement>();

  onMount(() => void load());

  async function load(): Promise<void> {
    try {
      events = await catalog.backend.listEvents();
    } catch (e) {
      catalog.fail(e);
    }
  }

  function replace(card: EventCard): void {
    events = (events ?? []).map((e) => (e.id === card.id ? card : e));
  }

  /** Otomatik başlık çekirdeğin Türkçe tarihidir; kilitli olan kullanıcının verdiği addır. */
  const titleOf = (e: EventCard): string => (e.locked ? e.title : coreText(e.title));

  function open(e: EventCard): void {
    void catalog.open({ kind: "event", id: e.id }, catalog.grouping, {
      title: titleOf(e),
      subtitle: [coreText(e.range), e.place].filter(Boolean).join(" · "),
    });
  }

  async function edit(e: EventCard): Promise<void> {
    editing = e.id;
    draft = e.title;
    await tick();
    field?.select();
  }

  async function commit(): Promise<void> {
    const id = editing;
    const title = draft.trim();
    editing = null;
    const current = events?.find((e) => e.id === id);
    if (id === null || !title || !current || title === current.title) return;
    try {
      replace(await catalog.backend.renameEvent(id, title));
      catalog.flash(t("events.renamed"));
    } catch (e) {
      catalog.fail(e);
    }
  }

  async function toggleLock(e: EventCard): Promise<void> {
    try {
      replace(await catalog.backend.setEventLocked(e.id, !e.locked));
    } catch (err) {
      catalog.fail(err);
    }
  }

  function onkeydown(e: KeyboardEvent): void {
    if (e.key === "Enter") {
      e.preventDefault();
      void commit();
    } else if (e.key === "Escape") {
      e.preventDefault();
      editing = null;
    }
  }

  const locked = $derived(events?.filter((e) => e.locked).length ?? 0);
</script>

<section class="flex h-full min-w-0 flex-1 flex-col" aria-label={t("view.events")}>
  <header class="flex h-toolbar shrink-0 items-center gap-3 border-b border-line px-4">
    <h1 class="text-sm font-medium text-ink">{t("view.events")}</h1>
    <span class="data text-2xs text-ink-3">{t("events.summary", { n: count(events?.length ?? 0), locked: count(locked) })}</span>
    <span class="flex-1"></span>
    <span class="text-xs text-ink-3">{t("events.hint")}</span>
  </header>

  <div class="min-h-0 flex-1 overflow-y-auto px-4 py-3">
    {#if events && events.length === 0}
      <p class="label pt-6">{t("events.none")}</p>
    {/if}
    <ul class="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-3">
      {#each events ?? [] as e (e.id)}
        <li class="flex flex-col border border-line bg-surface">
          <button class="block aspect-[3/2] w-full overflow-hidden bg-raised" onclick={() => open(e)} aria-label={t("events.open", { name: titleOf(e) })}>
            {#if e.coverHash}
              <img
                src={catalog.backend.thumbUrl("md", e.coverHash)}
                alt=""
                loading="lazy"
                draggable="false"
                class="h-full w-full object-cover"
              />
            {/if}
          </button>
          <div class="flex flex-col gap-1 p-2">
            <div class="flex items-center gap-1">
              {#if editing === e.id}
                <input
                  bind:this={field}
                  bind:value={draft}
                  {onkeydown}
                  onblur={commit}
                  class="input h-6 min-w-0 flex-1"
                  aria-label={t("events.titleLabel")}
                />
              {:else}
                <button class="min-w-0 flex-1 truncate text-left text-sm text-ink" ondblclick={() => edit(e)} onclick={() => open(e)}>
                  {titleOf(e)}
                </button>
                <button class="btn btn-ghost h-6 px-1" onclick={() => edit(e)} aria-label={t("vh.rename")}>
                  <Pencil size={12} strokeWidth={1.5} />
                </button>
              {/if}
              <button
                class="btn btn-ghost h-6 px-1 {e.locked ? 'text-amber' : 'text-ink-3'}"
                onclick={() => toggleLock(e)}
                aria-label={e.locked ? t("events.unlock") : t("events.lock")}
                aria-pressed={e.locked}
                title={e.locked ? t("events.lockedTitle") : t("events.autoTitle")}
              >
                {#if e.locked}<Lock size={12} strokeWidth={1.5} />{:else}<LockOpen size={12} strokeWidth={1.5} />{/if}
              </button>
            </div>
            <div class="data flex justify-between gap-2 text-2xs text-ink-3">
              <span class="truncate">{coreText(e.range)}</span>
              <span>{count(e.count)}</span>
            </div>
            {#if e.place}<span class="truncate text-xs text-ink-2">{e.place}</span>{/if}
          </div>
        </li>
      {/each}
    </ul>
  </div>
</section>

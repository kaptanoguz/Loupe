<script lang="ts">
  import BookmarkPlus from "@lucide/svelte/icons/bookmark-plus";
  import ExternalLink from "@lucide/svelte/icons/external-link";
  import FolderOpen from "@lucide/svelte/icons/folder-open";
  import Heart from "@lucide/svelte/icons/heart";
  import Maximize from "@lucide/svelte/icons/maximize";
  import Minus from "@lucide/svelte/icons/minus";
  import Play from "@lucide/svelte/icons/play";
  import Presentation from "@lucide/svelte/icons/presentation";
  import Trash from "@lucide/svelte/icons/trash";
  import { onMount, type Component } from "svelte";

  import { count } from "../format";
  import { t } from "../i18n";
  import type { Catalog } from "../state/catalog.svelte";

  interface Props {
    catalog: Catalog;
    x: number;
    y: number;
  }

  let { catalog, x, y }: Props = $props();

  interface Entry {
    label: string;
    key?: string;
    icon: Component;
    run: () => void | Promise<void>;
    danger?: boolean;
  }

  const item = $derived(catalog.selectedItem);
  const ids = $derived(catalog.targetIds());
  const many = $derived(ids.length > 1);
  const fav = $derived(catalog.allFavorite(ids));
  const album = $derived(catalog.view.kind === "album");

  const entries = $derived.by((): Entry[][] => {
    if (!item) return [];
    const it = item;
    const view: Entry[] = [];
    if (!catalog.loupeOpen) view.push({ label: t("menu.preview"), key: t("menu.space"), icon: Maximize, run: () => {
        catalog.loupeOpen = true;
      } });
    view.push({
      label: many ? t("menu.slideshowMarked", { n: count(ids.length) }) : t("menu.slideshowHere"),
      key: "S",
      icon: Presentation,
      run: () => catalog.startSlideshow(),
    });
    return [
      view,
      [
        {
          label: fav ? t("menu.unfavorite") : t("menu.favorite"),
          key: "F",
          icon: Heart,
          run: () => catalog.toggleFavorite(ids),
        },
        {
          label: t("menu.addToAlbum"),
          icon: BookmarkPlus,
          run: () => {
            catalog.overlay = "album";
          },
        },
        ...(album
          ? [
              {
                label: many ? t("menu.removeManyFromAlbum", { n: count(ids.length) }) : t("menu.removeFromAlbum"),
                icon: Minus,
                run: () => catalog.removeFromAlbum(ids),
              },
            ]
          : []),
      ],
      [
        { label: t("menu.reveal"), icon: FolderOpen, run: () => catalog.reveal(it.id) },
        it.video
          ? { label: t("menu.play"), icon: Play, run: () => catalog.openExternal(it.id) }
          : { label: t("menu.openDefault"), icon: ExternalLink, run: () => catalog.openExternal(it.id) },
      ],
      [
        {
          label: many ? t("menu.trashMany", { n: count(ids.length) }) : t("menu.trash"),
          key: "Delete",
          icon: Trash,
          danger: true,
          run: () => {
            catalog.overlay = "trash";
          },
        },
      ],
    ];
  });

  let el = $state<HTMLDivElement>();
  let pos = $state({ left: 0, top: 0 });

  onMount(() => {
    // Ekrandan taşmasın: sağ/alt kenara yakınsa imlecin soluna/üstüne açılır.
    const w = el?.offsetWidth ?? 0;
    const h = el?.offsetHeight ?? 0;
    pos = {
      left: x + w > window.innerWidth ? Math.max(0, x - w) : x,
      top: y + h > window.innerHeight ? Math.max(0, y - h) : y,
    };
    el?.querySelector<HTMLButtonElement>("button")?.focus();
  });

  function close(): void {
    catalog.menu = null;
  }

  async function choose(e: Entry): Promise<void> {
    close();
    await e.run();
  }

  function onkeydown(e: KeyboardEvent): void {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const buttons = [...(el?.querySelectorAll<HTMLButtonElement>("button") ?? [])];
    const at = buttons.indexOf(document.activeElement as HTMLButtonElement);
    const next = (at + (e.key === "ArrowDown" ? 1 : -1) + buttons.length) % buttons.length;
    buttons[next]?.focus();
  }
</script>

<div
  class="fixed inset-0 z-40"
  role="presentation"
  onmousedown={(e) => e.target === e.currentTarget && close()}
  oncontextmenu={(e) => {
    e.preventDefault();
    close();
  }}
>
  <div
    bind:this={el}
    class="fixed flex min-w-56 flex-col rounded-xs border border-line bg-raised py-1 text-sm"
    style:left="{pos.left}px"
    style:top="{pos.top}px"
    role="menu"
    aria-label={t("menu.label")}
    tabindex="-1"
    {onkeydown}
  >
    {#if many}
      <p class="px-3 py-1 text-2xs text-ink-3">{t("menu.selected", { n: count(ids.length) })}</p>
    {/if}
    {#each entries as group, g (g)}
      {#if g > 0}<div class="my-1 border-t border-line"></div>{/if}
      {#each group as e (e.label)}
        <button
          class="flex h-7 items-center gap-2 px-3 text-left hover:bg-line focus-visible:bg-line focus-visible:outline-none {e.danger
            ? 'text-amber'
            : 'text-ink'}"
          role="menuitem"
          onclick={() => choose(e)}
        >
          <e.icon size={14} strokeWidth={1.5} class={e.icon === Heart && fav ? "fill-ink" : ""} />
          <span class="flex-1">{e.label}</span>
          {#if e.key}<span class="data text-2xs text-ink-3">{e.key}</span>{/if}
        </button>
      {/each}
    {/each}
  </div>
</div>

<script lang="ts">
  import ChevronLeft from "@lucide/svelte/icons/chevron-left";
  import ChevronRight from "@lucide/svelte/icons/chevron-right";
  import Heart from "@lucide/svelte/icons/heart";
  import Pause from "@lucide/svelte/icons/pause";
  import Play from "@lucide/svelte/icons/play";
  import Shuffle from "@lucide/svelte/icons/shuffle";
  import X from "@lucide/svelte/icons/x";
  import { onDestroy } from "svelte";
  import { fade } from "svelte/transition";

  import { count } from "../format";
  import { SLIDE_SECONDS, type Catalog } from "../state/catalog.svelte";

  interface Props {
    catalog: Catalog;
  }

  let { catalog }: Props = $props();

  /** Fare durunca denetimler bu kadar sonra gizlenir (ms). */
  const IDLE_MS = 2500;
  const FADE_MS = 400;

  const show = $derived(catalog.slideshow);
  const item = $derived(catalog.selectedItem);
  const src = $derived(item ? catalog.backend.thumbUrl("lg", item.hash) : "");

  // Süre dolunca sonraki; elle geçişte ya da duraklatınca sayaç baştan kurulur.
  $effect(() => {
    const s = show;
    void item?.id;
    if (!s || s.paused) return;
    const t = setTimeout(() => catalog.slideStep(1), catalog.slide.seconds * 1000);
    return () => clearTimeout(t);
  });

  // Sonraki slayt önceden yüklenir; geçişte boş kare görünmez.
  $effect(() => {
    const s = show;
    if (!s) return;
    const next = catalog.items[s.order[(s.pos + 1) % s.order.length] ?? -1];
    if (next) new Image().src = catalog.backend.thumbUrl("lg", next.hash);
  });

  let active = $state(true);
  let idle: ReturnType<typeof setTimeout> | undefined;
  function wake(): void {
    active = true;
    clearTimeout(idle);
    idle = setTimeout(() => (active = false), IDLE_MS);
  }
  wake();
  onDestroy(() => clearTimeout(idle));

  const controls = $derived(active || (show?.paused ?? false));
</script>

<div
  class="fixed inset-0 z-40 bg-canvas select-none"
  class:cursor-none={!controls}
  role="dialog"
  aria-label="Slayt gösterisi"
  aria-modal="true"
  tabindex="-1"
  onmousemove={wake}
>
  {#key src}
    <img
      {src}
      alt=""
      draggable="false"
      class="absolute inset-0 h-full w-full object-contain"
      transition:fade={{ duration: FADE_MS }}
    />
  {/key}

  {#if show && controls}
    <div
      class="absolute inset-x-0 bottom-0 flex h-toolbar items-center justify-center gap-2 border-t border-line bg-surface px-3 text-xs text-ink-2"
      transition:fade={{ duration: 150 }}
    >
      <button class="btn btn-ghost h-7 px-2" title="Önceki (←)" aria-label="Önceki" onclick={() => catalog.slideStep(-1)}>
        <ChevronLeft size={16} strokeWidth={1.5} />
      </button>
      <button
        class="btn h-7 px-2"
        title="{show.paused ? 'Sürdür' : 'Duraklat'} (Boşluk)"
        aria-label={show.paused ? "Sürdür" : "Duraklat"}
        onclick={() => catalog.slidePause()}
      >
        {#if show.paused}<Play size={16} strokeWidth={1.5} />{:else}<Pause size={16} strokeWidth={1.5} />{/if}
      </button>
      <button class="btn btn-ghost h-7 px-2" title="Sonraki (→)" aria-label="Sonraki" onclick={() => catalog.slideStep(1)}>
        <ChevronRight size={16} strokeWidth={1.5} />
      </button>
      <span class="data w-28 text-center">{count(show.pos + 1)} / {count(show.order.length)}</span>
      <span class="label">Süre</span>
      {#each SLIDE_SECONDS as sec (sec)}
        <button
          class="btn h-7 px-2 {catalog.slide.seconds === sec ? 'border-amber text-ink' : 'text-ink-2'}"
          aria-pressed={catalog.slide.seconds === sec}
          onclick={() => catalog.setSlide({ seconds: sec })}>{sec} sn</button
        >
      {/each}
      <button
        class="btn h-7 px-2 {catalog.slide.shuffle ? 'border-amber text-ink' : 'text-ink-2'}"
        aria-pressed={catalog.slide.shuffle}
        title="Karışık sıra"
        onclick={() => catalog.setSlide({ shuffle: !catalog.slide.shuffle })}
      >
        <Shuffle size={14} strokeWidth={1.5} />Karışık
      </button>
      {#if item}
        <button
          class="btn btn-ghost h-7 px-2"
          title="Favori (F)"
          aria-label={item.favorite ? "Favorilerden çıkar" : "Favorilere ekle"}
          aria-pressed={item.favorite}
          onclick={() => catalog.toggleFavorite([item.id])}
        >
          <Heart size={16} strokeWidth={1.5} class={item.favorite ? "fill-ink" : ""} />
        </button>
      {/if}
      <button class="btn btn-ghost h-7 px-2" title="Kapat (Esc)" aria-label="Slayt gösterisini kapat" onclick={() => catalog.stopSlideshow()}>
        <X size={16} strokeWidth={1.5} />
      </button>
    </div>
  {/if}
</div>

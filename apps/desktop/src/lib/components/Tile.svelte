<script lang="ts">
  import Heart from "@lucide/svelte/icons/heart";

  import type { ColorLabel, SyncState } from "../api/types";
  import { LABEL_BG, LABEL_NAME } from "../design/labels";
  import { THUMB_RETRIES, THUMB_RETRY_MS } from "../design/layout";
  import { clock } from "../format";
  import { t } from "../i18n";

  interface Props {
    idx: number;
    src: string;
    x: number;
    y: number;
    w: number;
    h: number;
    selected: boolean;
    /** Sayfa henüz gelmediyse false: yalnızca boş yüzey. */
    ready: boolean;
    rating: number;
    label: ColorLabel | null;
    favorite: boolean;
    raw: string | null;
    video: boolean;
    /** Video süresi (ms). */
    duration: number | null;
    sync: SyncState;
  }

  let { idx, src, x, y, w, h, selected, ready, rating, label, favorite, raw, video, duration, sync }: Props = $props();
  /** Küçük karolarda göstergeler kalabalık etmesin. */
  const compact = $derived(h < 100 || w < 90);

  // Önbellekte olmayan boyut 404 döner ve çekirdek onu öne alır; birkaç kez,
  // artan aralıkla yeniden denenir (sorgu dizesi URI çözümlemesinde yok sayılır).
  let attempt = $state(0);
  let timer: ReturnType<typeof setTimeout> | undefined;
  const url = $derived(attempt > 0 && src ? `${src}?r=${attempt}` : src);

  $effect(() => {
    void src;
    attempt = 0;
    return () => clearTimeout(timer);
  });

  function onerror(): void {
    if (attempt >= THUMB_RETRIES) return;
    clearTimeout(timer);
    timer = setTimeout(() => (attempt += 1), THUMB_RETRY_MS * 2 ** attempt);
  }
</script>

<div
  class="tile"
  class:selected
  style:transform="translate3d({x}px, {y}px, 0)"
  style:width="{w}px"
  style:height="{h}px"
  style:display={idx < 0 ? "none" : null}
  data-idx={idx}
  role="gridcell"
  aria-selected={selected}
  tabindex="-1"
>
  {#if src}
    <img src={url} alt="" decoding="async" draggable="false" {onerror} />
  {/if}
  {#if ready}
    {#if rating > 0 || favorite}
      <span class="data scrim absolute top-1 left-1 flex h-4 items-center gap-1 px-1 text-2xs text-ink">
        {#if favorite}<Heart size={10} strokeWidth={1.5} class="fill-ink" aria-label={t("tile.favorite")} />{/if}
        {#if rating > 0}<span aria-label={t("tile.stars", { n: rating })}>{compact ? `★${rating}` : "★".repeat(rating)}</span>{/if}
      </span>
    {/if}
    {#if (raw || video) && !compact}
      <span class="data hairline scrim absolute top-1 right-1 h-4 px-1 text-2xs leading-3.5 text-ink-2">
        {video ? (duration ? clock(duration / 1000) : "VID") : raw}
      </span>
    {/if}
    {#if label}
      <span class="label-dot {LABEL_BG[label]}" aria-label={t("tile.label", { label: LABEL_NAME[label] })}></span>
    {/if}
    <span class="sync-dot" data-sync={sync}></span>
  {/if}
</div>

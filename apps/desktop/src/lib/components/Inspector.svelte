<script lang="ts">
  import Check from "@lucide/svelte/icons/check";
  import Copy from "@lucide/svelte/icons/copy";
  import FolderOpen from "@lucide/svelte/icons/folder-open";
  import Heart from "@lucide/svelte/icons/heart";

  import type { InspectorDetail } from "../api/types";
  import { LABEL_BG, LABEL_NAME } from "../design/labels";
  import { bytes, dateTimeMs, resolution, shortHash } from "../format";
  import { t } from "../i18n";
  import Histogram from "./Histogram.svelte";

  interface Props {
    detail: InspectorDetail | null;
    onFavorite?: (id: number) => void;
    onReveal?: (id: number) => void;
  }

  let { detail, onFavorite, onReveal }: Props = $props();
  let copied = $state(false);

  const SYNC_TEXT: Record<string, string> = {
    local_only: t("sync.local_only"),
    queued: t("sync.queued"),
    uploading: t("sync.uploading"),
    synced: t("sync.synced"),
    conflict: t("sync.conflict"),
    error: t("sync.error"),
    remote_deleted: t("sync.remote_deleted"),
  };

  const camera = $derived.by(() => {
    if (!detail) return null;
    const make = detail.cameraMake ?? "";
    const model = detail.cameraModel ?? "";
    // "Canon Canon EOS R5" tekrarını önle.
    return model.toLowerCase().startsWith(make.toLowerCase()) ? model || null : `${make} ${model}`.trim() || null;
  });

  async function copyHash(): Promise<void> {
    if (!detail) return;
    try {
      await navigator.clipboard.writeText(detail.hash);
      copied = true;
      setTimeout(() => (copied = false), 1200);
    } catch {
      copied = false;
    }
  }
</script>

<aside
  class="flex h-full w-inspector shrink-0 flex-col overflow-y-auto border-l border-line bg-surface"
  aria-label={t("insp.label")}
>
  <Histogram histogram={detail?.histogram ?? null} />

  {#if detail}
    <!-- Pozlama: Swiss grid, 1px bölücüler -->
    <section class="border-b border-line">
      <div class="px-3 pt-3 pb-2">
        <p class="truncate text-sm text-ink" title={camera ?? ""}>{camera ?? t("insp.unknownCamera")}</p>
        <p class="truncate text-xs text-ink-2" title={detail.lensModel ?? ""}>{detail.lensModel ?? "—"}</p>
      </div>
      <dl class="grid grid-cols-2 border-t border-line">
        {#each [[t("insp.aperture"), detail.aperture], [t("insp.shutter"), detail.exposure], ["ISO", detail.iso?.toString() ?? null], [t("insp.focal"), detail.focalLength]] as [k, v], i (k)}
          <div class="px-3 py-2 {i % 2 === 1 ? 'border-l border-line' : ''} {i >= 2 ? 'border-t border-line' : ''}">
            <dt class="label">{k}</dt>
            <dd class="data mt-0.5 text-base text-ink">{v ?? "—"}</dd>
          </div>
        {/each}
      </dl>
    </section>

    <section class="border-b border-line px-3 py-3">
      <h3 class="label mb-2">{t("insp.capture")}</h3>
      <dl class="kv gap-y-1">
        <dt>{t("insp.date")}</dt>
        <dd>{detail.takenLocal}</dd>
        <dt>{t("insp.source")}</dt>
        <dd>{detail.takenSource}</dd>
        <dt>{t("arrange.rating")}</dt>
        <dd aria-label={t("tile.stars", { n: detail.rating })}>{detail.rating > 0 ? "★".repeat(detail.rating) : "—"}</dd>
        <dt>{t("insp.label2")}</dt>
        <dd class="flex items-center justify-end gap-1.5">
          {#if detail.label}
            <span class="size-1.5 {LABEL_BG[detail.label]}"></span>{LABEL_NAME[detail.label]}
          {:else}—{/if}
        </dd>
        <dt>{t("insp.favorite")}</dt>
        <dd class="flex justify-end">
          {#if onFavorite}
            {@const id = detail.id}
            <button
              class="flex items-center gap-1 hover:text-ink"
              title={t("insp.favoriteKey")}
              aria-label={detail.favorite ? t("menu.unfavorite") : t("menu.favorite")}
              aria-pressed={detail.favorite}
              onclick={() => onFavorite(id)}
            >
              <Heart size={12} strokeWidth={1.5} class={detail.favorite ? "fill-ink" : ""} />{detail.favorite
                ? t("common.yes")
                : t("insp.add")}
            </button>
          {:else}{detail.favorite ? t("common.yes") : t("common.no")}{/if}
        </dd>
      </dl>
    </section>

    <section class="border-b border-line px-3 py-3">
      <h3 class="label mb-2">{t("insp.location")}</h3>
      {#if detail.gps}
        <p class="data text-xs text-ink">{detail.gps.dms}</p>
        {#if detail.gps.altM !== null}
          <p class="data text-2xs text-ink-3">{detail.gps.altM.toFixed(0)} m</p>
        {/if}
        <p class="mt-1 text-xs text-ink-2">{detail.place?.display ?? t("insp.noPlace")}</p>
      {:else}
        <p class="text-xs text-ink-3">{t("insp.noGps")}</p>
      {/if}
    </section>

    <section class="border-b border-line px-3 py-3">
      <h3 class="label mb-2">{t("insp.file")}</h3>
      <p class="truncate text-xs text-ink" title={detail.fileName}>{detail.fileName}</p>
      <p class="data truncate text-2xs text-ink-3" title={detail.path ?? detail.relPath}>
        {detail.path ?? detail.relPath}
      </p>
      {#if onReveal}
        {@const id = detail.id}
        <button class="btn mt-2 h-6 text-xs" onclick={() => onReveal(id)}
          ><FolderOpen size={12} strokeWidth={1.5} />{t("menu.reveal")}</button
        >
      {/if}
      <dl class="kv mt-2 gap-y-1">
        <dt>{t("insp.size")}</dt>
        <dd>{bytes(detail.sizeBytes)}</dd>
        <dt>{t("insp.resolution")}</dt>
        <dd>{resolution(detail.width, detail.height)}</dd>
        {#if detail.megapixels !== null}
          <dt>{t("insp.megapixels")}</dt>
          <dd>{detail.megapixels.toFixed(1)} MP</dd>
        {/if}
        <dt>{t("insp.type")}</dt>
        <dd>{detail.rawFormat ?? detail.mime}</dd>
        <dt>{t("insp.library")}</dt>
        <dd>{detail.libraryName}{detail.libraryOnline ? "" : ` (${t("sidebar.offline")})`}</dd>
        <dt>{t("settings.verify")}</dt>
        <dd>{detail.verifyState}</dd>
        <dt>BLAKE3</dt>
        <dd class="flex items-center justify-end gap-1">
          <span title={detail.hash}>{shortHash(detail.hash)}</span>
          <button
            class="btn btn-ghost h-5 px-1"
            onclick={copyHash}
            aria-label={t("insp.copyHash")}
            title={t("insp.copyFull")}
          >
            {#if copied}<Check size={12} strokeWidth={1.5} />{:else}<Copy size={12} strokeWidth={1.5} />{/if}
          </button>
        </dd>
      </dl>
    </section>

    <section class="px-3 py-3">
      <h3 class="label mb-2">{t("insp.backup")}</h3>
      {#if detail.sync.length === 0}
        <p class="text-xs text-ink-3">{t("insp.noCloud")}</p>
      {:else}
        <ul class="flex flex-col gap-2">
          {#each detail.sync as s (s.target)}
            <li class="text-xs">
              <div class="flex items-center justify-between">
                <span class="text-ink">{s.target}</span>
                <span class="data {s.state === 'error' || s.state === 'conflict' ? 'text-amber' : 'text-ink-2'}">
                  {SYNC_TEXT[s.state] ?? s.state}
                </span>
              </div>
              <div class="data flex items-center justify-between text-2xs text-ink-3">
                <span>{s.provider}</span>
                <span>
                  {s.encryption === "age" ? `age · ${s.keyFingerprint ? shortHash(s.keyFingerprint) : t("insp.keyUnknown")}` : t("insp.unencrypted")}
                </span>
              </div>
              {#if s.uploadedAt}
                <div class="data text-2xs text-ink-3">{dateTimeMs(s.uploadedAt)}</div>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </section>
  {:else}
    <p class="px-3 py-3 text-xs text-ink-3">{t("insp.noSelection")}</p>
  {/if}
</aside>

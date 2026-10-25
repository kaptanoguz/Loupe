<script lang="ts">
  import Check from "@lucide/svelte/icons/check";
  import Copy from "@lucide/svelte/icons/copy";

  import type { InspectorDetail } from "../api/types";
  import { LABEL_BG, LABEL_NAME } from "../design/labels";
  import { bytes, dateTime, resolution, shortHash } from "../format";
  import Histogram from "./Histogram.svelte";

  interface Props {
    detail: InspectorDetail | null;
  }

  let { detail }: Props = $props();
  let copied = $state(false);

  const SYNC_TEXT: Record<string, string> = {
    local_only: "yalnızca yerel",
    queued: "kuyrukta",
    uploading: "yükleniyor",
    synced: "yedeklendi",
    conflict: "çakışma",
    error: "hata",
    remote_deleted: "uzakta silindi",
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
  aria-label="Denetçi"
>
  <Histogram histogram={detail?.histogram ?? null} />

  {#if detail}
    <!-- Pozlama: Swiss grid, 1px bölücüler -->
    <section class="border-b border-line">
      <div class="px-3 pt-3 pb-2">
        <p class="truncate text-sm text-ink" title={camera ?? ""}>{camera ?? "Kamera bilinmiyor"}</p>
        <p class="truncate text-xs text-ink-2" title={detail.lensModel ?? ""}>{detail.lensModel ?? "—"}</p>
      </div>
      <dl class="grid grid-cols-2 border-t border-line">
        {#each [["Diyafram", detail.aperture], ["Enstantane", detail.exposure], ["ISO", detail.iso?.toString() ?? null], ["Odak", detail.focalLength]] as [k, v], i (k)}
          <div class="px-3 py-2 {i % 2 === 1 ? 'border-l border-line' : ''} {i >= 2 ? 'border-t border-line' : ''}">
            <dt class="label">{k}</dt>
            <dd class="data mt-0.5 text-base text-ink">{v ?? "—"}</dd>
          </div>
        {/each}
      </dl>
    </section>

    <section class="border-b border-line px-3 py-3">
      <h3 class="label mb-2">Çekim</h3>
      <dl class="kv gap-y-1">
        <dt>Tarih</dt>
        <dd>{detail.takenLocal}</dd>
        <dt>Kaynak</dt>
        <dd>{detail.takenSource}</dd>
        <dt>Puan</dt>
        <dd aria-label="{detail.rating} yıldız">{detail.rating > 0 ? "★".repeat(detail.rating) : "—"}</dd>
        <dt>Etiket</dt>
        <dd class="flex items-center justify-end gap-1.5">
          {#if detail.label}
            <span class="size-1.5 {LABEL_BG[detail.label]}"></span>{LABEL_NAME[detail.label]}
          {:else}—{/if}
        </dd>
        <dt>Favori</dt>
        <dd>{detail.favorite ? "evet" : "hayır"}</dd>
      </dl>
    </section>

    <section class="border-b border-line px-3 py-3">
      <h3 class="label mb-2">Konum</h3>
      {#if detail.gps}
        <p class="data text-xs text-ink">{detail.gps.dms}</p>
        {#if detail.gps.altM !== null}
          <p class="data text-2xs text-ink-3">{detail.gps.altM.toFixed(0)} m</p>
        {/if}
        <p class="mt-1 text-xs text-ink-2">{detail.place?.display ?? "Yer çözülmedi"}</p>
      {:else}
        <p class="text-xs text-ink-3">Konum bilgisi yok</p>
      {/if}
    </section>

    <section class="border-b border-line px-3 py-3">
      <h3 class="label mb-2">Dosya</h3>
      <p class="truncate text-xs text-ink" title={detail.fileName}>{detail.fileName}</p>
      <p class="data truncate text-2xs text-ink-3" title={detail.path ?? detail.relPath}>
        {detail.path ?? detail.relPath}
      </p>
      <dl class="kv mt-2 gap-y-1">
        <dt>Boyut</dt>
        <dd>{bytes(detail.sizeBytes)}</dd>
        <dt>Çözünürlük</dt>
        <dd>{resolution(detail.width, detail.height)}</dd>
        {#if detail.megapixels !== null}
          <dt>Megapiksel</dt>
          <dd>{detail.megapixels.toFixed(1)} MP</dd>
        {/if}
        <dt>Tür</dt>
        <dd>{detail.rawFormat ?? detail.mime}</dd>
        <dt>Kütüphane</dt>
        <dd>{detail.libraryName}{detail.libraryOnline ? "" : " (çevrimdışı)"}</dd>
        <dt>Doğrulama</dt>
        <dd>{detail.verifyState}</dd>
        <dt>BLAKE3</dt>
        <dd class="flex items-center justify-end gap-1">
          <span title={detail.hash}>{shortHash(detail.hash)}</span>
          <button
            class="btn btn-ghost h-5 px-1"
            onclick={copyHash}
            aria-label="BLAKE3 özetini kopyala"
            title="Tam özeti kopyala"
          >
            {#if copied}<Check size={12} strokeWidth={1.5} />{:else}<Copy size={12} strokeWidth={1.5} />{/if}
          </button>
        </dd>
      </dl>
    </section>

    <section class="px-3 py-3">
      <h3 class="label mb-2">Yedek</h3>
      {#if detail.sync.length === 0}
        <p class="text-xs text-ink-3">Bulut hedefi yok</p>
      {:else}
        <ul class="flex flex-col gap-2">
          {#each detail.sync as t (t.target)}
            <li class="text-xs">
              <div class="flex items-center justify-between">
                <span class="text-ink">{t.target}</span>
                <span class="data {t.state === 'error' || t.state === 'conflict' ? 'text-amber' : 'text-ink-2'}">
                  {SYNC_TEXT[t.state] ?? t.state}
                </span>
              </div>
              <div class="data flex items-center justify-between text-2xs text-ink-3">
                <span>{t.provider}</span>
                <span>
                  {t.encryption === "age" ? `age · ${t.keyFingerprint ? shortHash(t.keyFingerprint) : "anahtar ?"}` : "şifresiz"}
                </span>
              </div>
              {#if t.uploadedAt}
                <div class="data text-2xs text-ink-3">{dateTime(t.uploadedAt)}</div>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </section>
  {:else}
    <p class="px-3 py-3 text-xs text-ink-3">Seçim yok</p>
  {/if}
</aside>

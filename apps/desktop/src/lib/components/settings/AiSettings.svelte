<script lang="ts">
  import Pause from "@lucide/svelte/icons/pause";
  import Play from "@lucide/svelte/icons/play";

  import type { AiStatusDto, ResourceMode } from "../../api/types";
  import { count } from "../../format";
  import type { Catalog } from "../../state/catalog.svelte";

  interface Props {
    catalog: Catalog;
  }

  let { catalog }: Props = $props();

  const MODES: { key: ResourceMode; name: string; hint: string }[] = [
    { key: "quiet", name: "Sessiz", hint: "Tek arka plan işçisi; pilde ya da makine meşgulken" },
    { key: "balanced", name: "Dengeli", hint: "İşçilerin yarısı (varsayılan); arayüz akıcı kalır" },
    { key: "full", name: "Tam", hint: "Tüm işçiler; ilk içe aktarma en hızlı biter" },
  ];

  /** Komut yanıtı bir sonraki durum yoklamasını beklemeden gösterilir. */
  let local = $state.raw<AiStatusDto | null>(null);
  const ai = $derived(local ?? catalog.status?.ai ?? null);

  $effect(() => {
    // Yoklama yeni durum getirince yerel kopya bırakılır.
    void catalog.status;
    local = null;
  });

  async function togglePause(): Promise<void> {
    if (!ai) return;
    try {
      local = await catalog.backend.setPaused(!ai.paused);
      catalog.flash(local.paused ? "Arka plan analizi duraklatıldı" : "Arka plan analizi sürüyor");
    } catch (e) {
      catalog.fail(e);
    }
  }

  async function setMode(mode: ResourceMode): Promise<void> {
    try {
      local = await catalog.backend.setResourceMode(mode);
    } catch (e) {
      catalog.fail(e);
    }
  }

  function pct(done: number, total: number): string {
    return total > 0 ? `${Math.floor((100 * done) / total)}%` : "—";
  }
</script>

<div class="flex flex-col gap-4 p-4">
  {#if ai}
    <section class="flex items-start gap-4">
      <div class="flex flex-1 flex-col gap-1">
        <span class="label">Model</span>
        <span class="text-sm {ai.mode === 'off' ? 'text-amber' : 'text-ink'}">
          {ai.mode === "onnx" ? "ONNX (yerel, çevrimdışı)" : ai.mode === "mock" ? "Deneme modeli" : "Kapalı"}
        </span>
        {#if ai.reason}<span class="text-xs text-ink-3">{ai.reason}</span>{/if}
        <span class="text-xs text-ink-3">
          {ai.semantic ? "Anlamsal arama (CLIP) açık" : "Anlamsal arama kapalı: palet yalnızca sözcük arar"}
        </span>
      </div>
      <button class="btn" onclick={togglePause} aria-pressed={ai.paused}>
        {#if ai.paused}<Play size={14} strokeWidth={1.5} />Sürdür{:else}<Pause size={14} strokeWidth={1.5} />Duraklat{/if}
      </button>
    </section>

    <section class="data grid grid-cols-3 border border-line text-xs" aria-label="Analiz ilerlemesi">
      <div class="px-3 py-2">
        <div class="label">Yüz taraması</div>
        <div class="text-ink">{count(ai.facesDone)}/{count(ai.images)} · {pct(ai.facesDone, ai.images)}</div>
        <div class="text-2xs text-ink-3">{count(ai.pending.faces)} sırada</div>
      </div>
      <div class="border-l border-line px-3 py-2">
        <div class="label">CLIP vektörü</div>
        <div class="text-ink">{count(ai.clipDone)}/{count(ai.images)} · {pct(ai.clipDone, ai.images)}</div>
        <div class="text-2xs text-ink-3">{count(ai.pending.clip)} sırada</div>
      </div>
      <div class="border-l border-line px-3 py-2">
        <div class="label">Kümeleme</div>
        <div class="text-ink">{ai.pending.clustering > 0 ? "bekliyor" : "güncel"}</div>
        <div class="text-2xs text-ink-3">kişiler ve etkinlikler</div>
      </div>
    </section>

    <section>
      <h2 class="label pb-2">Kaynak kullanımı</h2>
      <div class="grid grid-cols-3 border border-line" role="radiogroup" aria-label="Kaynak kullanımı">
        {#each MODES as m, i (m.key)}
          <button
            class="flex flex-col items-start gap-1 px-3 py-2 text-left {i > 0 ? 'border-l border-line' : ''} {ai.resourceMode ===
            m.key
              ? 'bg-raised text-ink'
              : 'text-ink-3 hover:text-ink-2'}"
            role="radio"
            aria-checked={ai.resourceMode === m.key}
            onclick={() => setMode(m.key)}
          >
            <span class="text-sm">{m.name}</span>
            <span class="text-xs text-ink-3">{m.hint}</span>
          </button>
        {/each}
      </div>
    </section>
  {:else}
    <p class="text-xs text-ink-3">Durum yükleniyor…</p>
  {/if}
</div>

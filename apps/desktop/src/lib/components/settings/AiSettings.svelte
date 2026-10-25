<script lang="ts" module>
  import type { ModelFileState } from "../../api/types";

  export const ROLE_NAMES: Record<string, string> = {
    face_detect: "Yüz tespiti · SCRFD",
    face_embed: "Yüz vektörü · ArcFace",
    clip_image: "CLIP görüntü",
    clip_text: "CLIP metin (çok dilli)",
    tokenizer: "Metin belirteçleyici",
    runtime: "ONNX Runtime",
  };

  export const STATE_NAMES: Record<ModelFileState, string> = {
    verified: "doğrulandı",
    missing: "yok",
    corrupt: "bozuk",
    unpinned: "özet bilinmiyor",
  };

  export const PHASE_NAMES: Record<string, string> = {
    resolve: "özet alınıyor",
    download: "indiriliyor",
    verify: "SHA-256 doğrulanıyor",
    extract: "arşivden çıkarılıyor",
    done: "tamam",
  };

  /** İndirme sürerken durum yoklama aralığı (ms). */
  export const MODEL_POLL_MS = 500;
</script>

<script lang="ts">
  import Download from "@lucide/svelte/icons/download";
  import Pause from "@lucide/svelte/icons/pause";
  import Play from "@lucide/svelte/icons/play";
  import X from "@lucide/svelte/icons/x";
  import { onDestroy, onMount } from "svelte";

  import type { AiStatusDto, ModelsDto, ResourceMode } from "../../api/types";
  import { bytes, count } from "../../format";
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

  let models = $state.raw<ModelsDto | null>(null);
  let pollTimer: ReturnType<typeof setTimeout> | undefined;

  const missing = $derived(models ? models.files.filter((f) => f.state !== "verified") : []);
  const missingBytes = $derived(missing.reduce((a, f) => a + (f.size ?? 0), 0));

  onMount(() => void loadModels());
  onDestroy(() => clearTimeout(pollTimer));

  async function loadModels(): Promise<void> {
    clearTimeout(pollTimer);
    try {
      const next = await catalog.backend.modelsStatus();
      const finished = models?.job.running && !next.job.running;
      models = next;
      if (finished) announce(next);
    } catch (e) {
      catalog.fail(e);
    }
    if (models?.job.running) pollTimer = setTimeout(() => void loadModels(), MODEL_POLL_MS);
  }

  function announce(m: ModelsDto): void {
    if (m.job.error) catalog.flash(`Model indirme durdu: ${m.job.error}`);
    else if (m.job.failed.length > 0) catalog.flash(`${m.job.failed.length} kaynak indirilemedi`);
    else if (m.ready) catalog.flash("Modeller indirildi ve doğrulandı");
  }

  async function installModels(): Promise<void> {
    try {
      models = await catalog.backend.modelsInstall();
    } catch (e) {
      catalog.fail(e);
    }
    void loadModels();
  }

  async function cancelModels(): Promise<void> {
    try {
      models = await catalog.backend.modelsCancel();
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

  {#if models}
    <section class="flex flex-col gap-2" aria-label="Modeller">
      <div class="flex items-end justify-between gap-3">
        <div class="flex min-w-0 flex-col">
          <h2 class="label">Modeller</h2>
          <span class="data truncate text-2xs text-ink-3" title={models.dir}>{models.dir}</span>
        </div>
        {#if models.job.running}
          <button class="btn" onclick={cancelModels}><X size={14} strokeWidth={1.5} />İptal</button>
        {:else if missing.length > 0}
          <button class="btn btn-primary" onclick={installModels}>
            <Download size={14} strokeWidth={1.5} />Modelleri indir{missingBytes > 0 ? ` · ${bytes(missingBytes)}` : ""}
          </button>
        {/if}
      </div>

      {#if models.job.running}
        <div class="flex flex-col gap-1" aria-live="polite">
          <div class="h-px w-full bg-line">
            <div class="h-px bg-amber" style:width={pct(models.job.done, models.job.total)}></div>
          </div>
          <div class="data flex justify-between text-2xs text-ink-2">
            <span class="truncate">{models.job.source ?? "…"} · {PHASE_NAMES[models.job.phase ?? ""] ?? "hazırlanıyor"}</span>
            <span>{models.job.total > 0 ? `${bytes(models.job.done)} / ${bytes(models.job.total)}` : ""}</span>
          </div>
        </div>
      {/if}

      <table class="data w-full border border-line text-xs">
        <tbody>
          {#each models.files as f (f.file)}
            <tr class="border-t border-line first:border-t-0">
              <td class="px-3 py-1.5 text-ink-2">{ROLE_NAMES[f.role] ?? f.role}</td>
              <td class="px-3 py-1.5 text-ink-3" title={f.sha256 ? `SHA-256 ${f.sha256}` : "özet Hugging Face'ten alınacak"}>
                {f.file}
              </td>
              <td class="px-3 py-1.5 text-right text-ink-3">{bytes(f.size)}</td>
              <td
                class="px-3 py-1.5 text-right {f.state === 'verified'
                  ? 'text-ink'
                  : f.state === 'corrupt'
                    ? 'text-amber'
                    : 'text-ink-3'}"
              >
                {STATE_NAMES[f.state]}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>

      {#each models.job.failed as f (f.source)}
        <p class="border border-amber px-3 py-2 text-xs text-amber">{f.files.join(", ")}: {f.reason}</p>
      {/each}
      {#if models.job.error && !models.job.running}
        <p class="text-xs text-amber">İndirme durdu: {models.job.error}</p>
      {/if}
      {#if !models.onnxBuild}
        <p class="text-xs text-ink-3">
          Bu derleme ONNX Runtime desteği olmadan yapıldı; modeller indirilse de kullanılmaz. <span class="data"
            >make build</span
          > ile derleyin.
        </p>
      {:else if models.restartNeeded}
        <p class="text-xs text-ink">Modeller hazır. Yüz tanıma ve anlamsal aramayı açmak için Loupe'u yeniden başlatın.</p>
      {/if}
      <p class="text-2xs text-ink-3">
        Her dosya indirildikten sonra SHA-256 ile doğrulanır; tutmayan dosya kullanılmaz. Yüz modelleri (InsightFace)
        yalnızca ticari olmayan kullanım içindir.
      </p>
    </section>
  {/if}
</div>

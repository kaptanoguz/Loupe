<script lang="ts" module>
  import type { ModelFileState } from "../../api/types";
  import { t } from "../../i18n";

  export const ROLE_NAMES: Record<string, string> = {
    face_detect: t("ai.faceDetect"),
    face_embed: t("ai.faceEmbed"),
    clip_image: t("ai.clipImage"),
    clip_text: t("ai.clipText"),
    tokenizer: t("ai.tokenizer"),
    runtime: "ONNX Runtime",
  };

  export const STATE_NAMES: Record<ModelFileState, string> = {
    verified: t("ai.verified"),
    missing: t("ai.missing"),
    corrupt: t("ai.corrupt"),
    unpinned: t("ai.unpinned"),
    incomplete: t("ai.incomplete"),
  };

  export const PHASE_NAMES: Record<string, string> = {
    resolve: t("ai.resolve"),
    download: t("ai.download"),
    verify: t("ai.verify"),
    extract: t("ai.extract"),
    done: t("ai.done"),
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
    { key: "quiet", name: t("ai.quiet"), hint: t("ai.quietHint") },
    { key: "balanced", name: t("ai.balanced"), hint: t("ai.balancedHint") },
    { key: "full", name: t("ai.full"), hint: t("ai.fullHint") },
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
      catalog.flash(local.paused ? t("app.aiPaused") : t("app.aiRunning"));
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
    if (m.job.error) catalog.flash(t("ai.downloadStopped", { error: m.job.error }));
    else if (m.job.failed.length > 0) catalog.flash(t("ai.sourcesFailed", { n: m.job.failed.length }));
    else if (m.ready) catalog.flash(t("ai.ready"));
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
        <span class="label">{t("ai.model")}</span>
        <span class="text-sm {ai.mode === 'off' ? 'text-amber' : 'text-ink'}">
          {ai.mode === "onnx" ? t("ai.onnx") : ai.mode === "mock" ? t("ai.mock") : t("ai.off")}
        </span>
        {#if ai.reason}<span class="text-xs text-ink-3">{ai.reason}</span>{/if}
        <span class="text-xs text-ink-3">
          {ai.semantic ? t("ai.semanticOn") : t("ai.semanticOff")}
        </span>
      </div>
      <button class="btn" onclick={togglePause} aria-pressed={ai.paused}>
        {#if ai.paused}<Play size={14} strokeWidth={1.5} />{t("slide.resume")}{:else}<Pause size={14} strokeWidth={1.5} />{t("slide.pause")}{/if}
      </button>
    </section>

    <section class="data grid grid-cols-3 border border-line text-xs" aria-label={t("ai.progress")}>
      <div class="px-3 py-2">
        <div class="label">{t("ai.faceScan")}</div>
        <div class="text-ink">{count(ai.facesDone)}/{count(ai.images)} · {pct(ai.facesDone, ai.images)}</div>
        <div class="text-2xs text-ink-3">{t("ai.queued", { n: count(ai.pending.faces) })}</div>
      </div>
      <div class="border-l border-line px-3 py-2">
        <div class="label">{t("ai.clipVector")}</div>
        <div class="text-ink">{count(ai.clipDone)}/{count(ai.images)} · {pct(ai.clipDone, ai.images)}</div>
        <div class="text-2xs text-ink-3">{t("ai.queued", { n: count(ai.pending.clip) })}</div>
      </div>
      <div class="border-l border-line px-3 py-2">
        <div class="label">{t("ai.clustering")}</div>
        <div class="text-ink">{ai.pending.clustering > 0 ? t("ai.waiting") : t("ai.upToDate")}</div>
        <div class="text-2xs text-ink-3">{t("ai.peopleEvents")}</div>
      </div>
    </section>

    <section>
      <h2 class="label pb-2">{t("ai.resources")}</h2>
      <div class="grid grid-cols-3 border border-line" role="radiogroup" aria-label={t("ai.resources")}>
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
    <p class="text-xs text-ink-3">{t("ai.loading")}</p>
  {/if}

  {#if models}
    <section class="flex flex-col gap-2" aria-label={t("ai.models")}>
      <div class="flex items-end justify-between gap-3">
        <div class="flex min-w-0 flex-col">
          <h2 class="label">{t("ai.models")}</h2>
          <span class="data truncate text-2xs text-ink-3" title={models.dir}>{models.dir}</span>
        </div>
        {#if models.job.running}
          <button class="btn" onclick={cancelModels}><X size={14} strokeWidth={1.5} />{t("ai.cancel")}</button>
        {:else if missing.length > 0}
          <button class="btn btn-primary" onclick={installModels}>
            <Download size={14} strokeWidth={1.5} />{t("ai.downloadModels")}{missingBytes > 0 ? ` · ${bytes(missingBytes)}` : ""}
          </button>
        {/if}
      </div>

      {#if models.job.running}
        <div class="flex flex-col gap-1" aria-live="polite">
          <div class="h-px w-full bg-line">
            <div class="h-px bg-amber" style:width={pct(models.job.done, models.job.total)}></div>
          </div>
          <div class="data flex justify-between text-2xs text-ink-2">
            <span class="truncate">{models.job.source ?? "…"} · {PHASE_NAMES[models.job.phase ?? ""] ?? t("ai.preparing")}</span>
            <span>{models.job.total > 0 ? `${bytes(models.job.done)} / ${bytes(models.job.total)}` : ""}</span>
          </div>
        </div>
      {/if}

      <table class="data w-full border border-line text-xs">
        <tbody>
          {#each models.files as f (f.file)}
            <tr class="border-t border-line first:border-t-0">
              <td class="px-3 py-1.5 text-ink-2">{ROLE_NAMES[f.role] ?? f.role}</td>
              <td class="px-3 py-1.5 text-ink-3" title={f.sha256 ? `SHA-256 ${f.sha256}` : t("ai.hashFromHf")}>
                {f.file}
              </td>
              <td class="px-3 py-1.5 text-right text-ink-3">{bytes(f.size)}</td>
              <td
                class="px-3 py-1.5 text-right {f.state === 'verified'
                  ? 'text-ink'
                  : f.state === 'corrupt' || f.state === 'incomplete'
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
        <p class="text-xs text-amber">{t("ai.downloadStopped", { error: models.job.error })}</p>
      {/if}
      {#if !models.onnxBuild}
        <p class="text-xs text-ink-3">
          {@html t("ai.noOnnx")}
        </p>
      {:else if models.restartNeeded}
        <p class="text-xs text-ink">{t("ai.restart")}</p>
      {/if}
      <p class="text-2xs text-ink-3">
        {t("ai.license")}
      </p>
    </section>
  {/if}
</div>

<script lang="ts">
  import ShieldCheck from "@lucide/svelte/icons/shield-check";
  import { onDestroy, onMount } from "svelte";

  import type { VerifyRunDto } from "../../api/types";
  import { bytes, clock, count, dateTimeMs } from "../../format";
  import type { Catalog } from "../../state/catalog.svelte";

  interface Props {
    catalog: Catalog;
  }

  let { catalog }: Props = $props();

  /** Çalışan doğrulama varken rapor bu aralıkla yenilenir (ms). */
  const POLL_MS = 1000;
  const STATE: Record<VerifyRunDto["state"], string> = {
    running: "sürüyor",
    paused: "duraklatıldı",
    done: "bitti",
    cancelled: "durduruldu",
    failed: "yarıda kaldı",
  };

  let runs = $state.raw<VerifyRunDto[]>([]);
  let timer: ReturnType<typeof setInterval> | undefined;

  const running = $derived(runs.some((r) => r.state === "running") || (catalog.status?.verifying ?? 0) > 0);
  const last = $derived(runs[0]);

  onMount(() => {
    void load();
    timer = setInterval(() => {
      if (running) void load();
    }, POLL_MS);
  });

  onDestroy(() => clearInterval(timer));

  async function load(): Promise<void> {
    try {
      runs = await catalog.backend.verifyRuns(10);
    } catch (e) {
      catalog.fail(e);
    }
  }

  async function start(): Promise<void> {
    try {
      const started = await catalog.backend.verifyLibrary();
      catalog.flash(started.length ? `Doğrulama başladı · ${started.length} kütüphane` : "Doğrulanacak çevrimiçi kütüphane yok");
      await load();
      void catalog.refreshStatus();
    } catch (e) {
      catalog.fail(e);
    }
  }

  async function stop(): Promise<void> {
    try {
      await catalog.backend.cancelVerify();
      await load();
    } catch (e) {
      catalog.fail(e);
    }
  }

  function problems(r: VerifyRunDto): number {
    return r.missing + r.mismatched + r.unreadable;
  }
</script>

<div class="flex flex-col gap-4 p-4">
  <section class="flex items-start gap-4">
    <p class="flex-1 text-sm text-ink-2">
      Her dosyanın BLAKE3 özeti diskteki içerikle karşılaştırılır. Uyuşmayan dosya karantinaya alınır, kopyası buluttan
      geri yüklenebilir. Disk taramanın ortasında çıkarılırsa hiçbir dosya “kayıp” sayılmaz.
    </p>
    {#if running}
      <button class="btn" onclick={stop}>Durdur</button>
    {:else}
      <button class="btn btn-primary" onclick={start}><ShieldCheck size={14} strokeWidth={1.5} />Kütüphaneyi Doğrula</button>
    {/if}
  </section>

  {#if last}
    <section class="border border-line" aria-label="Son doğrulama raporu">
      <div class="flex items-baseline justify-between border-b border-line px-3 py-2">
        <span class="text-sm text-ink">{last.library}</span>
        <span class="data text-2xs {last.state === 'failed' ? 'text-amber' : 'text-ink-3'}">
          {STATE[last.state]} · {dateTimeMs(last.startedAt)}{last.finishedAt
            ? ` · ${clock((last.finishedAt - last.startedAt) / 1000)}`
            : ""}
        </span>
      </div>
      {#if last.state === "running" && last.total > 0}
        <div class="h-px w-full bg-line"><div class="h-px bg-amber" style:width="{(100 * last.checked) / last.total}%"></div></div>
      {/if}
      <div class="data grid grid-cols-5 text-xs">
        <div class="px-3 py-2"><div class="label">Denetlenen</div><div class="text-ink">{count(last.checked)}/{count(last.total)}</div></div>
        <div class="border-l border-line px-3 py-2"><div class="label">Sağlam</div><div class="text-ink">{count(last.ok)}</div></div>
        <div class="border-l border-line px-3 py-2">
          <div class="label">Kayıp</div><div class={last.missing ? "text-amber" : "text-ink"}>{count(last.missing)}</div>
        </div>
        <div class="border-l border-line px-3 py-2">
          <div class="label">Bozuk</div><div class={last.mismatched ? "text-amber" : "text-ink"}>{count(last.mismatched)}</div>
        </div>
        <div class="border-l border-line px-3 py-2">
          <div class="label">Okunamayan</div><div class={last.unreadable ? "text-amber" : "text-ink"}>{count(last.unreadable)}</div>
        </div>
      </div>
      <div class="data border-t border-line px-3 py-1 text-2xs text-ink-3">{bytes(last.bytesRead)} okundu</div>
    </section>
  {:else}
    <p class="text-xs text-ink-3">Henüz doğrulama yapılmadı.</p>
  {/if}

  {#if runs.length > 1}
    <section>
      <h2 class="label pb-2">Önceki çalıştırmalar</h2>
      <table class="data w-full text-2xs">
        <tbody>
          {#each runs.slice(1) as r (r.id)}
            <tr class="border-t border-line text-ink-2">
              <td class="py-1">{dateTimeMs(r.startedAt)}</td>
              <td>{r.library}</td>
              <td>{STATE[r.state]}</td>
              <td class="text-right">{count(r.checked)}</td>
              <td class="text-right {problems(r) ? 'text-amber' : ''}">{count(problems(r))} sorun</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </section>
  {/if}
</div>

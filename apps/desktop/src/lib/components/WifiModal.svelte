<script lang="ts" module>
  import type { ConflictPolicy } from "../api/types";

  export const POLICIES: { key: ConflictPolicy; name: string; hint: string }[] = [
    { key: "keep_both", name: "İkisini de tut", hint: "Telefondaki düzenleme yeni sürüm olarak eklenir" },
    { key: "replace_with_newer", name: "Yeniyle değiştir", hint: "Daha yeni olan katalogdaki sürümün yerine geçer" },
    { key: "skip", name: "Atla", hint: "Önceden aktarılmış dosyanın düzenlemesi alınmaz" },
    { key: "ask_user", name: "Sor", hint: "Çakışmalar onay için bekletilir" },
  ];

  /** Cihaz listesinin yoklama aralığı (ms): yeni eşleşme ve son görülme. */
  export const DEVICE_POLL_MS = 2000;
  /** Geri sayımın yenilenme aralığı (ms). */
  export const TICK_MS = 250;
</script>

<script lang="ts">
  import RefreshCw from "@lucide/svelte/icons/refresh-cw";
  import Smartphone from "@lucide/svelte/icons/smartphone";
  import X from "@lucide/svelte/icons/x";
  import { onDestroy, onMount } from "svelte";

  import type { DeviceDto, PairingQrDto } from "../api/types";
  import { bytes, clock, count, dateTimeMs, rate } from "../format";
  import type { Catalog } from "../state/catalog.svelte";

  interface Props {
    catalog: Catalog;
  }

  let { catalog }: Props = $props();

  let qr = $state.raw<PairingQrDto | null>(null);
  let qrError = $state<string | null>(null);
  let devices = $state.raw<DeviceDto[]>([]);
  let now = $state(Date.now());
  let confirmRevoke = $state<number | null>(null);
  let loading = false;
  let tickTimer: ReturnType<typeof setInterval> | undefined;
  let pollTimer: ReturnType<typeof setInterval> | undefined;

  const remaining = $derived(qr ? Math.max(0, Math.ceil((qr.expiresAtMs - now) / 1000)) : 0);
  const lan = $derived(catalog.status?.lan);
  const active = $derived(devices.filter((d) => !d.revoked));

  onMount(() => {
    void refresh();
    void loadDevices(false);
    tickTimer = setInterval(() => {
      now = Date.now();
      if (qr && remaining === 0) void refresh();
    }, TICK_MS);
    pollTimer = setInterval(() => void loadDevices(true), DEVICE_POLL_MS);
  });

  onDestroy(() => {
    clearInterval(tickTimer);
    clearInterval(pollTimer);
    // Ekranda olmayan belirteç geçersiz kılınır.
    void catalog.backend.cancelPairing().catch(() => undefined);
  });

  async function refresh(): Promise<void> {
    if (loading) return;
    loading = true;
    try {
      qr = await catalog.backend.pairingQr();
      qrError = null;
      now = Date.now();
    } catch (e) {
      qr = null;
      qrError = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  async function loadDevices(announce: boolean): Promise<void> {
    try {
      const next = await catalog.backend.listDevices();
      const known = new Set(devices.map((d) => d.id));
      const fresh = next.filter((d) => !known.has(d.id) && !d.revoked);
      devices = next;
      if (announce && fresh.length > 0) {
        catalog.flash(`Eşleşti: ${fresh.map((d) => d.name).join(", ")}`);
        // Belirteç tek kullanımlık: yenisi gösterilir.
        void refresh();
      }
    } catch (e) {
      catalog.fail(e);
    }
  }

  async function revoke(id: number): Promise<void> {
    confirmRevoke = null;
    try {
      devices = await catalog.backend.revokeDevice(id);
      catalog.flash("Cihazın erişimi kaldırıldı");
    } catch (e) {
      catalog.fail(e);
    }
  }

  async function setPolicy(id: number, policy: ConflictPolicy): Promise<void> {
    try {
      devices = await catalog.backend.setConflictPolicy(id, policy);
    } catch (e) {
      catalog.fail(e);
    }
  }

  function close(): void {
    catalog.overlay = null;
  }

  function onscrim(e: MouseEvent): void {
    if (e.target === e.currentTarget) close();
  }
</script>

<div class="scrim fixed inset-0 z-30 flex items-center justify-center" role="presentation" onmousedown={onscrim}>
  <div
    class="flex max-h-[calc(100vh-64px)] w-[760px] max-w-[calc(100vw-32px)] flex-col rounded-sm border border-line bg-surface"
    role="dialog"
    aria-label="Wi-Fi telefon eşleştirme"
    aria-modal="true"
  >
    <header class="flex h-toolbar shrink-0 items-center gap-3 border-b border-line px-4">
      <h1 class="text-sm font-medium text-ink">Wi-Fi Telefon Eşleştir</h1>
      <span class="flex-1"></span>
      <button class="btn btn-ghost px-2" onclick={close} aria-label="Kapat"><X size={14} strokeWidth={1.5} /></button>
    </header>

    <div class="grid min-h-0 flex-1 grid-cols-[256px_1fr] overflow-hidden">
      <div class="flex flex-col gap-3 border-r border-line p-4">
        <div class="aspect-square w-full bg-ink p-0 [&>svg]:block [&>svg]:h-full [&>svg]:w-full" aria-live="polite">
          {#if qr}
            <!-- Çekirdekten gelen SVG yalnızca <rect> ve <path> içerir (kullanıcı girdisi yok). -->
            {@html qr.svg}
          {:else if qrError}
            <p class="p-3 text-xs text-canvas">{qrError}</p>
          {/if}
        </div>
        <div class="flex items-center justify-between">
          <span class="font-mono text-xl text-ink tabular-nums {remaining <= 10 ? 'text-amber' : ''}" aria-label="Kalan süre">
            {clock(remaining)}
          </span>
          <button class="btn" onclick={refresh} aria-label="Yeni kod"><RefreshCw size={14} strokeWidth={1.5} />Yeni kod</button>
        </div>
        <div class="h-px w-full bg-line">
          <div class="h-px bg-amber" style:width="{qr ? (100 * remaining) / Math.max(1, qr.ttlSecs) : 0}%"></div>
        </div>
        <p class="text-xs text-ink-2">
          Telefondaki Loupe uygulamasıyla tarayın. Kod tek kullanımlıktır ve {qr?.ttlSecs ?? 60} saniyede yenilenir.
        </p>
        {#if qr}
          <dl class="data text-2xs text-ink-3">
            <dt class="label">Adres</dt>
            {#each qr.addrs as a (a)}<dd>{a}:{qr.port}</dd>{/each}
            <dt class="label pt-1">Ad</dt>
            <dd>{qr.name}</dd>
          </dl>
        {/if}
      </div>

      <div class="flex min-h-0 flex-col">
        <div class="data grid shrink-0 grid-cols-4 border-b border-line text-xs" aria-label="Aktarım">
          <div class="border-r border-line px-3 py-2">
            <div class="label">Hız</div>
            <div class="text-ink">{rate(lan?.bytesPerSec)}</div>
          </div>
          <div class="border-r border-line px-3 py-2">
            <div class="label">Kalan</div>
            <div class="text-ink">{lan?.etaSecs != null ? clock(lan.etaSecs) : "—"}</div>
          </div>
          <div class="border-r border-line px-3 py-2">
            <div class="label">Doğrulandı</div>
            <div class="text-ink">{count(lan?.verifiedFiles ?? 0)}</div>
          </div>
          <div class="px-3 py-2">
            <div class="label">Çakışma</div>
            <div class={lan && lan.conflicts > 0 ? "text-amber" : "text-ink"}>{count(lan?.conflicts ?? 0)}</div>
          </div>
        </div>
        {#if lan && lan.bytesExpected > 0}
          <div class="data border-b border-line px-3 py-2 text-2xs text-ink-3">
            {bytes(lan.bytesReceived)} / {bytes(lan.bytesExpected)} · {count(lan.skipped)} atlandı · {count(lan.failed)} hata
          </div>
        {/if}

        <h2 class="label flex shrink-0 justify-between px-3 pt-3 pb-2">
          <span>Eşleşmiş cihazlar</span><span class="data">{active.length}</span>
        </h2>
        <ul class="min-h-0 flex-1 overflow-y-auto">
          {#each devices as d (d.id)}
            <li class="flex flex-col gap-2 border-t border-line px-3 py-2 {d.revoked ? 'text-ink-3' : ''}">
              <div class="flex items-center gap-2">
                <Smartphone size={14} strokeWidth={1.5} class="shrink-0 text-ink-3" />
                <span class="min-w-0 flex-1 truncate text-sm {d.revoked ? '' : 'text-ink'}">{d.name}</span>
                <span class="data text-2xs text-ink-3">{d.platform}</span>
              </div>
              <div class="data flex justify-between text-2xs text-ink-3">
                <span>{d.revoked ? "erişim kaldırıldı" : `son ${dateTimeMs(d.lastSeenAt)}`}{d.lastAddress ? ` · ${d.lastAddress}` : ""}</span>
                <span>{count(d.received)} dosya</span>
              </div>
              {#if !d.revoked}
                <div class="flex items-center gap-2">
                  <select
                    class="input h-6 min-w-0 flex-1 text-xs"
                    value={d.conflictPolicy}
                    aria-label="{d.name} çakışma politikası"
                    title={POLICIES.find((p) => p.key === d.conflictPolicy)?.hint}
                    onchange={(e) => setPolicy(d.id, (e.currentTarget as HTMLSelectElement).value as ConflictPolicy)}
                  >
                    {#each POLICIES as p (p.key)}<option value={p.key}>{p.name}</option>{/each}
                  </select>
                  {#if confirmRevoke === d.id}
                    <button class="btn h-6 border-amber text-amber" onclick={() => revoke(d.id)}>Kaldır</button>
                    <button class="btn btn-ghost h-6" onclick={() => (confirmRevoke = null)}>Vazgeç</button>
                  {:else}
                    <button class="btn h-6" onclick={() => (confirmRevoke = d.id)}>Erişimi kaldır</button>
                  {/if}
                </div>
              {/if}
            </li>
          {:else}
            <li class="px-3 py-2 text-xs text-ink-3">Henüz eşleşmiş cihaz yok.</li>
          {/each}
        </ul>
      </div>
    </div>
  </div>
</div>

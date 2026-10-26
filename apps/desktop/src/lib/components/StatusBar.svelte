<script lang="ts">
  import { bytes, clock, count, rate } from "../format";
  import { t } from "../i18n";
  import type { Catalog } from "../state/catalog.svelte";

  interface Props {
    catalog: Catalog;
  }

  let { catalog }: Props = $props();

  const st = $derived(catalog.status);
  const queue = $derived(st ? st.queue.pending + st.queue.running : 0);
  const selection = $derived.by(() => {
    if (catalog.selected < 0) return t("status.noSelection");
    const d = catalog.detail;
    const pos = `${count(catalog.selected + 1)}/${count(catalog.total)}`;
    if (!d || d.id !== catalog.selectedItem?.id) return pos;
    return [pos, d.fileName, d.aperture, d.exposure, d.iso ? `ISO ${d.iso}` : null].filter(Boolean).join("  ");
  });
  const lan = $derived.by(() => {
    if (!st) return null;
    const l = st.lan;
    switch (l.state) {
      case "listening":
        if (l.activeUploads > 0) {
          const eta = l.etaSecs !== null ? ` · ${clock(l.etaSecs)}` : "";
          return `Wi-Fi ↓${l.activeUploads} ${rate(l.bytesPerSec)}${eta} · ✓${count(l.verifiedFiles)}`;
        }
        return `Wi-Fi :${l.port ?? "?"}${l.verifiedFiles > 0 ? ` · ✓${count(l.verifiedFiles)}` : ""}`;
      case "error":
        return t("status.wifiError");
      default:
        return null;
    }
  });
  const ai = $derived.by(() => {
    const a = st?.ai;
    if (!a || a.mode === "off") return null;
    const pending = a.pending.faces + a.pending.clip + a.pending.clustering;
    if (a.paused) return { text: t("status.aiPaused"), warn: true };
    return pending > 0 ? { text: `AI ${count(pending)}`, warn: false } : null;
  });
  const uploading = $derived(st?.cloud.filter((c) => c.state === "uploading") ?? []);
</script>

<footer class="statusbar flex shrink-0 items-center gap-4" aria-label={t("status.label")}>
  <span>{t("status.photos", { n: count(st?.totalMedia ?? catalog.total) })}</span>
  <span class={st && (st.queue.failed > 0 || st.queue.dead > 0) ? "text-amber" : ""}>
    {t("status.jobs", { n: count(queue) })}{st && st.queue.running > 0 ? t("status.running", { n: st.queue.running }) : ""}{st &&
    st.queue.dead > 0
      ? t("status.failed", { n: st.queue.dead })
      : ""}
  </span>
  <span class={st && st.librariesOffline > 0 ? "text-ink-3" : ""}>
    {t("status.diskFree", { size: bytes(st?.freeBytes) })}{st && st.librariesOffline > 0 ? t("status.offline", { n: st.librariesOffline }) : ""}
  </span>
  {#if st && st.importsActive > 0}<span class="text-amber">{t("status.imports", { n: st.importsActive })}</span>{/if}
  {#if lan}<span class={st && st.lan.activeUploads > 0 ? "text-ink" : ""}>{lan}</span>{/if}
  {#if uploading.length > 0}
    <span>{t("status.cloud")} ↑{count(uploading.reduce((n, c) => n + c.pending, 0))}</span>
  {/if}
  {#if st && st.verifying > 0}<span class="text-amber">{t("status.verifying")}</span>{/if}
  {#if ai}
    <button
      class={ai.warn ? "text-amber" : ""}
      onclick={() => catalog.openSettings("ai")}
      title={t("status.aiTitle")}>{ai.text}</button
    >
  {/if}
  <span class="min-w-0 flex-1 truncate text-right text-ink">
    {#if catalog.error}<span class="text-amber" title={catalog.error}>{catalog.error}</span>{:else if catalog.notice}<span
        class="text-ink"
        role="status">{catalog.notice}</span
      >{:else}{selection}{/if}
  </span>
  <span class="text-ink-3" title={t("status.layoutTitle")}>{catalog.layoutMs.toFixed(1)} ms</span>
</footer>

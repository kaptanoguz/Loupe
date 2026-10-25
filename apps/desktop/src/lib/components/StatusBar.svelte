<script lang="ts">
  import { bytes, count } from "../format";
  import type { Catalog } from "../state/catalog.svelte";

  interface Props {
    catalog: Catalog;
  }

  let { catalog }: Props = $props();

  const st = $derived(catalog.status);
  const queue = $derived(st ? st.queue.pending + st.queue.running : 0);
  const selection = $derived.by(() => {
    if (catalog.selected < 0) return "seçim yok";
    const d = catalog.detail;
    const pos = `${count(catalog.selected + 1)}/${count(catalog.total)}`;
    if (!d || d.id !== catalog.selectedItem?.id) return pos;
    return [pos, d.fileName, d.aperture, d.exposure, d.iso ? `ISO ${d.iso}` : null].filter(Boolean).join("  ");
  });
  const lan = $derived.by(() => {
    if (!st) return null;
    switch (st.lan.state) {
      case "listening":
        return `LAN :${st.lan.port ?? "?"}${st.lan.activeUploads > 0 ? ` ↑${st.lan.activeUploads}` : ""}`;
      case "error":
        return "LAN hata";
      default:
        return null;
    }
  });
</script>

<footer class="statusbar flex shrink-0 items-center gap-4" aria-label="Durum">
  <span>{count(st?.totalMedia ?? catalog.total)} fotoğraf</span>
  <span class={st && (st.queue.failed > 0 || st.queue.dead > 0) ? "text-amber" : ""}>
    iş {count(queue)}{st && st.queue.running > 0 ? ` (${st.queue.running} çalışıyor)` : ""}{st && st.queue.dead > 0
      ? ` · ${st.queue.dead} başarısız`
      : ""}
  </span>
  <span class={st && st.librariesOffline > 0 ? "text-ink-3" : ""}>
    disk {bytes(st?.freeBytes)} boş{st && st.librariesOffline > 0 ? ` · ${st.librariesOffline} çevrimdışı` : ""}
  </span>
  {#if st && st.importsActive > 0}<span class="text-amber">içe aktarma {st.importsActive}</span>{/if}
  {#if lan}<span>{lan}</span>{/if}
  <span class="min-w-0 flex-1 truncate text-right text-ink">
    {#if catalog.error}<span class="text-amber" title={catalog.error}>{catalog.error}</span>{:else}{selection}{/if}
  </span>
  <span class="text-ink-3" title="Son düzen hesabı">{catalog.layoutMs.toFixed(1)} ms</span>
</footer>

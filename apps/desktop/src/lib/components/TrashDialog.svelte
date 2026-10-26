<script lang="ts">
  import Trash from "@lucide/svelte/icons/trash";
  import { onMount } from "svelte";

  import { count } from "../format";
  import type { Catalog } from "../state/catalog.svelte";

  interface Props {
    catalog: Catalog;
    ids: number[];
    /** Onaylandı: çağıran siler, görünümü yeniler. */
    onConfirm: (ids: number[]) => Promise<void>;
  }

  let { catalog, ids, onConfirm }: Props = $props();
  let busy = $state(false);
  let cancelButton: HTMLButtonElement | undefined = $state();

  // Varsayılan düğme "Vazgeç": Enter yanlışlıkla silmez.
  onMount(() => cancelButton?.focus());

  function close(): void {
    if (!busy) catalog.overlay = null;
  }

  async function confirm(): Promise<void> {
    busy = true;
    try {
      await onConfirm(ids);
    } finally {
      busy = false;
      catalog.overlay = null;
    }
  }

  function onscrim(e: MouseEvent): void {
    if (e.target === e.currentTarget) close();
  }
</script>

<div class="scrim fixed inset-0 z-30 flex items-center justify-center" role="presentation" onmousedown={onscrim}>
  <div
    class="flex w-[420px] max-w-[calc(100vw-32px)] flex-col gap-3 rounded-sm border border-line bg-surface p-4"
    role="alertdialog"
    aria-label="Diskten sil"
    aria-modal="true"
  >
    <h1 class="flex items-center gap-2 text-sm font-medium text-ink">
      <Trash size={14} strokeWidth={1.5} />{count(ids.length)} fotoğraf diskten silinsin mi?
    </h1>
    <p class="text-sm text-ink-2">
      Dosyalar Çöp Kutusu'na taşınır ve Loupe'den çıkar. Fikrinizi değiştirirseniz dosya yöneticisinin Çöp Kutusu'ndan
      geri alabilirsiniz; geri aldığınız klasörü yeniden içe aktarınca fotoğraflar kataloğa döner.
    </p>
    <div class="flex justify-end gap-2">
      <button class="btn" bind:this={cancelButton} disabled={busy} onclick={close}>Vazgeç</button>
      <button class="btn btn-primary" disabled={busy} onclick={confirm}>
        {busy ? "Taşınıyor…" : "Çöp Kutusu'na taşı"}
      </button>
    </div>
  </div>
</div>

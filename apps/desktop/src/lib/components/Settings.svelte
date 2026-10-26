<script lang="ts">
  import X from "@lucide/svelte/icons/x";

  import type { Catalog, SettingsTab } from "../state/catalog.svelte";
  import AiSettings from "./settings/AiSettings.svelte";
  import CloudSettings from "./settings/CloudSettings.svelte";
  import LibrarySettings from "./settings/LibrarySettings.svelte";
  import PhoneSettings from "./settings/PhoneSettings.svelte";
  import RecoveryKey from "./settings/RecoveryKey.svelte";
  import VerifySettings from "./settings/VerifySettings.svelte";

  interface Props {
    catalog: Catalog;
  }

  let { catalog }: Props = $props();

  const TABS: { key: SettingsTab; name: string }[] = [
    { key: "libraries", name: "Kütüphaneler" },
    { key: "phone", name: "Telefon" },
    { key: "cloud", name: "Bulut" },
    { key: "encryption", name: "Şifreleme" },
    { key: "verify", name: "Doğrulama" },
    { key: "ai", name: "Yapay zekâ" },
  ];

  function close(): void {
    catalog.overlay = null;
  }

  function onscrim(e: MouseEvent): void {
    if (e.target === e.currentTarget) close();
  }
</script>

<div class="scrim fixed inset-0 z-30 flex items-center justify-center" role="presentation" onmousedown={onscrim}>
  <div
    class="flex h-[min(640px,calc(100vh-64px))] w-[820px] max-w-[calc(100vw-32px)] flex-col rounded-sm border border-line bg-surface"
    role="dialog"
    aria-label="Ayarlar"
    aria-modal="true"
  >
    <header class="flex h-toolbar shrink-0 items-center gap-1 border-b border-line px-4">
      <h1 class="mr-4 text-sm font-medium text-ink">Ayarlar</h1>
      <div class="flex h-full" role="tablist" aria-label="Ayar bölümleri">
        {#each TABS as t (t.key)}
          <button
            class="h-full border-b px-3 text-sm {catalog.settingsTab === t.key
              ? 'border-amber text-ink'
              : 'border-transparent text-ink-3 hover:text-ink-2'}"
            role="tab"
            aria-selected={catalog.settingsTab === t.key}
            onclick={() => (catalog.settingsTab = t.key)}>{t.name}</button
          >
        {/each}
      </div>
      <span class="flex-1"></span>
      <button class="btn btn-ghost px-2" onclick={close} aria-label="Kapat"><X size={14} strokeWidth={1.5} /></button>
    </header>
    <div class="min-h-0 flex-1 overflow-y-auto" role="tabpanel">
      {#if catalog.settingsTab === "libraries"}
        <LibrarySettings {catalog} />
      {:else if catalog.settingsTab === "phone"}
        <PhoneSettings {catalog} />
      {:else if catalog.settingsTab === "cloud"}
        <CloudSettings {catalog} />
      {:else if catalog.settingsTab === "encryption"}
        <RecoveryKey {catalog} />
      {:else if catalog.settingsTab === "verify"}
        <VerifySettings {catalog} />
      {:else}
        <AiSettings {catalog} />
      {/if}
    </div>
  </div>
</div>

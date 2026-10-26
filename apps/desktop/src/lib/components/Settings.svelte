<script lang="ts">
  import X from "@lucide/svelte/icons/x";

  import { LANGS, lang, setLang, t } from "../i18n";
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
    { key: "libraries", name: t("settings.libraries") },
    { key: "phone", name: t("settings.phone") },
    { key: "cloud", name: t("settings.cloud") },
    { key: "encryption", name: t("settings.encryption") },
    { key: "verify", name: t("settings.verify") },
    { key: "ai", name: t("settings.ai") },
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
    aria-label={t("settings.title")}
    aria-modal="true"
  >
    <header class="flex h-toolbar shrink-0 items-center gap-1 border-b border-line px-4">
      <h1 class="mr-4 text-sm font-medium text-ink">{t("settings.title")}</h1>
      <div class="flex h-full" role="tablist" aria-label={t("settings.sections")}>
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
      <div class="mr-2 flex items-center gap-1" role="radiogroup" aria-label={t("settings.language")}>
        {#each LANGS as l (l.key)}
          <button
            class="btn px-2 text-xs {lang() === l.key ? 'btn-primary' : 'btn-ghost'}"
            role="radio"
            aria-checked={lang() === l.key}
            onclick={() => lang() !== l.key && setLang(l.key)}>{l.name}</button
          >
        {/each}
      </div>
      <button class="btn btn-ghost px-2" onclick={close} aria-label={t("common.close")}><X size={14} strokeWidth={1.5} /></button>
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

<script lang="ts">
  import Smartphone from "@lucide/svelte/icons/smartphone";
  import Wifi from "@lucide/svelte/icons/wifi";

  import { t } from "../../i18n";
  import type { Catalog } from "../../state/catalog.svelte";

  interface Props {
    catalog: Catalog;
  }

  let { catalog }: Props = $props();

  const lan = $derived(catalog.status?.lan);
  const target = $derived(catalog.sidebar?.libraries.find((l) => l.primary));
  const STATE = { off: t("phone.off"), listening: t("phone.on"), no_library: t("phone.noLibrary"), error: t("sync.error") } as const;
</script>

<div class="flex flex-col gap-6 p-4">
  <section class="flex flex-col gap-2">
    <h2 class="label">{t("phone.wifiTitle")}</h2>
    <dl class="kv data text-2xs">
      <dt>{t("phone.server")}</dt>
      <dd class={lan?.state === "listening" ? "" : "text-amber"}>
        {lan ? STATE[lan.state] : "—"}{lan?.port ? ` · ${t("phone.port", { n: lan.port })}` : ""}
      </dd>
      <dt>{t("phone.target")}</dt>
      <dd class="break-all">{target ? `${target.name} · ${target.root ?? t("sidebar.notMounted")}` : t("phone.noPrimary")}</dd>
    </dl>
    <p class="text-xs text-ink-2">
      {t("phone.wifiHint")}
    </p>
    <div>
      <button class="btn" onclick={() => (catalog.overlay = "wifi")}
        ><Wifi size={14} strokeWidth={1.5} />{t("phone.qrButton")}</button
      >
    </div>
  </section>

  <section class="flex flex-col gap-2">
    <h2 class="label">{t("phone.shortcutTitle")}</h2>
    <p class="text-xs text-ink-2">
      {@html t("phone.shortcutHint")}
    </p>
  </section>

  <section class="flex flex-col gap-2">
    <h2 class="label">{t("phone.cableTitle")}</h2>
    <p class="text-xs text-ink-2">
      {t("phone.cable1")}
      <Smartphone size={12} strokeWidth={1.5} class="inline" />
      {@html t("phone.cable2")}
    </p>
  </section>
</div>

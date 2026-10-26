<script lang="ts" module>
  import type { CloudProvider } from "../../api/types";
  import { t } from "../../i18n";

  export interface ProviderSpec {
    key: CloudProvider;
    name: string;
    /** Gizli olmayan ayar alanları (config anahtarı, etiket, yer tutucu). */
    fields: { key: string; label: string; placeholder: string }[];
    /** Kullanıcı adı / erişim anahtarı alanının etiketi; `null`: kimlik bilgisi yok. */
    user: string | null;
    secret: string | null;
  }

  const S3_FIELDS = (endpoint: string, region: string) => [
    { key: "endpoint", label: t("cloud.endpoint"), placeholder: endpoint },
    { key: "bucket", label: t("cloud.bucket"), placeholder: t("cloud.bucketPlaceholder") },
    { key: "region", label: t("cloud.region"), placeholder: region },
  ];

  export const PROVIDERS: ProviderSpec[] = [
    { key: "s3", name: "Amazon S3", fields: S3_FIELDS("https://s3.eu-central-1.amazonaws.com", "eu-central-1"), user: t("cloud.accessKey"), secret: t("cloud.secretKey") },
    { key: "b2", name: "Backblaze B2", fields: S3_FIELDS("https://s3.eu-central-003.backblazeb2.com", "eu-central-003"), user: "keyID", secret: "applicationKey" },
    { key: "r2", name: "Cloudflare R2", fields: S3_FIELDS(t("cloud.r2Endpoint"), "auto"), user: t("cloud.accessKey"), secret: t("cloud.secretKey") },
    { key: "wasabi", name: "Wasabi", fields: S3_FIELDS("https://s3.eu-central-1.wasabisys.com", "eu-central-1"), user: t("cloud.accessKey"), secret: t("cloud.secretKey") },
    { key: "webdav", name: "WebDAV", fields: [{ key: "url", label: t("wifi.address"), placeholder: "https://cloud.example.org/remote.php/dav/files/me/" }], user: t("cloud.user"), secret: t("cloud.password") },
    { key: "local", name: t("cloud.localFolder"), fields: [{ key: "path", label: t("sidebar.folder"), placeholder: t("cloud.localPlaceholder") }], user: null, secret: null },
  ];

  export function providerSpec(key: CloudProvider): ProviderSpec {
    return PROVIDERS.find((p) => p.key === key) ?? (PROVIDERS[0] as ProviderSpec);
  }

  /** "4", "4,5", "" → 4 / 4.5 / null (sınırsız). Geçersizse `undefined`. */
  export function parseMbps(s: string): number | null | undefined {
    const t = s.trim().replace(",", ".");
    if (!t) return null;
    const n = Number(t);
    return Number.isFinite(n) && n > 0 ? n : undefined;
  }
</script>

<script lang="ts">
  import CloudUpload from "@lucide/svelte/icons/cloud-upload";
  import Pencil from "@lucide/svelte/icons/pencil";
  import Trash from "@lucide/svelte/icons/trash";
  import { onMount } from "svelte";

  import type { CloudSettingsDto, CloudTargetDto, CloudTargetInput } from "../../api/types";
  import { count } from "../../format";
  import type { Catalog } from "../../state/catalog.svelte";

  interface Props {
    catalog: Catalog;
  }

  let { catalog }: Props = $props();

  interface Form {
    id: number | null;
    name: string;
    provider: CloudProvider;
    config: Record<string, string>;
    user: string;
    secret: string;
    keyPrefix: string;
    encrypted: boolean;
    bandwidth: string;
    enabled: boolean;
    /** Düzenlenen hedefte kayıtlı kimlik bilgisi var (boş alan: değişmez). */
    hasCredentials: boolean;
    wasEncrypted: boolean;
  }

  let settings = $state.raw<CloudSettingsDto | null>(null);
  let globalBw = $state("");
  let form = $state<Form | null>(null);
  let confirmDelete = $state<number | null>(null);
  let saving = $state(false);

  const spec = $derived(form ? providerSpec(form.provider) : null);
  const statusOf = $derived(new Map((catalog.status?.cloud ?? []).map((c) => [c.id, c])));

  onMount(() => void load());

  async function load(): Promise<void> {
    try {
      apply(await catalog.backend.cloudSettings());
    } catch (e) {
      catalog.fail(e);
    }
  }

  function apply(s: CloudSettingsDto): void {
    settings = s;
    globalBw = s.globalBandwidthMbps === null ? "" : String(s.globalBandwidthMbps);
  }

  function blank(): Form {
    return {
      id: null,
      name: "",
      provider: "b2",
      config: {},
      user: "",
      secret: "",
      keyPrefix: "loupe/",
      encrypted: true,
      bandwidth: "",
      enabled: true,
      hasCredentials: false,
      wasEncrypted: false,
    };
  }

  function edit(t: CloudTargetDto): void {
    const config: Record<string, string> = {};
    for (const [k, v] of Object.entries(t.config)) if (typeof v === "string") config[k] = v;
    form = {
      id: t.id,
      name: t.name,
      provider: t.provider,
      config,
      user: t.user ?? "",
      secret: "",
      keyPrefix: t.keyPrefix,
      encrypted: t.encrypted,
      bandwidth: t.bandwidthMbps === null ? "" : String(t.bandwidthMbps),
      enabled: t.enabled,
      hasCredentials: t.hasCredentials,
      wasEncrypted: t.encrypted,
    };
  }

  async function save(): Promise<void> {
    const f = form;
    if (!f || !spec) return;
    const bw = parseMbps(f.bandwidth);
    if (bw === undefined) {
      catalog.error = t("cloud.bwInvalid");
      return;
    }
    const config: Record<string, string> = {};
    for (const field of spec.fields) {
      const v = (f.config[field.key] ?? "").trim();
      if (v) config[field.key] = v;
    }
    const input: CloudTargetInput = {
      name: f.name.trim(),
      provider: f.provider,
      config,
      keyPrefix: f.keyPrefix.trim(),
      encrypted: f.encrypted,
      bandwidthMbps: bw,
      enabled: f.enabled,
    };
    if (spec.user) {
      // Düzenlemede boş bırakılan kimlik bilgisi değişmez.
      if (f.user.trim() || f.id === null) input.user = f.user.trim();
      if (f.secret || f.id === null) input.secret = f.secret;
    }
    saving = true;
    try {
      apply(await catalog.backend.saveCloudTarget(f.id, input));
      form = null;
      catalog.flash(f.id === null ? t("cloud.added", { name: input.name }) : t("cloud.saved", { name: input.name }));
    } catch (e) {
      catalog.fail(e);
    } finally {
      saving = false;
    }
  }

  async function toggle(t: CloudTargetDto): Promise<void> {
    try {
      apply(await catalog.backend.setCloudTargetEnabled(t.id, !t.enabled));
    } catch (e) {
      catalog.fail(e);
    }
  }

  async function remove(id: number): Promise<void> {
    confirmDelete = null;
    try {
      apply(await catalog.backend.deleteCloudTarget(id));
      catalog.flash(t("cloud.deleted"));
    } catch (e) {
      catalog.fail(e);
    }
  }

  async function saveGlobal(): Promise<void> {
    const bw = parseMbps(globalBw);
    if (bw === undefined) {
      catalog.error = t("cloud.bwInvalid");
      return;
    }
    try {
      apply(await catalog.backend.setGlobalBandwidth(bw));
      catalog.flash(bw === null ? t("cloud.bwUnlimited") : t("cloud.bwSet", { n: bw }));
    } catch (e) {
      catalog.fail(e);
    }
  }

  async function sync(): Promise<void> {
    try {
      const st = await catalog.backend.startCloudSync();
      const pending = st.reduce((a, t) => a + t.pending, 0);
      catalog.flash(t("app.cloudStarted", { n: count(pending) }));
      void catalog.refreshStatus();
    } catch (e) {
      catalog.fail(e);
    }
  }

  function where(t: CloudTargetDto): string {
    const c = t.config;
    const s = (k: string) => (typeof c[k] === "string" ? (c[k] as string) : "");
    return t.provider === "local" ? s("path") : t.provider === "webdav" ? s("url") : `${s("bucket")} · ${s("endpoint")}`;
  }
</script>

<div class="flex flex-col gap-4 p-4">
  <section class="flex flex-wrap items-end gap-3">
    <div class="flex flex-1 flex-col gap-1">
      <span class="label">{t("cloud.credentials")}</span>
      <span class="text-sm text-ink-2">
        {#if settings?.secretBackend === "secret-service"}
          {t("cloud.keyring")}
        {:else if settings}
          {t("cloud.fileSecrets")}
        {/if}
      </span>
    </div>
    <label class="flex flex-col gap-1">
      <span class="label">{t("cloud.totalBw")}</span>
      <span class="flex gap-2">
        <input class="input data w-24" bind:value={globalBw} placeholder={t("cloud.unlimited")} inputmode="decimal" aria-label={t("cloud.totalMbps")} />
        <span class="data self-center text-xs text-ink-3">MB/s</span>
        <button class="btn" onclick={saveGlobal}>{t("cloud.apply")}</button>
      </span>
    </label>
    <button class="btn btn-primary" onclick={sync}><CloudUpload size={14} strokeWidth={1.5} />{t("cloud.startSync")}</button>
  </section>

  <section>
    <h2 class="label flex justify-between pb-2">
      <span>{t("cloud.targets")}</span>
      {#if !form}<button class="text-ink-2 normal-case hover:text-ink" onclick={() => (form = blank())}>{t("cloud.addTarget")}</button>{/if}
    </h2>
    <ul class="border border-line">
      {#each settings?.targets ?? [] as tg, i (tg.id)}
        {@const st = statusOf.get(tg.id)}
        <li class="flex items-center gap-3 px-3 py-2 {i > 0 ? 'border-t border-line' : ''}">
          <div class="flex min-w-0 flex-1 flex-col">
            <span class="flex items-baseline gap-2">
              <span class="truncate text-sm {tg.enabled ? 'text-ink' : 'text-ink-3'}">{tg.name}</span>
              <span class="label">{providerSpec(tg.provider).name}</span>
              {#if tg.encrypted}<span class="data text-2xs text-ink-2">age</span>{/if}
            </span>
            <span class="data truncate text-2xs text-ink-3">{where(tg)}</span>
            {#if st}
              <span class="data text-2xs {st.state === 'error' || st.state === 'needs_setup' ? 'text-amber' : 'text-ink-3'}">
                {st.state === "needs_setup"
                  ? t("cloud.needsKey")
                  : st.state}{st.pending > 0 ? ` · ${t("ai.queued", { n: count(st.pending) })}` : ""} · {t("cloud.synced", { n: count(st.synced) })}{st.lastError
                  ? ` · ${st.lastError}`
                  : ""}
              </span>
            {/if}
          </div>
          <span class="data text-2xs text-ink-3">{tg.bandwidthMbps === null ? "∞" : `${tg.bandwidthMbps} MB/s`}</span>
          <button
            class="btn h-6 {tg.enabled ? '' : 'text-ink-3'}"
            onclick={() => toggle(tg)}
            aria-pressed={tg.enabled}
            aria-label={tg.enabled ? t("cloud.disable", { name: tg.name }) : t("cloud.enable", { name: tg.name })}
            >{tg.enabled ? t("cloud.on") : t("cloud.off")}</button
          >
          <button class="btn btn-ghost h-6 px-1" onclick={() => edit(tg)} aria-label={t("cloud.edit", { name: tg.name })}>
            <Pencil size={12} strokeWidth={1.5} />
          </button>
          {#if confirmDelete === tg.id}
            <button class="btn h-6 border-amber text-amber" onclick={() => remove(tg.id)}>{t("common.delete")}</button>
          {:else}
            <button class="btn btn-ghost h-6 px-1" onclick={() => (confirmDelete = tg.id)} aria-label={t("cloud.delete", { name: tg.name })}>
              <Trash size={12} strokeWidth={1.5} />
            </button>
          {/if}
        </li>
      {:else}
        <li class="px-3 py-2 text-xs text-ink-3">{t("cloud.none")}</li>
      {/each}
    </ul>
  </section>

  {#if form && spec}
    <section class="flex flex-col gap-3 border border-line p-3" aria-label={t("cloud.targetForm")}>
      <div class="grid grid-cols-2 gap-3">
        <label class="flex flex-col gap-1">
          <span class="label">{t("cloud.provider")}</span>
          <select class="input" bind:value={form.provider} disabled={form.id !== null} aria-label={t("cloud.provider")}>
            {#each PROVIDERS as p (p.key)}<option value={p.key}>{p.name}</option>{/each}
          </select>
        </label>
        <label class="flex flex-col gap-1">
          <span class="label">{t("wifi.name")}</span>
          <input class="input" bind:value={form.name} placeholder={t("cloud.namePlaceholder")} aria-label={t("cloud.targetName")} />
        </label>
        {#each spec.fields as field (field.key)}
          <label class="flex flex-col gap-1 {field.key === 'endpoint' || field.key === 'url' || field.key === 'path' ? 'col-span-2' : ''}">
            <span class="label">{field.label}</span>
            <input
              class="input data"
              value={form.config[field.key] ?? ""}
              oninput={(e) => form && (form.config[field.key] = (e.currentTarget as HTMLInputElement).value)}
              placeholder={field.placeholder}
              aria-label={field.label}
              spellcheck="false"
            />
          </label>
        {/each}
        {#if spec.user && spec.secret}
          <label class="flex flex-col gap-1">
            <span class="label">{spec.user}</span>
            <input class="input data" bind:value={form.user} aria-label={spec.user} spellcheck="false" autocomplete="off" />
          </label>
          <label class="flex flex-col gap-1">
            <span class="label">{spec.secret}</span>
            <input
              class="input data"
              type="password"
              bind:value={form.secret}
              placeholder={form.hasCredentials ? t("cloud.storedSecret") : ""}
              aria-label={spec.secret}
              autocomplete="new-password"
            />
          </label>
        {/if}
        <label class="flex flex-col gap-1">
          <span class="label">{t("cloud.prefix")}</span>
          <input class="input data" bind:value={form.keyPrefix} aria-label={t("cloud.prefix")} spellcheck="false" />
        </label>
        <label class="flex flex-col gap-1">
          <span class="label">{t("cloud.bw")}</span>
          <input class="input data" bind:value={form.bandwidth} placeholder={t("cloud.unlimited")} inputmode="decimal" aria-label={t("cloud.targetMbps")} />
        </label>
      </div>
      <label class="flex items-center gap-2 text-sm text-ink-2">
        <input type="checkbox" bind:checked={form.encrypted} disabled={form.wasEncrypted} />
        {t("cloud.encrypt")}
        <span class="text-xs text-ink-3">
          {form.wasEncrypted ? t("cloud.encryptLocked") : t("cloud.encryptHint")}
        </span>
      </label>
      <div class="flex justify-end gap-2">
        <button class="btn btn-ghost" onclick={() => (form = null)}>{t("common.cancel")}</button>
        <button class="btn btn-primary" onclick={save} disabled={saving || !form.name.trim()}>{t("common.save")}</button>
      </div>
    </section>
  {/if}
</div>

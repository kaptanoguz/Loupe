<script lang="ts" module>
  import type { CloudProvider } from "../../api/types";

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
    { key: "endpoint", label: "Uç nokta", placeholder: endpoint },
    { key: "bucket", label: "Kova", placeholder: "loupe-yedek" },
    { key: "region", label: "Bölge", placeholder: region },
  ];

  export const PROVIDERS: ProviderSpec[] = [
    { key: "s3", name: "Amazon S3", fields: S3_FIELDS("https://s3.eu-central-1.amazonaws.com", "eu-central-1"), user: "Erişim anahtarı", secret: "Gizli anahtar" },
    { key: "b2", name: "Backblaze B2", fields: S3_FIELDS("https://s3.eu-central-003.backblazeb2.com", "eu-central-003"), user: "keyID", secret: "applicationKey" },
    { key: "r2", name: "Cloudflare R2", fields: S3_FIELDS("https://<hesap>.r2.cloudflarestorage.com", "auto"), user: "Erişim anahtarı", secret: "Gizli anahtar" },
    { key: "wasabi", name: "Wasabi", fields: S3_FIELDS("https://s3.eu-central-1.wasabisys.com", "eu-central-1"), user: "Erişim anahtarı", secret: "Gizli anahtar" },
    { key: "webdav", name: "WebDAV", fields: [{ key: "url", label: "Adres", placeholder: "https://bulut.example.org/remote.php/dav/files/oguz/" }], user: "Kullanıcı", secret: "Parola" },
    { key: "local", name: "Yerel klasör", fields: [{ key: "path", label: "Klasör", placeholder: "/mnt/yedek/loupe" }], user: null, secret: null },
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
      catalog.error = "Bant genişliği pozitif bir sayı (MB/s) olmalı";
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
      catalog.flash(f.id === null ? `Hedef eklendi: ${input.name}` : `Hedef kaydedildi: ${input.name}`);
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
      catalog.flash("Hedef silindi (buluttaki nesnelere dokunulmadı)");
    } catch (e) {
      catalog.fail(e);
    }
  }

  async function saveGlobal(): Promise<void> {
    const bw = parseMbps(globalBw);
    if (bw === undefined) {
      catalog.error = "Bant genişliği pozitif bir sayı (MB/s) olmalı";
      return;
    }
    try {
      apply(await catalog.backend.setGlobalBandwidth(bw));
      catalog.flash(bw === null ? "Toplam bant genişliği sınırsız" : `Toplam bant genişliği ${bw} MB/s`);
    } catch (e) {
      catalog.fail(e);
    }
  }

  async function sync(): Promise<void> {
    try {
      const st = await catalog.backend.startCloudSync();
      const pending = st.reduce((a, t) => a + t.pending, 0);
      catalog.flash(`Bulut senkronizasyonu başladı · ${count(pending)} dosya sırada`);
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
      <span class="label">Kimlik bilgileri</span>
      <span class="text-sm text-ink-2">
        {#if settings?.secretBackend === "secret-service"}
          Sistem anahtarlığında (Secret Service: GNOME Keyring / KWallet)
        {:else if settings}
          Yalnızca sizin okuyabildiğiniz dosyalarda (0600). Secret Service bulunamadı.
        {/if}
      </span>
    </div>
    <label class="flex flex-col gap-1">
      <span class="label">Toplam bant genişliği</span>
      <span class="flex gap-2">
        <input class="input data w-24" bind:value={globalBw} placeholder="sınırsız" inputmode="decimal" aria-label="Toplam MB/s" />
        <span class="data self-center text-xs text-ink-3">MB/s</span>
        <button class="btn" onclick={saveGlobal}>Uygula</button>
      </span>
    </label>
    <button class="btn btn-primary" onclick={sync}><CloudUpload size={14} strokeWidth={1.5} />Senkronizasyonu başlat</button>
  </section>

  <section>
    <h2 class="label flex justify-between pb-2">
      <span>Hedefler</span>
      {#if !form}<button class="text-ink-2 normal-case hover:text-ink" onclick={() => (form = blank())}>+ Hedef ekle</button>{/if}
    </h2>
    <ul class="border border-line">
      {#each settings?.targets ?? [] as t, i (t.id)}
        {@const st = statusOf.get(t.id)}
        <li class="flex items-center gap-3 px-3 py-2 {i > 0 ? 'border-t border-line' : ''}">
          <div class="flex min-w-0 flex-1 flex-col">
            <span class="flex items-baseline gap-2">
              <span class="truncate text-sm {t.enabled ? 'text-ink' : 'text-ink-3'}">{t.name}</span>
              <span class="label">{providerSpec(t.provider).name}</span>
              {#if t.encrypted}<span class="data text-2xs text-ink-2">age</span>{/if}
            </span>
            <span class="data truncate text-2xs text-ink-3">{where(t)}</span>
            {#if st}
              <span class="data text-2xs {st.state === 'error' || st.state === 'needs_setup' ? 'text-amber' : 'text-ink-3'}">
                {st.state === "needs_setup"
                  ? "kurtarma anahtarı gerekli"
                  : st.state}{st.pending > 0 ? ` · ${count(st.pending)} sırada` : ""} · {count(st.synced)} yedekli{st.lastError
                  ? ` · ${st.lastError}`
                  : ""}
              </span>
            {/if}
          </div>
          <span class="data text-2xs text-ink-3">{t.bandwidthMbps === null ? "∞" : `${t.bandwidthMbps} MB/s`}</span>
          <button
            class="btn h-6 {t.enabled ? '' : 'text-ink-3'}"
            onclick={() => toggle(t)}
            aria-pressed={t.enabled}
            aria-label="{t.name} {t.enabled ? 'kapat' : 'aç'}">{t.enabled ? "Açık" : "Kapalı"}</button
          >
          <button class="btn btn-ghost h-6 px-1" onclick={() => edit(t)} aria-label="{t.name} düzenle">
            <Pencil size={12} strokeWidth={1.5} />
          </button>
          {#if confirmDelete === t.id}
            <button class="btn h-6 border-amber text-amber" onclick={() => remove(t.id)}>Sil</button>
          {:else}
            <button class="btn btn-ghost h-6 px-1" onclick={() => (confirmDelete = t.id)} aria-label="{t.name} sil">
              <Trash size={12} strokeWidth={1.5} />
            </button>
          {/if}
        </li>
      {:else}
        <li class="px-3 py-2 text-xs text-ink-3">Henüz bulut hedefi yok.</li>
      {/each}
    </ul>
  </section>

  {#if form && spec}
    <section class="flex flex-col gap-3 border border-line p-3" aria-label="Hedef ayarı">
      <div class="grid grid-cols-2 gap-3">
        <label class="flex flex-col gap-1">
          <span class="label">Sağlayıcı</span>
          <select class="input" bind:value={form.provider} disabled={form.id !== null} aria-label="Sağlayıcı">
            {#each PROVIDERS as p (p.key)}<option value={p.key}>{p.name}</option>{/each}
          </select>
        </label>
        <label class="flex flex-col gap-1">
          <span class="label">Ad</span>
          <input class="input" bind:value={form.name} placeholder="B2 Yedek" aria-label="Hedef adı" />
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
              placeholder={form.hasCredentials ? "kayıtlı · değiştirmek için yazın" : ""}
              aria-label={spec.secret}
              autocomplete="new-password"
            />
          </label>
        {/if}
        <label class="flex flex-col gap-1">
          <span class="label">Anahtar öneki</span>
          <input class="input data" bind:value={form.keyPrefix} aria-label="Anahtar öneki" spellcheck="false" />
        </label>
        <label class="flex flex-col gap-1">
          <span class="label">Bant genişliği (MB/s)</span>
          <input class="input data" bind:value={form.bandwidth} placeholder="sınırsız" inputmode="decimal" aria-label="Hedef MB/s" />
        </label>
      </div>
      <label class="flex items-center gap-2 text-sm text-ink-2">
        <input type="checkbox" bind:checked={form.encrypted} disabled={form.wasEncrypted} />
        age ile uçtan uca şifrele
        <span class="text-xs text-ink-3">
          {form.wasEncrypted ? "şifreli hedefte kapatılamaz" : "kapalıysa dosyalar sağlayıcıda açık durur"}
        </span>
      </label>
      <div class="flex justify-end gap-2">
        <button class="btn btn-ghost" onclick={() => (form = null)}>Vazgeç</button>
        <button class="btn btn-primary" onclick={save} disabled={saving || !form.name.trim()}>Kaydet</button>
      </div>
    </section>
  {/if}
</div>

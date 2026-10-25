<script lang="ts">
  import Copy from "@lucide/svelte/icons/copy";
  import Download from "@lucide/svelte/icons/download";
  import ShieldCheck from "@lucide/svelte/icons/shield-check";
  import { onDestroy, onMount } from "svelte";

  import type { EncryptionDto, RecoveryDraftDto } from "../../api/types";
  import { dateTimeMs } from "../../format";
  import type { Catalog } from "../../state/catalog.svelte";

  interface Props {
    catalog: Catalog;
  }

  let { catalog }: Props = $props();

  /** Akış: durum → anahtar gösterilir → üç grup geri yazılır → kurulur. */
  type Step = "status" | "show" | "challenge";

  let enc = $state.raw<EncryptionDto | null>(null);
  let step = $state<Step>("status");
  let draft = $state.raw<RecoveryDraftDto | null>(null);
  let answers = $state<Record<number, string>>({});
  let saved = $state<string | null>(null);
  let copied = $state(false);
  let checkInput = $state("");
  let checkResult = $state<boolean | null>(null);
  let busy = $state(false);

  onMount(() => void load());

  onDestroy(() => {
    // Yarım kalan taslak bellekte bırakılmaz.
    if (draft) void catalog.backend.cancelRecoveryKey().catch(() => undefined);
  });

  async function load(): Promise<void> {
    try {
      enc = (await catalog.backend.cloudSettings()).encryption;
    } catch (e) {
      catalog.fail(e);
    }
  }

  async function generate(): Promise<void> {
    busy = true;
    try {
      draft = await catalog.backend.generateRecoveryKey();
      answers = {};
      saved = null;
      copied = false;
      step = "show";
    } catch (e) {
      catalog.fail(e);
    } finally {
      busy = false;
    }
  }

  async function copy(): Promise<void> {
    if (!draft) return;
    try {
      await navigator.clipboard.writeText(draft.secret);
      copied = true;
    } catch (e) {
      catalog.fail(e);
    }
  }

  async function exportFile(): Promise<void> {
    try {
      const path = await catalog.backend.exportRecoveryKey();
      if (path) saved = path;
    } catch (e) {
      catalog.fail(e);
    }
  }

  async function confirm(): Promise<void> {
    if (!draft) return;
    busy = true;
    try {
      enc = await catalog.backend.confirmRecoveryKey(
        draft.challenge.map((g) => ({ group: g, value: (answers[g] ?? "").trim().toUpperCase() })),
      );
      draft = null;
      step = "status";
      catalog.flash("Kurtarma anahtarı kuruldu; şifreli yedekler başlayabilir");
      void catalog.refreshStatus();
    } catch (e) {
      catalog.fail(e);
    } finally {
      busy = false;
    }
  }

  async function cancel(): Promise<void> {
    draft = null;
    step = "status";
    try {
      await catalog.backend.cancelRecoveryKey();
    } catch (e) {
      catalog.fail(e);
    }
  }

  async function check(): Promise<void> {
    try {
      checkResult = await catalog.backend.checkRecoveryKey(checkInput.trim());
    } catch (e) {
      checkResult = null;
      catalog.fail(e);
    }
  }

  const complete = $derived(draft ? draft.challenge.every((g) => (answers[g] ?? "").trim().length === (draft?.groups[g]?.length ?? 4)) : false);
</script>

<div class="flex flex-col gap-4 p-4">
  {#if step === "status"}
    <section class="flex flex-col gap-2">
      <h2 class="label">age kurtarma anahtarı</h2>
      {#if enc?.configured}
        <dl class="kv gap-y-1">
          <dt>Açık anahtar</dt>
          <dd class="truncate" title={enc.recipient ?? ""}>{enc.recipient}</dd>
          <dt>Parmak izi</dt>
          <dd>{enc.fingerprint ?? "—"}</dd>
          <dt>Oluşturuldu</dt>
          <dd>{dateTimeMs(enc.createdAt)}</dd>
          <dt>Cihaz anahtarı</dt>
          <dd>{enc.deviceKey ? "var" : "yok"}</dd>
        </dl>
      {:else if enc}
        <p class="text-sm text-ink-2">
          Şifreli bulut yedekleri için bir kurtarma anahtarı gerekir. Bu cihaz kaybolursa yedekler yalnızca bu
          anahtarla açılır; Loupe onu bir kez gösterir, sonra yalnızca doğrulayabilir.
        </p>
      {/if}
      <div class="flex gap-2 pt-1">
        <button class="btn {enc?.configured ? '' : 'btn-primary'}" onclick={generate} disabled={busy}>
          {enc?.configured ? "Yeni anahtar oluştur" : "Kurtarma anahtarı oluştur"}
        </button>
      </div>
    </section>

    {#if enc?.configured}
      <section class="flex flex-col gap-2 border-t border-line pt-4">
        <h2 class="label">Elinizdeki anahtarı doğrulayın</h2>
        <textarea
          class="input data h-16 py-1 text-xs"
          bind:value={checkInput}
          oninput={() => (checkResult = null)}
          placeholder="AGE-SECRET-KEY-1…"
          aria-label="Kurtarma anahtarı"
          spellcheck="false"
          autocomplete="off"
        ></textarea>
        <div class="flex items-center gap-3">
          <button class="btn" onclick={check} disabled={!checkInput.trim()}>
            <ShieldCheck size={14} strokeWidth={1.5} />Doğrula
          </button>
          {#if checkResult === true}
            <span class="text-sm text-ink">Eşleşiyor: bu anahtar yedekleri açar.</span>
          {:else if checkResult === false}
            <span class="text-sm text-amber">Eşleşmiyor: bu anahtar bu kütüphanenin değil.</span>
          {/if}
        </div>
      </section>
    {/if}
  {:else if step === "show" && draft}
    <section class="flex flex-col gap-3">
      <h2 class="label">Anahtarı kâğıda yazın ya da güvenli bir yere kaydedin</h2>
      {#if draft.replaces}
        <p class="border border-amber px-3 py-2 text-xs text-amber">
          Kurulu anahtarın yerine geçer. Eski yedekleri açmak için eski anahtar yine gerekir.
        </p>
      {/if}
      <div class="data text-xs text-ink-3">{draft.prefix}</div>
      <ol class="data grid grid-cols-5 gap-x-3 gap-y-1 border border-line p-3" aria-label="Anahtar grupları">
        {#each draft.groups as g, i (i)}
          <li class="flex gap-2 text-sm">
            <span class="w-5 text-right text-2xs leading-5 text-ink-3">{i + 1}</span><span class="text-ink">{g}</span>
          </li>
        {/each}
      </ol>
      <div class="flex flex-wrap items-center gap-2">
        <button class="btn" onclick={copy}><Copy size={14} strokeWidth={1.5} />{copied ? "Kopyalandı" : "Kopyala"}</button>
        <button class="btn" onclick={exportFile}><Download size={14} strokeWidth={1.5} />Dosyaya kaydet</button>
        {#if saved}<span class="data truncate text-2xs text-ink-3">{saved}</span>{/if}
        <span class="flex-1"></span>
        <button class="btn btn-ghost" onclick={cancel}>Vazgeç</button>
        <button class="btn btn-primary" onclick={() => (step = "challenge")}>Yazdım, devam</button>
      </div>
    </section>
  {:else if step === "challenge" && draft}
    <section class="flex flex-col gap-3">
      <h2 class="label">Sakladığınızı doğrulayın</h2>
      <p class="text-sm text-ink-2">Yazdığınız anahtardan şu grupları girin:</p>
      <div class="flex gap-3">
        {#each draft.challenge as g (g)}
          <label class="flex flex-col gap-1">
            <span class="label">Grup {g + 1}</span>
            <input
              class="input data w-24 uppercase"
              maxlength="4"
              value={answers[g] ?? ""}
              oninput={(e) => (answers[g] = (e.currentTarget as HTMLInputElement).value)}
              aria-label="Grup {g + 1}"
              spellcheck="false"
              autocomplete="off"
            />
          </label>
        {/each}
      </div>
      <div class="flex gap-2">
        <button class="btn btn-ghost" onclick={() => (step = "show")}>Geri</button>
        <span class="flex-1"></span>
        <button class="btn btn-ghost" onclick={cancel}>Vazgeç</button>
        <button class="btn btn-primary" onclick={confirm} disabled={!complete || busy}>Anahtarı kur</button>
      </div>
    </section>
  {/if}
</div>

<script lang="ts">
  import FolderMinus from "@lucide/svelte/icons/folder-minus";
  import Save from "@lucide/svelte/icons/save";

  import type { ImportMode, LibraryDto, LibraryPatch } from "../../api/types";
  import { bytes, count } from "../../format";
  import { DATE_PATTERNS, joinLayout, layoutError, previewPath, splitLayout } from "../../importLayout";
  import type { Catalog } from "../../state/catalog.svelte";

  interface Props {
    catalog: Catalog;
  }

  let { catalog }: Props = $props();

  const MODES: { key: ImportMode; name: string }[] = [
    { key: "copy", name: "Kopyala" },
    { key: "move", name: "Taşı" },
    { key: "hardlink", name: "Bağla" },
  ];
  const STATE: Record<LibraryDto["state"], string> = {
    online: "bağlı",
    offline: "bağlı değil",
    readonly: "salt okunur",
    error: "hata",
  };

  const libs = $derived(catalog.sidebar?.libraries ?? []);
  /** Düzenlenen alanlar (kütüphane kimliğine göre); kaydedilince silinir. */
  let names = $state<Record<number, string>>({});
  let layouts = $state<Record<number, { sub: string; date: string }>>({});
  let confirming = $state<number | null>(null);
  let busy = $state(false);

  function layoutOf(l: LibraryDto): { sub: string; date: string } {
    return layouts[l.id] ?? splitLayout(l.layoutTemplate);
  }

  function setLayout(l: LibraryDto, patch: Partial<{ sub: string; date: string }>): void {
    layouts[l.id] = { ...layoutOf(l), ...patch };
  }

  async function update(l: LibraryDto, patch: LibraryPatch, done: string): Promise<void> {
    busy = true;
    try {
      await catalog.backend.updateLibrary(l.id, patch);
      await catalog.refreshSidebar();
      catalog.flash(done);
    } catch (e) {
      catalog.fail(e);
    } finally {
      busy = false;
    }
  }

  async function saveName(l: LibraryDto): Promise<void> {
    const name = names[l.id]?.trim();
    if (!name || name === l.name) return;
    await update(l, { name }, `Ad değişti: ${name}`);
    delete names[l.id];
  }

  async function saveLayout(l: LibraryDto): Promise<void> {
    const { sub, date } = layoutOf(l);
    const layoutTemplate = joinLayout(sub, date);
    await update(l, { layoutTemplate }, `${l.name}: yeni aktarımlar ${layoutTemplate} düzeniyle`);
    delete layouts[l.id];
  }

  async function remove(l: LibraryDto): Promise<void> {
    busy = true;
    try {
      const n = await catalog.backend.removeLibrary(l.id);
      if (catalog.view.kind === "library" && catalog.view.id === l.id) void catalog.open({ kind: "all" });
      else void catalog.open(catalog.view);
      await catalog.refreshSidebar();
      catalog.flash(`${l.name} kaldırıldı · ${count(n)} kayıt (dosyalar diskte duruyor)`);
    } catch (e) {
      catalog.fail(e);
    } finally {
      busy = false;
      confirming = null;
    }
  }
</script>

<div class="flex flex-col gap-6 p-4">
  <section class="flex flex-col gap-2">
    <h2 class="label">Kütüphaneler</h2>
    <p class="text-xs text-ink-3">
      <b class="text-ink-2">Yönetilen</b> kütüphaneye içe aktarılan fotoğraflar kopyalanır ya da taşınır;
      <b class="text-ink-2">birincil</b> olan içe aktarma penceresinde önce seçili gelir. Yeni klasörü kenar çubuğundaki
      "Klasör" düğmesiyle ekleyin.
    </p>
    {#if libs.length === 0}
      <p class="text-sm text-ink-2">Henüz klasör yok.</p>
    {/if}
    {#each libs as l (l.id)}
      {@const lay = layoutOf(l)}
      {@const template = joinLayout(lay.sub, lay.date)}
      {@const problem = layoutError(template)}
      {@const dirty = layouts[l.id] !== undefined && template !== l.layoutTemplate}
      <article class="flex flex-col gap-3 border border-line p-3" aria-label={l.name}>
        <div class="flex items-center gap-2">
          <input
            class="input h-6 min-w-0 flex-1 text-sm"
            aria-label="Kütüphane adı"
            value={names[l.id] ?? l.name}
            oninput={(e) => (names[l.id] = (e.currentTarget as HTMLInputElement).value)}
            onblur={() => saveName(l)}
            onkeydown={(e) => e.key === "Enter" && (e.currentTarget as HTMLInputElement).blur()}
          />
          {#if l.primary}
            <span class="border border-amber px-1 text-2xs text-amber">birincil</span>
          {:else if l.managed}
            <button class="btn h-6" disabled={busy || l.state !== "online"} onclick={() => update(l, { primary: true }, `${l.name} birincil kütüphane`)}
              >Birincil yap</button
            >
          {/if}
          <span class="text-2xs text-ink-3">{l.managed ? "yönetilen" : "referans (yalnızca dizinlenir)"}</span>
        </div>
        <dl class="kv data text-2xs">
          <dt>Klasör</dt>
          <dd class="break-all">{l.root ?? "—"}</dd>
          <dt>Durum</dt>
          <dd class={l.state === "online" ? "" : "text-amber"}>{STATE[l.state]}{l.stateReason ? ` · ${l.stateReason}` : ""}</dd>
          <dt>Fotoğraf</dt>
          <dd>{count(l.mediaCount)}</dd>
          <dt>Disk</dt>
          <dd>{l.fsType ?? "?"}{l.freeBytes !== null ? ` · ${bytes(l.freeBytes)} boş` : ""}</dd>
        </dl>

        {#if l.managed}
          <div class="flex flex-col gap-1">
            <span class="label">İçe aktarma klasör düzeni (varsayılan)</span>
            <div class="flex gap-1">
              <input
                class="input h-6 w-36 text-xs"
                aria-label="Alt klasör"
                placeholder="alt klasör (boş)"
                value={lay.sub}
                oninput={(e) => setLayout(l, { sub: (e.currentTarget as HTMLInputElement).value })}
              />
              {#each DATE_PATTERNS as p (p.value)}
                <button
                  class="btn h-6 flex-1 px-1 text-xs {lay.date === p.value ? 'border-amber text-ink' : 'text-ink-2'}"
                  aria-pressed={lay.date === p.value}
                  onclick={() => setLayout(l, { date: p.value })}>{p.label}</button
                >
              {/each}
            </div>
            {#if problem}
              <p class="text-2xs text-amber">{problem}</p>
            {:else if l.root}
              <p class="data text-2xs break-all text-ink-3">{previewPath(l.root, template, "IMG_1234.HEIC")}</p>
            {/if}
            {#if dirty}
              <div class="flex justify-end gap-2">
                <button class="btn h-6" onclick={() => delete layouts[l.id]}>Vazgeç</button>
                <button class="btn btn-primary h-6" disabled={busy || !!problem} onclick={() => saveLayout(l)}
                  ><Save size={12} strokeWidth={1.5} />Kaydet</button
                >
              </div>
            {/if}
          </div>

          <div class="flex items-center gap-2">
            <span class="label w-36">Varsayılan mod</span>
            {#each MODES as m (m.key)}
              <button
                class="btn h-6 flex-1 text-xs {l.defaultImportMode === m.key ? 'border-amber text-ink' : 'text-ink-2'}"
                aria-pressed={l.defaultImportMode === m.key}
                disabled={busy}
                onclick={() => update(l, { defaultImportMode: m.key }, `${l.name}: varsayılan mod ${m.name}`)}
                >{m.name}</button
              >
            {/each}
          </div>
        {/if}

        <div class="flex items-center justify-end gap-2 border-t border-line pt-2">
          {#if confirming === l.id}
            <span class="flex-1 text-xs text-ink-2"
              >{count(l.mediaCount)} kayıt Loupe'den çıkar. Diskteki dosyalara dokunulmaz.</span
            >
            <button class="btn h-6" onclick={() => (confirming = null)}>Vazgeç</button>
            <button class="btn h-6 border-amber text-amber" disabled={busy} onclick={() => remove(l)}>Kaldır</button>
          {:else}
            <button class="btn btn-ghost h-6 text-ink-3" onclick={() => (confirming = l.id)}
              ><FolderMinus size={12} strokeWidth={1.5} />Loupe'den kaldır</button
            >
          {/if}
        </div>
      </article>
    {/each}
  </section>

  <section class="flex flex-col gap-2">
    <h2 class="label">Silme ve Çöp Kutusu</h2>
    <p class="text-xs text-ink-2">
      Seçili fotoğraflar (Ctrl ya da Shift ile seçip Delete) onaydan sonra sistemin Çöp Kutusu'na taşınır, sidecar
      dosyalarıyla birlikte. Çöp Kutusu kurulamayan disklerde (ör. kullanıcıya bağlanmamış NTFS) aynı diskteki
      <span class="data">&lt;kütüphane&gt;/.loupe/trash/&lt;tarih&gt;/</span> klasörüne taşınır. Geri almak için dosyayı
      eski yerine taşıyın; Loupe açıkken klasör izleyicisi onu yeniden kataloglar.
    </p>
  </section>
</div>

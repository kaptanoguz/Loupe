<script lang="ts">
  import FolderMinus from "@lucide/svelte/icons/folder-minus";
  import Save from "@lucide/svelte/icons/save";

  import type { ImportMode, LibraryDto, LibraryPatch } from "../../api/types";
  import { bytes, count } from "../../format";
  import { t } from "../../i18n";
  import { DATE_PATTERNS, joinLayout, layoutError, previewPath, splitLayout } from "../../importLayout";
  import type { Catalog } from "../../state/catalog.svelte";

  interface Props {
    catalog: Catalog;
  }

  let { catalog }: Props = $props();

  const MODES: { key: ImportMode; name: string }[] = [
    { key: "copy", name: t("import.copy") },
    { key: "move", name: t("import.move") },
    { key: "hardlink", name: t("import.link") },
  ];
  const STATE: Record<LibraryDto["state"], string> = {
    online: t("lib.online"),
    offline: t("sidebar.notMounted"),
    readonly: t("lib.readonly"),
    error: t("sync.error"),
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
    await update(l, { name }, t("lib.renamed", { name }));
    delete names[l.id];
  }

  async function saveLayout(l: LibraryDto): Promise<void> {
    const { sub, date } = layoutOf(l);
    const layoutTemplate = joinLayout(sub, date);
    await update(l, { layoutTemplate }, t("lib.layoutSaved", { name: l.name, layout: layoutTemplate }));
    delete layouts[l.id];
  }

  async function remove(l: LibraryDto): Promise<void> {
    busy = true;
    try {
      const n = await catalog.backend.removeLibrary(l.id);
      if (catalog.view.kind === "library" && catalog.view.id === l.id) void catalog.open({ kind: "all" });
      else void catalog.open(catalog.view);
      await catalog.refreshSidebar();
      catalog.flash(t("lib.removed", { name: l.name, n: count(n) }));
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
    <h2 class="label">{t("settings.libraries")}</h2>
    <p class="text-xs text-ink-3">
      {@html t("lib.intro")}
    </p>
    {#if libs.length === 0}
      <p class="text-sm text-ink-2">{t("lib.none")}</p>
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
            aria-label={t("lib.nameLabel")}
            value={names[l.id] ?? l.name}
            oninput={(e) => (names[l.id] = (e.currentTarget as HTMLInputElement).value)}
            onblur={() => saveName(l)}
            onkeydown={(e) => e.key === "Enter" && (e.currentTarget as HTMLInputElement).blur()}
          />
          {#if l.primary}
            <span class="border border-amber px-1 text-2xs text-amber">{t("import.primary")}</span>
          {:else if l.managed}
            <button class="btn h-6" disabled={busy || l.state !== "online"} onclick={() => update(l, { primary: true }, t("lib.nowPrimary", { name: l.name }))}
              >{t("lib.makePrimary")}</button
            >
          {/if}
          <span class="text-2xs text-ink-3">{l.managed ? t("lib.managed") : t("lib.reference")}</span>
        </div>
        <dl class="kv data text-2xs">
          <dt>{t("sidebar.folder")}</dt>
          <dd class="break-all">{l.root ?? "—"}</dd>
          <dt>{t("status.label")}</dt>
          <dd class={l.state === "online" ? "" : "text-amber"}>{STATE[l.state]}{l.stateReason ? ` · ${l.stateReason}` : ""}</dd>
          <dt>{t("lib.photos")}</dt>
          <dd>{count(l.mediaCount)}</dd>
          <dt>{t("lib.disk")}</dt>
          <dd>{l.fsType ?? "?"}{l.freeBytes !== null ? ` · ${t("sidebar.free", { size: bytes(l.freeBytes) })}` : ""}</dd>
        </dl>

        {#if l.managed}
          <div class="flex flex-col gap-1">
            <span class="label">{t("lib.layout")}</span>
            <div class="flex gap-1">
              <input
                class="input h-6 w-36 text-xs"
                aria-label={t("lib.sub")}
                placeholder={t("lib.subPlaceholder")}
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
                <button class="btn h-6" onclick={() => delete layouts[l.id]}>{t("common.cancel")}</button>
                <button class="btn btn-primary h-6" disabled={busy || !!problem} onclick={() => saveLayout(l)}
                  ><Save size={12} strokeWidth={1.5} />{t("common.save")}</button
                >
              </div>
            {/if}
          </div>

          <div class="flex items-center gap-2">
            <span class="label w-36">{t("lib.defaultMode")}</span>
            {#each MODES as m (m.key)}
              <button
                class="btn h-6 flex-1 text-xs {l.defaultImportMode === m.key ? 'border-amber text-ink' : 'text-ink-2'}"
                aria-pressed={l.defaultImportMode === m.key}
                disabled={busy}
                onclick={() => update(l, { defaultImportMode: m.key }, t("lib.modeSaved", { name: l.name, mode: m.name }))}
                >{m.name}</button
              >
            {/each}
          </div>
        {/if}

        <div class="flex items-center justify-end gap-2 border-t border-line pt-2">
          {#if confirming === l.id}
            <span class="flex-1 text-xs text-ink-2"
              >{t("sidebar.removeBody", { n: count(l.mediaCount) })}</span
            >
            <button class="btn h-6" onclick={() => (confirming = null)}>{t("common.cancel")}</button>
            <button class="btn h-6 border-amber text-amber" disabled={busy} onclick={() => remove(l)}>{t("common.remove")}</button>
          {:else}
            <button class="btn btn-ghost h-6 text-ink-3" onclick={() => (confirming = l.id)}
              ><FolderMinus size={12} strokeWidth={1.5} />{t("lib.removeFromLoupe")}</button
            >
          {/if}
        </div>
      </article>
    {/each}
  </section>

  <section class="flex flex-col gap-2">
    <h2 class="label">{t("lib.trashTitle")}</h2>
    <p class="text-xs text-ink-2">
      {@html t("lib.trashHint")}
    </p>
  </section>
</div>

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import App from "./App.svelte";
import { MockBackend } from "./lib/api/mock";
import { LayoutEngine } from "./lib/layout/engine";

beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, "clientWidth", { configurable: true, get: () => 1200 });
  Object.defineProperty(HTMLElement.prototype, "clientHeight", { configurable: true, get: () => 800 });
  // jsdom'da canvas yok: çizim atlanır, denetimler yine çalışır.
  HTMLCanvasElement.prototype.getContext = (() => null) as never;
  // Önizleme görüntüsü 1500×1000 yüklenmiş sayılır (jsdom görüntü indirmez).
  Object.defineProperty(HTMLImageElement.prototype, "naturalWidth", { configurable: true, get: () => 1500 });
  Object.defineProperty(HTMLImageElement.prototype, "naturalHeight", { configurable: true, get: () => 1000 });
  Object.defineProperty(HTMLImageElement.prototype, "src", {
    configurable: true,
    set(this: HTMLImageElement, v: string) {
      this.setAttribute("src", v);
      queueMicrotask(() => this.onload?.(new Event("load")));
    },
    get(this: HTMLImageElement) {
      return this.getAttribute("src") ?? "";
    },
  });
  vi.stubGlobal(
    "ResizeObserver",
    class {
      private cb: ResizeObserverCallback;
      constructor(cb: ResizeObserverCallback) {
        this.cb = cb;
      }
      observe(target: Element): void {
        this.cb([{ target, contentRect: { width: 1200, height: 800 } } as unknown as ResizeObserverEntry], this as never);
      }
      unobserve(): void {}
      disconnect(): void {}
    },
  );
});

afterEach(() => cleanup());

async function openEditor() {
  const backend = new MockBackend(40);
  // İlk öğe video olmasın diye fotoğraf olarak işaretlenir.
  for (const it of backend.items) it.video = false;
  render(App, { backend, engine: new LayoutEngine(false) });
  await waitFor(() => expect(screen.getByRole("contentinfo", { name: "Durum" }).textContent).toContain("40 fotoğraf"));
  await fireEvent.keyDown(window, { key: "ArrowRight" });
  await fireEvent.keyDown(window, { key: "e" });
  const dialog = await screen.findByRole("dialog", { name: "Düzenle" });
  await waitFor(() => expect(within(dialog).getByRole("button", { name: "Kaydet" }).hasAttribute("disabled")).toBe(false));
  return { backend, dialog };
}

describe("düzenleme ekranı", () => {
  it("döndürme, kırpma oranı ve ışık ayarı kaydedilir; ızgara oranı güncellenir; orijinale dönülür", async () => {
    const { backend, dialog } = await openEditor();
    const aspects = new Map(backend.items.map((i) => [i.id, i.aspect]));
    await fireEvent.keyDown(window, { key: "r" });
    await fireEvent.click(within(dialog).getByRole("button", { name: /Kırp/ }));
    await fireEvent.click(within(dialog).getByRole("button", { name: "1:1" }));
    await fireEvent.input(within(dialog).getByRole("slider", { name: /Parlaklık/ }), { target: { value: "25" } });
    expect(within(dialog).getByText("+25")).toBeTruthy();
    await fireEvent.click(within(dialog).getByRole("button", { name: "Otomatik" }));
    await waitFor(() => expect(within(dialog).getByText("+12")).toBeTruthy());

    await fireEvent.click(within(dialog).getByRole("button", { name: "Kaydet" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Düzenle" })).toBeNull());
    const id = backend.items.find((i) => i.edit > 0)?.id as number;
    const before = aspects.get(id) as number;
    const st = await backend.editState(id);
    expect(st.revision).toBeGreaterThan(0);
    expect(st.recipe.quarter).toBe(1);
    expect(st.recipe.brightness).toBe(12);
    expect(st.aspect).toBeCloseTo(1);
    expect(before).not.toBeCloseTo(1);
    expect(screen.getByRole("contentinfo", { name: "Durum" }).textContent).toContain("Düzenleme kaydedildi");

    // Yeniden açılınca kayıtlı tarif gelir; orijinale dönüş tarifi siler.
    await fireEvent.keyDown(window, { key: "E" });
    const again = await screen.findByRole("dialog", { name: "Düzenle" });
    await waitFor(() => expect(within(again).getByText("+12")).toBeTruthy());
    await fireEvent.click(within(again).getByRole("button", { name: "Orijinale dön" }));
    await waitFor(async () => expect((await backend.editState(id)).revision).toBe(0));
    expect(backend.items.find((i) => i.id === id)?.aspect).toBeCloseTo(before);
  });

  it("kaydedilmemiş değişiklikte ilk Esc uyarır, ikincisi vazgeçer; dışa aktarma ve dış düzenleyici", async () => {
    const { backend, dialog } = await openEditor();
    await fireEvent.input(within(dialog).getByRole("slider", { name: /Kontrast/ }), { target: { value: "-30" } });
    await fireEvent.keyDown(window, { key: "Escape" });
    expect(within(dialog).getByText(/tekrar Esc/)).toBeTruthy();

    await fireEvent.click(within(dialog).getByRole("button", { name: "JPEG olarak dışa aktar…" }));
    await waitFor(() => expect(backend.exported.length).toBe(1));
    await fireEvent.click(within(dialog).getByRole("button", { name: "GIMP ile aç" }));
    expect(backend.openedIn[0]?.[1]).toBe("gimp");

    // Dışa aktarma önce kaydetti; artık değişiklik yok, tek Esc kapatır.
    await fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Düzenle" })).toBeNull());
    expect((await backend.editState(backend.exported[0] as number)).recipe.contrast).toBe(-30);
  });
});

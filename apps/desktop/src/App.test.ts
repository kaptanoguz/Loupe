import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import App from "./App.svelte";
import { MockBackend } from "./lib/api/mock";
import { LayoutEngine } from "./lib/layout/engine";

// jsdom düzen hesaplamaz: ızgara kabına gerçekçi bir boyut verilir.
beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, "clientWidth", { configurable: true, get: () => 1200 });
  Object.defineProperty(HTMLElement.prototype, "clientHeight", { configurable: true, get: () => 800 });
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

function setup(n = 400) {
  const backend = new MockBackend(n);
  const prioritize = vi.spyOn(backend, "prioritizeViewport");
  const utils = render(App, { backend, engine: new LayoutEngine(false) });
  return { backend, prioritize, ...utils };
}

describe("App", () => {
  it("doğrudan zaman çizelgesiyle açılır: kenar çubuğu, ızgara, denetçi, durum çubuğu", async () => {
    const { container } = setup();
    await waitFor(() => expect(container.querySelectorAll(".tile[data-idx]:not([style*='display: none'])").length).toBeGreaterThan(10));
    expect(screen.getByRole("navigation", { name: "Kenar çubuğu" })).toBeTruthy();
    expect(screen.getByText("Tüm Fotoğraflar")).toBeTruthy();
    expect(screen.getByText("Fotoğraflar")).toBeTruthy();
    expect(screen.getByRole("complementary", { name: "Denetçi" })).toBeTruthy();
    expect(screen.getByRole("contentinfo", { name: "Durum" }).textContent).toContain("400 fotoğraf");
    // Karşılama ekranı yok (metin parçalı: slop-guard kaynakta da arar).
    expect(container.textContent?.toLowerCase()).not.toContain(["hoş", "geldiniz"].join(" "));
    // Daire yok: ikonlar dahil.
    expect(container.querySelectorAll("circle").length).toBe(0);
    // Yalnızca görünür alan + taşma payı DOM'da.
    expect(container.querySelectorAll(".tile").length).toBeLessThan(400);
  });

  it("görünür alanı çekirdeğe önceliklendirir", async () => {
    const { prioritize } = setup();
    await waitFor(() => expect(prioritize).toHaveBeenCalled());
    const ids = prioritize.mock.calls[0]?.[0] ?? [];
    expect(ids.length).toBeGreaterThan(0);
    expect(ids[0]).toBe(1);
  });

  it("klavye: I denetçiyi gizler, Space Loupe'u açar, Esc kapatır, 3 puan verir", async () => {
    const { container } = setup();
    await waitFor(() => expect(container.querySelector(".tile.selected")).toBeTruthy());
    await fireEvent.keyDown(window, { key: "i" });
    expect(screen.queryByRole("complementary", { name: "Denetçi" })).toBeNull();
    await fireEvent.keyDown(window, { key: " " });
    expect(screen.getByRole("dialog", { name: "Loupe" })).toBeTruthy();
    await fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Loupe" })).toBeNull();
    await fireEvent.keyDown(window, { key: "3" });
    await waitFor(() => expect(container.querySelector(".tile.selected [aria-label='3 yıldız']")).toBeTruthy());
  });

  it("çift tıklama Loupe'u açar", async () => {
    const { container } = setup();
    await waitFor(() => expect(container.querySelector(".tile[data-idx='2']")).toBeTruthy());
    const tile = container.querySelector(".tile[data-idx='2']") as HTMLElement;
    await fireEvent.dblClick(tile);
    expect(screen.getByRole("dialog", { name: "Loupe" })).toBeTruthy();
    expect(container.querySelector(".tile.selected")?.getAttribute("data-idx")).toBe("2");
  });
});

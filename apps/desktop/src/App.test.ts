import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import App from "./App.svelte";
import { MockBackend } from "./lib/api/mock";
import { LayoutEngine } from "./lib/layout/engine";

// jsdom düzen hesaplamaz: ızgara kabına gerçekçi bir boyut verilir.
beforeAll(() => {
  // jsdom'da Web Animations yok; Svelte geçişleri (slayt gösterisi) anında biter.
  Element.prototype.animate ??= function (this: Element) {
    const a = { onfinish: null as (() => void) | null, cancel: () => {}, finished: Promise.resolve(), currentTime: 0 };
    queueMicrotask(() => a.onfinish?.());
    return a as unknown as Animation;
  };
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
    expect(screen.getAllByText("Tüm Fotoğraflar").length).toBe(2);
    expect(screen.getByRole("heading", { name: "Tüm Fotoğraflar" })).toBeTruthy();
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

  it("görünüm seçenekleri sıralamayı ve gruplamayı değiştirir, seçimi saklar", async () => {
    const { backend, container } = setup();
    const slice = vi.spyOn(backend, "timelineSlice");
    await waitFor(() => expect(container.querySelectorAll(".tile[data-idx]").length).toBeGreaterThan(10));
    await fireEvent.click(screen.getByRole("button", { name: "Çekim tarihi" }));
    const menu = screen.getByRole("dialog", { name: "Görünüm seçenekleri" });
    await fireEvent.click(within(menu).getByRole("button", { name: "Yıl" }));
    await waitFor(() => expect(slice).toHaveBeenLastCalledWith(expect.objectContaining({ sort: "taken", grouping: "year" })));
    await fireEvent.click(within(menu).getByRole("button", { name: "Puan" }));
    await waitFor(() => expect(slice).toHaveBeenLastCalledWith(expect.objectContaining({ sort: "rating", dir: "desc" })));
    expect(within(menu).getByRole("button", { name: "Gün" }).closest("fieldset")?.disabled).toBe(true);
    expect(JSON.parse(localStorage.getItem("loupe.arrange") ?? "{}")).toMatchObject({ sort: "rating", grouping: "year" });
    await fireEvent.click(within(menu).getByRole("button", { name: "Büyük" }));
    expect(within(menu).getByRole("button", { name: "Büyük" }).getAttribute("aria-pressed")).toBe("true");
    localStorage.removeItem("loupe.arrange");
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

  it("sağ tık menüsü: favoriye ekler, bulunduğu klasörü açar; çoklu seçimde hepsine uygular", async () => {
    const { backend, container } = setup();
    await waitFor(() => expect(container.querySelector(".tile[data-idx='4']")).toBeTruthy());
    const tile = (i: number) => container.querySelector(`.tile[data-idx='${i}']`) as HTMLElement;
    const before = backend.items[4]?.favorite;
    await fireEvent.contextMenu(tile(4), { clientX: 100, clientY: 100 });
    expect(tile(4).classList.contains("selected")).toBe(true);
    let menu = screen.getByRole("menu", { name: "Fotoğraf menüsü" });
    await fireEvent.click(within(menu).getByRole("menuitem", { name: /Bulunduğu klasörü aç/ }));
    expect(screen.queryByRole("menu")).toBeNull();
    await waitFor(() => expect(backend.revealed).toEqual([backend.items[4]?.id]));

    await fireEvent.contextMenu(tile(4), { clientX: 100, clientY: 100 });
    menu = screen.getByRole("menu");
    await fireEvent.click(within(menu).getByRole("menuitem", { name: before ? /Favorilerden çıkar/ : /Favorilere ekle/ }));
    await waitFor(() => expect(backend.items[4]?.favorite).toBe(!before));

    // Ctrl ile iki karo işaretlenir; sağ tık seçimi korur, favori ikisine de uygulanır.
    await fireEvent.click(tile(1));
    await fireEvent.click(tile(2), { ctrlKey: true });
    await fireEvent.contextMenu(tile(2), { clientX: 100, clientY: 100 });
    menu = screen.getByRole("menu");
    expect(menu.textContent).toContain("2 öğe seçili");
    const on = !(backend.items[1]?.favorite && backend.items[2]?.favorite);
    await fireEvent.click(within(menu).getByRole("menuitem", { name: on ? /Favorilere ekle/ : /Favorilerden çıkar/ }));
    await waitFor(() => expect([backend.items[1]?.favorite, backend.items[2]?.favorite]).toEqual([on, on]));
    // Esc menüyü kapatır.
    await fireEvent.contextMenu(tile(2), { clientX: 100, clientY: 100 });
    await fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("F tuşu ve Loupe'taki kalp düğmesi favoriyi değiştirir", async () => {
    const { backend, container } = setup();
    await waitFor(() => expect(container.querySelector(".tile.selected")).toBeTruthy());
    const idx = Number(container.querySelector(".tile.selected")?.getAttribute("data-idx"));
    const first = backend.items[idx]?.favorite ?? false;
    await fireEvent.keyDown(window, { key: "f" });
    await waitFor(() => expect(backend.items[idx]?.favorite).toBe(!first));
    await fireEvent.keyDown(window, { key: " " });
    const loupe = screen.getByRole("dialog", { name: "Loupe" });
    await fireEvent.click(within(loupe).getByRole("button", { name: first ? "Favorilere ekle" : "Favorilerden çıkar" }));
    await waitFor(() => expect(backend.items[idx]?.favorite).toBe(first));
  });

  it("slayt gösterisi: S başlatır, tam ekran olur, süre dolunca ilerler, oklar ve Esc", async () => {
    const { backend, container } = setup();
    const full = vi.spyOn(backend, "setFullscreen");
    await waitFor(() => expect(container.querySelector(".tile.selected")).toBeTruthy());
    const selected = () => container.querySelector(".tile.selected")?.getAttribute("data-idx");
    const start = selected();
    vi.useFakeTimers();
    try {
      await fireEvent.keyDown(window, { key: "s" });
      const show = screen.getByRole("dialog", { name: "Slayt gösterisi" });
      expect(full).toHaveBeenLastCalledWith(true);
      expect(show.textContent).toContain("1 / 400");
      await vi.advanceTimersByTimeAsync(5_100);
      // Denetimler fare durunca gizlenir, kıpırdayınca görünür.
      expect(within(show).queryByRole("button", { name: "Duraklat" })).toBeNull();
      await fireEvent.mouseMove(show);
      expect(show.textContent).toContain("2 / 400");
      // Boşluk duraklatır: süre dolsa da ilerlemez.
      await fireEvent.keyDown(window, { key: " " });
      await vi.advanceTimersByTimeAsync(20_000);
      expect(show.textContent).toContain("2 / 400");
      await fireEvent.keyDown(window, { key: "ArrowLeft" });
      await fireEvent.keyDown(window, { key: "ArrowLeft" });
      expect(show.textContent).toContain("400 / 400");
      await fireEvent.click(within(show).getByRole("button", { name: "3 sn" }));
      expect(JSON.parse(localStorage.getItem("loupe.slideshow") ?? "{}").seconds).toBe(3);
      await fireEvent.keyDown(window, { key: "Escape" });
      expect(screen.queryByRole("dialog", { name: "Slayt gösterisi" })).toBeNull();
      expect(full).toHaveBeenLastCalledWith(false);
      expect(selected()).not.toBe(start);
    } finally {
      vi.useRealTimers();
      localStorage.removeItem("loupe.slideshow");
    }
  });

  it("Videolar bölümü yalnızca videoları süreleriyle gösterir", async () => {
    const { backend, container } = setup();
    const videos = backend.items.filter((i) => i.video).length;
    await waitFor(() => expect(screen.getByRole("button", { name: /Videolar/ })).toBeTruthy());
    await fireEvent.click(screen.getByRole("button", { name: /Videolar/ }));
    await waitFor(() => expect(screen.getByRole("heading", { name: "Videolar" })).toBeTruthy());
    await waitFor(() => expect(screen.getByRole("contentinfo", { name: "Durum" }).textContent).toContain(`${videos}`));
    await waitFor(() => expect(container.querySelector(".tile[data-idx='0']")?.textContent).toMatch(/\d+:\d\d/));
  });

  it("albüm: sağ tıkla yeni albüm oluşturur, ekler, albümden çıkarır, siler", async () => {
    const { backend, container } = setup();
    await waitFor(() => expect(container.querySelector(".tile[data-idx='3']")).toBeTruthy());
    const tile = (i: number) => container.querySelector(`.tile[data-idx='${i}']`) as HTMLElement;
    await fireEvent.click(tile(2));
    await fireEvent.click(tile(3), { ctrlKey: true });
    await fireEvent.contextMenu(tile(3), { clientX: 50, clientY: 50 });
    await fireEvent.click(within(screen.getByRole("menu")).getByRole("menuitem", { name: /Albüme ekle/ }));
    const picker = screen.getByRole("dialog", { name: "Albüme ekle" });
    expect(picker.textContent).toContain("2 öğeyi albüme ekle");
    await fireEvent.input(within(picker).getByRole("textbox", { name: "Albüm adı" }), { target: { value: "Aile" } });
    await fireEvent.keyDown(within(picker).getByRole("textbox", { name: "Albüm adı" }), { key: "Enter" });
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Albüme ekle" })).toBeNull());
    const side = await backend.sidebar();
    const aile = side.albums.find((a) => a.name === "Aile");
    expect(aile?.mediaCount).toBe(2);

    // Kenar çubuğundan albüm açılır; sağ tık "Albümden çıkar" sunar.
    await waitFor(() => expect(screen.getByRole("button", { name: /^Aile/ })).toBeTruthy());
    await fireEvent.click(screen.getByRole("button", { name: /^Aile/ }));
    await waitFor(() => expect(screen.getByRole("heading", { name: "Aile" })).toBeTruthy());
    await waitFor(() => expect(container.querySelectorAll(".tile[data-idx]:not([style*='display: none'])").length).toBe(2));
    await fireEvent.contextMenu(tile(0), { clientX: 50, clientY: 50 });
    await fireEvent.click(within(screen.getByRole("menu")).getByRole("menuitem", { name: "Albümden çıkar" }));
    await waitFor(async () => expect((await backend.sidebar()).albums[0]?.mediaCount).toBe(1));

    // Yeniden adlandır ve sil (onaylı); fotoğraflar katalogda kalır.
    await fireEvent.click(screen.getByRole("button", { name: "Yeniden adlandır" }));
    const field = screen.getByRole("textbox", { name: "Albüm adı" });
    await fireEvent.input(field, { target: { value: "Ailem" } });
    await fireEvent.keyDown(field, { key: "Enter" });
    await waitFor(() => expect(screen.getByRole("heading", { name: "Ailem" })).toBeTruthy());
    await fireEvent.click(screen.getByRole("button", { name: "Albümü sil" }));
    await fireEvent.click(screen.getByRole("button", { name: "Sil" }));
    await waitFor(async () => expect((await backend.sidebar()).albums).toEqual([]));
    expect(backend.items.length).toBe(400);
  });
});

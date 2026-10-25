import { describe, expect, it, vi } from "vitest";

import { MockBackend } from "../api/mock";
import { ROW_HEIGHTS } from "../design/layout";
import { justify } from "../layout/justified";
import { Catalog, FIRST_PAGE } from "./catalog.svelte";

async function opened(n = 1500): Promise<{ catalog: Catalog; backend: MockBackend }> {
  const backend = new MockBackend(n);
  const catalog = new Catalog(backend);
  await catalog.open({ kind: "all" });
  return { catalog, backend };
}

describe("Catalog", () => {
  it("tüm sayfaları sırayla yükler ve ilk öğeyi seçer", async () => {
    const backend = new MockBackend(FIRST_PAGE * 2 + 17);
    const spy = vi.spyOn(backend, "timelineSlice");
    const catalog = new Catalog(backend);
    await catalog.open({ kind: "all" });
    expect(catalog.total).toBe(FIRST_PAGE * 2 + 17);
    expect(catalog.loaded).toBe(catalog.total);
    expect(catalog.items.every((i) => i !== undefined)).toBe(true);
    expect(spy.mock.calls[0]?.[0]).toMatchObject({ offset: 0, limit: FIRST_PAGE });
    expect(spy.mock.calls[1]?.[0]).toMatchObject({ offset: FIRST_PAGE });
    expect(catalog.selected).toBe(0);
    expect(catalog.aspects[5]).toBeCloseTo(catalog.items[5]?.aspect ?? 0);
  });

  it("yeni görünüm eski yüklemeyi geçersiz kılar", async () => {
    const { catalog } = await opened();
    const a = catalog.open({ kind: "favorites" });
    const b = catalog.open({ kind: "all" });
    await Promise.all([a, b]);
    expect(catalog.view).toEqual({ kind: "all" });
    expect(catalog.total).toBe(1500);
  });

  it("puan, etiket ve favori iyimser güncellenir ve çekirdeğe gider", async () => {
    const { catalog, backend } = await opened();
    const spy = vi.spyOn(backend, "setRatingLabelFavorite");
    const id = catalog.selectedItem?.id ?? -1;
    const rev = catalog.rev;
    catalog.dispatch({ type: "rate", value: 5 });
    expect(catalog.selectedItem?.rating).toBe(5);
    expect(catalog.rev).toBeGreaterThan(rev);
    await vi.waitFor(() => expect(spy).toHaveBeenCalledWith({ ids: [id], rating: 5 }));

    const before = catalog.selectedItem?.label ?? null;
    catalog.dispatch({ type: "label", label: "green" });
    expect(catalog.selectedItem?.label).toBe(before === "green" ? null : "green");
    const fav = catalog.selectedItem?.favorite;
    catalog.dispatch({ type: "favorite" });
    expect(catalog.selectedItem?.favorite).toBe(!fav);
  });

  it("çekirdek hatasında değer geri alınır ve hata gösterilir", async () => {
    const { catalog, backend } = await opened();
    vi.spyOn(backend, "setRatingLabelFavorite").mockRejectedValue(new Error("disk salt okunur"));
    const old = catalog.selectedItem?.rating;
    await catalog.cull({ ids: [catalog.selectedItem?.id ?? -1], rating: 3 });
    expect(catalog.selectedItem?.rating).toBe(old);
    expect(catalog.error).toBe("disk salt okunur");
  });

  it("ok tuşları düzeni kullanır; düzen yoksa ←/→ sıra", async () => {
    const { catalog } = await opened(200);
    catalog.dispatch({ type: "move", dir: "right" });
    expect(catalog.selected).toBe(1);
    catalog.dispatch({ type: "move", dir: "down" });
    expect(catalog.selected).toBe(1);
    catalog.layout = justify({
      aspects: catalog.aspects,
      sections: catalog.sections,
      width: 1000,
      targetHeight: catalog.targetRowHeight,
    });
    catalog.dispatch({ type: "move", dir: "down" });
    expect(catalog.selected).toBeGreaterThan(1);
    catalog.dispatch({ type: "move", dir: "left" });
    catalog.dispatch({ type: "move", dir: "up" });
    expect(catalog.selected).toBeLessThan(catalog.layout.rowFirst[1] as number);
  });

  it("I denetçiyi, Space Loupe'u açıp kapatır; Esc yalnızca Loupe'ta yutulur", async () => {
    const { catalog } = await opened(50);
    expect(catalog.inspectorOpen).toBe(true);
    catalog.dispatch({ type: "inspector" });
    expect(catalog.inspectorOpen).toBe(false);
    expect(catalog.dispatch({ type: "close" })).toBe(false);
    catalog.dispatch({ type: "loupe" });
    expect(catalog.loupeOpen).toBe(true);
    catalog.dispatch({ type: "loupeZoom" });
    expect(catalog.loupeActual).toBe(true);
    expect(catalog.dispatch({ type: "close" })).toBe(true);
    expect(catalog.loupeOpen).toBe(false);
    expect(catalog.loupeActual).toBe(false);
  });

  it("yakınlaştırma sınırlarda durur", async () => {
    const { catalog } = await opened(10);
    for (let i = 0; i < 10; i++) catalog.dispatch({ type: "zoom", delta: 1 });
    expect(catalog.targetRowHeight).toBe(ROW_HEIGHTS[ROW_HEIGHTS.length - 1]);
    for (let i = 0; i < 10; i++) catalog.dispatch({ type: "zoom", delta: -1 });
    expect(catalog.targetRowHeight).toBe(ROW_HEIGHTS[0]);
  });

  it("denetçi ayrıntısı seçimi izler", async () => {
    const { catalog } = await opened(30);
    catalog.select(4);
    await vi.waitFor(() => expect(catalog.detail?.id).toBe(catalog.items[4]?.id));
    expect(catalog.detail?.aperture).toBe("f/2.8");
  });

  it("boş görünüm", async () => {
    const backend = new MockBackend(0);
    const catalog = new Catalog(backend);
    await catalog.open({ kind: "all" });
    expect(catalog.opened).toBe(true);
    expect(catalog.total).toBe(0);
    expect(catalog.selected).toBe(-1);
    catalog.dispatch({ type: "loupe" });
    expect(catalog.loupeOpen).toBe(false);
  });
});

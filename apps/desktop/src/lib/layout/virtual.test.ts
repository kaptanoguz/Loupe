import { describe, expect, it } from "vitest";

import { justify } from "./justified";
import { ScrollSpeed, lastAtOrBefore, neighbor, revealTop, thumbClass, visibleRange } from "./virtual";

const aspects = Float32Array.from({ length: 2000 }, (_, i) => [1.5, 0.667, 1.333, 1][i % 4] as number);
const layout = justify({
  aspects,
  sections: Array.from({ length: 50 }, (_, s) => ({ start: s * 40, count: 40 })),
  width: 1200,
  targetHeight: 180,
});

describe("virtual", () => {
  it("ikili arama", () => {
    const tops = Float32Array.from([0, 10, 20, 30]);
    expect(lastAtOrBefore(tops, -5)).toBe(0);
    expect(lastAtOrBefore(tops, 0)).toBe(0);
    expect(lastAtOrBefore(tops, 15)).toBe(1);
    expect(lastAtOrBefore(tops, 30)).toBe(3);
    expect(lastAtOrBefore(tops, 1e9)).toBe(3);
  });

  it("görünür aralık ekranı ve taşma payını kapsar, fazlasını değil", () => {
    const vh = 800;
    const top = layout.height / 2;
    const r = visibleRange(layout, top, vh, 1.5);
    for (let i = r.start; i < r.end; i++) {
      const y = layout.rects[i * 4 + 1] as number;
      const h = layout.rects[i * 4 + 3] as number;
      expect(y + h).toBeGreaterThanOrEqual(top - 1.5 * vh - 300);
      expect(y).toBeLessThanOrEqual(top + vh + 1.5 * vh);
    }
    // Ekrandaki her öğe aralıkta.
    for (let i = 0; i < 2000; i++) {
      const y = layout.rects[i * 4 + 1] as number;
      const h = layout.rects[i * 4 + 3] as number;
      if (y < top + vh && y + h > top) {
        expect(i).toBeGreaterThanOrEqual(r.start);
        expect(i).toBeLessThan(r.end);
      }
    }
    expect(r.end - r.start).toBeLessThan(400);
    expect(r.headers.length).toBeGreaterThan(0);
  });

  it("boyut sınıfı: yükseklik × dpr", () => {
    expect(thumbClass(120, 2)).toBe("sm");
    expect(thumbClass(180, 2)).toBe("md");
    expect(thumbClass(380, 1)).toBe("md");
    expect(thumbClass(380, 2.5)).toBe("lg");
  });

  it("ok tuşları: ←/→ sıra, ↑/↓ en yakın yatay merkez", () => {
    expect(neighbor(layout, 0, "left")).toBe(0);
    expect(neighbor(layout, 0, "right")).toBe(1);
    expect(neighbor(layout, 1999, "right")).toBe(1999);
    expect(neighbor(layout, 0, "up")).toBe(0);
    const down = neighbor(layout, 0, "down");
    const cx = (layout.rects[2] as number) / 2;
    const [dx, , dw] = [layout.rects[down * 4], 0, layout.rects[down * 4 + 2]] as number[];
    expect(dx).toBeLessThanOrEqual(cx);
    expect((dx as number) + (dw as number)).toBeGreaterThanOrEqual(cx);
    expect(neighbor(layout, down, "up")).toBe(0);
  });

  it("seçimi görünür alana getirir", () => {
    expect(revealTop(layout, 0, 0, 800)).toBeNull();
    const far = 1500;
    const t = revealTop(layout, far, 0, 800);
    const y = layout.rects[far * 4 + 1] as number;
    const h = layout.rects[far * 4 + 3] as number;
    expect(t).toBe(y + h - 800);
    // Yukarı giderken bölüm başlığı da görünür.
    const up = revealTop(layout, 40, layout.height, 800);
    expect(up).toBe((layout.headerTops[1] as number));
  });

  it("hızlı kaydırma algısı ve sönüm", () => {
    const s = new ScrollSpeed();
    s.sample(0, 0, 800);
    s.sample(800 * 2, 100, 800); // 20 ekran/sn
    expect(s.fast).toBe(true);
    s.reset();
    expect(s.fast).toBe(false);
    s.sample(0, 1000, 800);
    s.sample(40, 1100, 800); // 0.5 ekran/sn
    expect(s.fast).toBe(false);
  });
});

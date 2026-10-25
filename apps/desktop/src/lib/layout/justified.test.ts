import { describe, expect, it } from "vitest";

import { GUTTER, ROW_STRETCH_MAX, ROW_STRETCH_MIN, SECTION_GAP, SECTION_HEADER } from "../design/layout";
import { justify } from "./justified";

const W = 1000;
const HT = 180;

function rowsOf(l: ReturnType<typeof justify>): { first: number; end: number; top: number; h: number }[] {
  return Array.from(l.rowTops, (top, r) => ({
    first: l.rowFirst[r] as number,
    end: l.rowFirst[r + 1] as number,
    top,
    h: l.rowHeights[r] as number,
  }));
}

function rect(l: ReturnType<typeof justify>, i: number): [number, number, number, number] {
  return [l.rects[i * 4], l.rects[i * 4 + 1], l.rects[i * 4 + 2], l.rects[i * 4 + 3]] as [number, number, number, number];
}

describe("justify", () => {
  it("dolu satırlar tam W'de biter ve hedef aralığında kalır", () => {
    const aspects = Float32Array.from({ length: 300 }, (_, i) => [1.5, 0.667, 1.333, 1, 1.778][i % 5] as number);
    const l = justify({ aspects, sections: [{ start: 0, count: 300 }], width: W, targetHeight: HT });
    const rows = rowsOf(l);
    expect(rows.length).toBeGreaterThan(10);
    for (const r of rows.slice(0, -1)) {
      const last = rect(l, r.end - 1);
      expect(last[0] + last[2]).toBe(W);
      expect(r.h).toBeGreaterThanOrEqual(Math.round(ROW_STRETCH_MIN * HT));
      expect(r.h).toBeLessThanOrEqual(Math.round(ROW_STRETCH_MAX * HT));
      // Satır içi oluk tam GUTTER, tüm karolar aynı yükseklikte.
      for (let i = r.first + 1; i < r.end; i++) {
        const [px, , pw] = rect(l, i - 1);
        const [x, y, , h] = rect(l, i);
        expect(x - (px + pw)).toBe(GUTTER);
        expect(y).toBe(r.top);
        expect(h).toBe(r.h);
      }
    }
  });

  it("son satır esnetilmez", () => {
    const aspects = Float32Array.from([1.5, 1.5]);
    const l = justify({ aspects, sections: [{ start: 0, count: 2 }], width: W, targetHeight: HT });
    expect(l.rowTops.length).toBe(1);
    expect(l.rowHeights[0]).toBe(HT);
    const [, , w] = rect(l, 1);
    expect(rect(l, 1)[0] + w).toBeLessThan(W);
  });

  it("panorama kendi satırına geçer", () => {
    const aspects = Float32Array.from([1.5, 1.5, 12, 1.5, 1.5]);
    const l = justify({ aspects, sections: [{ start: 0, count: 5 }], width: W, targetHeight: HT });
    const rows = rowsOf(l);
    const pano = rows.find((r) => r.first === 2);
    expect(pano?.end).toBe(3);
    const [x, , w, h] = rect(l, 2);
    expect(x).toBe(0);
    expect(w).toBe(W);
    expect(h).toBe(Math.round(W / 12));
  });

  it("bölüm başlıkları ve boşluklar", () => {
    const aspects = new Float32Array(10).fill(1.5);
    const l = justify({
      aspects,
      sections: [
        { start: 0, count: 4 },
        { start: 4, count: 6 },
      ],
      width: W,
      targetHeight: HT,
    });
    expect(l.headerTops[0]).toBe(0);
    expect(rect(l, 0)[1]).toBe(SECTION_HEADER);
    const rows = rowsOf(l);
    const lastOfFirst = rows.filter((r) => r.first < 4).at(-1);
    expect(lastOfFirst).toBeDefined();
    if (!lastOfFirst) return;
    expect(l.headerTops[1]).toBe(lastOfFirst.top + lastOfFirst.h + SECTION_GAP);
    expect(rect(l, 4)[1]).toBe((l.headerTops[1] as number) + SECTION_HEADER);
    // İkinci bölümün ilk öğesi yeni satırda başlar.
    expect(rect(l, 4)[0]).toBe(0);
  });

  it("geçersiz en/boy yedek değere düşer", () => {
    const aspects = Float32Array.from([Number.NaN, 0, -1, 1e9]);
    const l = justify({ aspects, sections: [{ start: 0, count: 4 }], width: W, targetHeight: HT });
    for (let i = 0; i < 4; i++) expect(rect(l, i)[2]).toBeGreaterThan(0);
  });

  it("boş katalog", () => {
    const l = justify({ aspects: new Float32Array(0), sections: [], width: W, targetHeight: HT });
    expect(l.height).toBe(0);
    expect(Array.from(l.rowFirst)).toEqual([0]);
  });

  it("100k öğe makul sürede", () => {
    const n = 100_000;
    const aspects = Float32Array.from({ length: n }, (_, i) => [1.5, 0.667, 1.333, 1, 3.5][(i * 7) % 5] as number);
    const sections = Array.from({ length: n / 40 }, (_, s) => ({ start: s * 40, count: 40 }));
    const t0 = performance.now();
    const l = justify({ aspects, sections, width: 1600, targetHeight: HT });
    const ms = performance.now() - t0;
    expect(l.rowFirst.at(-1)).toBe(n);
    // RFC ölçümü ~13 ms; CI makineleri ve jsdom için geniş pay.
    expect(ms).toBeLessThan(500);
  });
});

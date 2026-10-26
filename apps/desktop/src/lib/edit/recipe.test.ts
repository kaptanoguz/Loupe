import { describe, expect, it } from "vitest";

import { IDENTITY, coverScale, editedAspect, fitRatio, isIdentity, luts, normalized, rotateCrop } from "./recipe";

describe("düzenleme tarifi", () => {
  it("ton tabloları çekirdekle aynı değerleri verir (edit.rs testleri)", () => {
    const id = luts(IDENTITY);
    expect([...id[0]].every((v, i) => v === i)).toBe(true);
    let l = luts({ ...IDENTITY, brightness: 50, contrast: 30, temperature: 40 });
    expect([l[0][64], l[1][64], l[2][64]]).toEqual([86, 81, 76]);
    expect([l[0][200], l[1][200], l[2][200]]).toEqual([243, 232, 220]);
    l = luts({ ...IDENTITY, brightness: -40, contrast: -50, temperature: -60 });
    expect([l[0][128], l[1][128], l[2][128]]).toEqual([105, 114, 122]);
  });

  it("düzleştirme köşeleri kaplayacak kadar büyütür", () => {
    expect(coverScale(60, 40, 0)).toBeCloseTo(1);
    expect(coverScale(60, 40, 10)).toBeGreaterThan(1.2);
    expect(coverScale(60, 40, -10)).toBeCloseTo(coverScale(60, 40, 10));
  });

  it("döndürme ve kırpma en/boy oranını değiştirir", () => {
    expect(editedAspect(1.5, { ...IDENTITY, quarter: 1 })).toBeCloseTo(1 / 1.5);
    expect(editedAspect(2, { ...IDENTITY, crop: { x: 0.5, y: 0.5, w: 0.5, h: 0.5 } })).toBeCloseTo(2);
    expect(editedAspect(2, { ...IDENTITY, crop: { x: 0, y: 0, w: 0.25, h: 1 } })).toBeCloseTo(0.5);
  });

  it("kırpma 90° döndürmede çerçeveyle döner, dört dönüşte yerine gelir", () => {
    const c = { x: 0.1, y: 0.2, w: 0.3, h: 0.4 };
    expect(rotateCrop(c, 1)).toEqual({ x: 1 - 0.2 - 0.4, y: 0.1, w: 0.4, h: 0.3 });
    let r = c;
    for (let i = 0; i < 4; i++) r = rotateCrop(r, 1) ?? r;
    expect(r.x).toBeCloseTo(c.x);
    expect(r.y).toBeCloseTo(c.y);
    const back = rotateCrop(rotateCrop(c, 1), -1);
    expect(back?.w).toBeCloseTo(c.w);
    expect(back?.x).toBeCloseTo(c.x);
  });

  it("oran seçimi merkezde, sınırların içinde kalır", () => {
    const sq = fitRatio({ x: 0, y: 0, w: 1, h: 1 }, 1, 1.5);
    // 3:2 çerçevede kare: genişlik yüksekliğin 2/3'ü.
    expect(sq.h).toBeCloseTo(1);
    expect(sq.w).toBeCloseTo(1 / 1.5);
    expect(sq.x).toBeCloseTo((1 - 1 / 1.5) / 2);
  });

  it("tam çerçeve kırpma düzenleme sayılmaz", () => {
    expect(isIdentity({ ...IDENTITY, crop: { x: 0, y: 0, w: 1, h: 1 } })).toBe(true);
    expect(normalized({ ...IDENTITY, quarter: -1, straighten: 80, crop: { x: 0, y: 0, w: 1, h: 1 } })).toEqual({
      ...IDENTITY,
      quarter: 3,
      straighten: 45,
    });
  });
});

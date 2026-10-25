import { describe, expect, it } from "vitest";

import { faceCrop } from "./face";

describe("faceCrop", () => {
  it("küçük yüzü büyütür ve ortalar, kap hep dolar", () => {
    const c = faceCrop({ aspect: 1.5, bbox: { x: 0.45, y: 0.3, w: 0.1, h: 0.15 } }, 96, 120);
    expect(c.width).toBeGreaterThan(96);
    expect(c.height).toBeCloseTo(c.width / 1.5, 6);
    // Kap dolu: görüntü kenarları kabın dışında ya da sınırında.
    expect(c.left).toBeLessThanOrEqual(0);
    expect(c.top).toBeLessThanOrEqual(0);
    expect(c.left + c.width).toBeGreaterThanOrEqual(96 - 1e-9);
    expect(c.top + c.height).toBeGreaterThanOrEqual(120 - 1e-9);
    // Yüzün merkezi kabın merkezinde.
    expect(c.left + 0.5 * c.width).toBeCloseTo(48, 6);
    expect(c.top + 0.375 * c.height).toBeCloseTo(60, 6);
  });

  it("kenardaki yüzde boşluk bırakmaz (kaydırma sınırlanır)", () => {
    const c = faceCrop({ aspect: 1, bbox: { x: 0, y: 0, w: 0.2, h: 0.2 } }, 100, 100);
    expect(c.left).toBe(0);
    expect(c.top).toBe(0);
  });

  it("büyük yüzde görüntü kaptan küçük olmaz", () => {
    const c = faceCrop({ aspect: 0.75, bbox: { x: 0.05, y: 0.05, w: 0.9, h: 0.9 } }, 96, 120);
    expect(c.width).toBeGreaterThanOrEqual(96);
    expect(c.height).toBeGreaterThanOrEqual(120 - 1e-9);
  });
});

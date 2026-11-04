import { describe, expect, it } from "vitest";

import { MAX_K, WORLD, clamp, cluster, fit, markerSize, minK, pan, project, tiles, toScreen, zoomAt } from "./mercator";

describe("mercator", () => {
  it("projects the equator and the antimeridian to the world's edges", () => {
    expect(project(0, 0)).toEqual({ x: WORLD / 2, y: WORLD / 2 });
    expect(project(0, -180).x).toBe(0);
    expect(project(0, 180).x).toBe(WORLD);
    expect(project(90, 0).y).toBeCloseTo(0, 3);
    expect(project(-90, 0).y).toBeCloseTo(WORLD, 3);
    // Göreme kuzey yarıkürede ve başlangıç meridyeninin doğusunda.
    const g = project(38.64, 34.83);
    expect(g.x).toBeGreaterThan(WORLD / 2);
    expect(g.y).toBeLessThan(WORLD / 2);
  });

  it("keeps the zoom in range and the world on screen", () => {
    const v = clamp({ x: -500, y: 99_999, k: 0 }, 800, 600);
    expect(v.k).toBe(minK(800, 600));
    expect(v).toEqual({ k: 600 / WORLD, x: WORLD / 2, y: WORLD / 2 });
    const deep = clamp({ x: 0, y: 0, k: 1e9 }, 800, 600);
    expect(deep.k).toBe(MAX_K);
    expect(deep.x).toBeCloseTo(400 / MAX_K);
  });

  it("fits points with padding and opens a single place at city level", () => {
    const a = project(41.01, 28.95);
    const b = project(36.88, 30.7);
    const v = fit([a, b], 800, 600);
    for (const p of [a, b]) {
      const s = toScreen(v, p, 800, 600);
      expect(s.x).toBeGreaterThanOrEqual(47);
      expect(s.x).toBeLessThanOrEqual(753);
      expect(s.y).toBeGreaterThanOrEqual(47);
      expect(s.y).toBeLessThanOrEqual(553);
    }
    const one = fit([a], 800, 600);
    expect(one.k).toBe(128);
    expect(toScreen(one, a, 800, 600)).toEqual({ x: 400, y: 300 });
    expect(fit([], 800, 600).k).toBe(minK(800, 600));
  });

  it("zooms around the cursor and pans by screen pixels", () => {
    const v = { x: 2048, y: 2048, k: 1 };
    const z = zoomAt(v, 2, 600, 300, 800, 600);
    // İmlecin altındaki dünya noktası yerinde kalır.
    expect(z.k).toBe(2);
    expect(z.x + (600 - 400) / z.k).toBeCloseTo(v.x + (600 - 400) / v.k);
    const p = pan(z, 100, -50, 800, 600);
    expect(p.x).toBeCloseTo(z.x - 50);
    expect(p.y).toBeCloseTo(z.y + 25);
  });

  it("covers the viewport with tiles of about 256 px", () => {
    const whole = tiles({ x: 2048, y: 2048, k: 0.0625 }, 256, 256);
    expect(whole).toEqual([{ z: 0, x: 0, y: 0, left: 0, top: 0, size: 256 }]);
    const t = tiles({ x: 2048, y: 2048, k: 1 }, 800, 600);
    expect(new Set(t.map((x) => x.z))).toEqual(new Set([4]));
    expect(t[0]?.size).toBe(256);
    // Görünüm tamamen örtülür.
    const minLeft = Math.min(...t.map((x) => x.left));
    const maxRight = Math.max(...t.map((x) => x.left + x.size));
    expect(minLeft).toBeLessThanOrEqual(0);
    expect(maxRight).toBeGreaterThanOrEqual(800);
    for (const x of t) {
      expect(x.x).toBeGreaterThanOrEqual(0);
      expect(x.x).toBeLessThan(16);
    }
  });

  it("merges nearby pins and keeps far ones apart", () => {
    const pins = [
      { id: 1, count: 10, at: project(41.01, 28.95) },
      { id: 2, count: 30, at: project(41.02, 28.97) },
      { id: 3, count: 5, at: project(35.68, 139.77) },
    ];
    const world = cluster(pins, fit(pins.map((p) => p.at), 800, 600), 800, 600);
    expect(world.map((c) => c.members.map((m) => m.id))).toEqual([[3], [1, 2]]);
    expect(world[1]?.count).toBe(40);
    const close = cluster(pins, fit([pins[0]!.at, pins[1]!.at], 800, 600), 800, 600);
    expect(close.map((c) => c.count)).toEqual([10, 30]);
  });

  it("sizes markers by photo count", () => {
    expect(markerSize(1)).toBe(10);
    expect(markerSize(100)).toBe(20);
    expect(markerSize(1_000_000)).toBe(30);
  });
});

/**
 * Sanal kaydırma yardımcıları (RFC-001B §4.5). DOM'a dokunmaz; Grid bileşeni
 * her rAF'ta bunları çağırır.
 */
import { FAST_SCROLL_SCREENS_PER_S, OVERSCAN_SCREENS, SECTION_HEADER, THUMB_PX, type ThumbClass } from "../design/layout";
import type { JustifiedLayout } from "./justified";

/** `tops[i] <= y` olan son i (yoksa 0). Artan dizide ikili arama. */
export function lastAtOrBefore(tops: Float32Array, y: number): number {
  let lo = 0;
  let hi = tops.length - 1;
  let ans = 0;
  while (lo <= hi) {
    const mid = (lo + hi) >>> 1;
    if ((tops[mid] as number) <= y) {
      ans = mid;
      lo = mid + 1;
    } else hi = mid - 1;
  }
  return ans;
}

export interface Range {
  /** İlk öğe (dahil). */
  start: number;
  /** Son öğe (hariç). */
  end: number;
  /** Görünen bölüm başlıkları. */
  headers: number[];
}

/** Görünür alan + üstte/altta `overscan` ekran. */
export function visibleRange(
  layout: JustifiedLayout,
  scrollTop: number,
  viewport: number,
  overscan: number = OVERSCAN_SCREENS,
): Range {
  const rows = layout.rowTops.length;
  if (rows === 0) return { start: 0, end: 0, headers: [] };
  const top = scrollTop - overscan * viewport;
  const bottom = scrollTop + viewport + overscan * viewport;
  const r0 = lastAtOrBefore(layout.rowTops, top);
  const r1 = lastAtOrBefore(layout.rowTops, bottom);
  const headers: number[] = [];
  const h0 = lastAtOrBefore(layout.headerTops, top);
  for (let s = h0; s < layout.headerTops.length; s++) {
    const y = layout.headerTops[s] as number;
    if (y > bottom) break;
    if (y + SECTION_HEADER >= top) headers.push(s);
  }
  return { start: layout.rowFirst[r0] as number, end: layout.rowFirst[r1 + 1] as number, headers };
}

/** Gereken piksel = karo yüksekliği × dpr; ≤sm → sm, ≤md → md, değilse lg. */
export function thumbClass(cssHeight: number, dpr: number): ThumbClass {
  const px = cssHeight * dpr;
  if (px <= THUMB_PX.sm) return "sm";
  if (px <= THUMB_PX.md) return "md";
  return "lg";
}

/** Kaydırma hızı (ekran/sn), üstel yumuşatmalı. */
export class ScrollSpeed {
  private lastY = 0;
  private lastT = -1;
  private speed = 0;

  sample(y: number, t: number, viewport: number): number {
    if (this.lastT >= 0 && t > this.lastT && viewport > 0) {
      const inst = Math.abs(y - this.lastY) / viewport / ((t - this.lastT) / 1000);
      this.speed = this.speed * 0.6 + inst * 0.4;
    } else if (this.lastT >= 0) {
      this.speed *= 0.6;
    }
    this.lastY = y;
    this.lastT = t;
    return this.speed;
  }

  /** Kaydırma durduğunda (rAF boşta) hız sönümlenir. */
  decay(): number {
    this.speed *= 0.5;
    if (this.speed < 0.01) this.speed = 0;
    return this.speed;
  }

  /** Kaydırma durdu: bir sonraki kare tam kalitede çizilir. */
  reset(): void {
    this.speed = 0;
  }

  get fast(): boolean {
    return this.speed > FAST_SCROLL_SCREENS_PER_S;
  }
}

/** Öğenin satırı (ikili arama: rowFirst artan). */
export function rowOf(layout: JustifiedLayout, idx: number): number {
  let lo = 0;
  let hi = layout.rowTops.length - 1;
  let ans = 0;
  while (lo <= hi) {
    const mid = (lo + hi) >>> 1;
    if ((layout.rowFirst[mid] as number) <= idx) {
      ans = mid;
      lo = mid + 1;
    } else hi = mid - 1;
  }
  return ans;
}

export type Direction = "left" | "right" | "up" | "down";

/**
 * Ok tuşu komşusu. ←/→ sıradaki öğe; ↑/↓ komşu satırda yatay merkezi en
 * yakın öğe (RFC §4.6). Kenarda aynı öğe döner.
 */
export function neighbor(layout: JustifiedLayout, idx: number, dir: Direction): number {
  const n = layout.rects.length / 4;
  if (n === 0) return -1;
  if (idx < 0 || idx >= n) return 0;
  if (dir === "left") return Math.max(0, idx - 1);
  if (dir === "right") return Math.min(n - 1, idx + 1);
  const row = rowOf(layout, idx);
  const target = dir === "up" ? row - 1 : row + 1;
  if (target < 0 || target >= layout.rowTops.length) return idx;
  const cx = (layout.rects[idx * 4] as number) + (layout.rects[idx * 4 + 2] as number) / 2;
  let best = layout.rowFirst[target] as number;
  let bestD = Number.POSITIVE_INFINITY;
  for (let k = layout.rowFirst[target] as number; k < (layout.rowFirst[target + 1] as number); k++) {
    const kx = (layout.rects[k * 4] as number) + (layout.rects[k * 4 + 2] as number) / 2;
    const d = Math.abs(kx - cx);
    if (d < bestD) {
      bestD = d;
      best = k;
    }
  }
  return best;
}

/** Öğe görünmüyorsa onu gösteren en yakın scrollTop; görünüyorsa null. */
export function revealTop(layout: JustifiedLayout, idx: number, scrollTop: number, viewport: number): number | null {
  const y = layout.rects[idx * 4 + 1];
  const h = layout.rects[idx * 4 + 3];
  if (y === undefined || h === undefined) return null;
  if (y < scrollTop) {
    // Bölümün ilk satırıysa başlık da görünsün.
    const s = lastAtOrBefore(layout.headerTops, y);
    const first = layout.headerTops.length > 0 && (layout.headerTops[s] as number) + SECTION_HEADER === y;
    return Math.max(0, y - (first ? SECTION_HEADER : 0));
  }
  if (y + h > scrollTop + viewport) return y + h - viewport;
  return null;
}

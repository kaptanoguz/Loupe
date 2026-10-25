/**
 * Justified grid düzeni (RFC-001B §4.4). Saf fonksiyon: Web Worker'da ve
 * testlerde aynı kod çalışır.
 *
 * Bir satır i..j için yükseklik h = (W − g·(j−i)) / Σa (a = en/boy).
 * Açgözlü: satıra öğe eklendikçe h düşer; |ln(h/Ht)| en küçük olan kesim
 * seçilir, h ∈ [0.7·Ht, 1.5·Ht]. Tek başına bile çok basık kalan panorama
 * kendi satırına geçer: h = min(1.5·Ht, W/a). Bölümün son satırı esnetilmez
 * (h = min(Ht, h)). Kümülatif yuvarlama: dolu satırlar tam W'de biter.
 */
import { GUTTER, ROW_STRETCH_MAX, ROW_STRETCH_MIN, SECTION_GAP, SECTION_HEADER } from "../design/layout";

export interface SectionSpan {
  start: number;
  count: number;
}

export interface JustifiedInput {
  /** Öğe başına en/boy oranı (w/h). */
  aspects: Float32Array;
  sections: SectionSpan[];
  width: number;
  targetHeight: number;
  gutter?: number;
  header?: number;
  sectionGap?: number;
}

export interface JustifiedLayout {
  /** Öğe başına [x, y, w, h] (CSS px, tamsayı). */
  rects: Float32Array;
  /** Satır başına üst kenar (artan). */
  rowTops: Float32Array;
  rowHeights: Float32Array;
  /** Satırın ilk öğesi; rowFirst[rows] = toplam öğe sayısı. */
  rowFirst: Uint32Array;
  /** Bölüm başlığının üst kenarı. */
  headerTops: Float32Array;
  height: number;
  width: number;
}

const FALLBACK_ASPECT = 1.5;

function sane(a: number | undefined): number {
  return a !== undefined && Number.isFinite(a) && a > 0.05 && a < 20 ? a : FALLBACK_ASPECT;
}

export function justify(input: JustifiedInput): JustifiedLayout {
  const { aspects, sections, targetHeight: ht } = input;
  const g = input.gutter ?? GUTTER;
  const header = input.header ?? SECTION_HEADER;
  const gap = input.sectionGap ?? SECTION_GAP;
  const width = Math.max(1, Math.floor(input.width));
  const n = aspects.length;
  const rects = new Float32Array(n * 4);
  const minH = ROW_STRETCH_MIN * ht;
  const maxH = ROW_STRETCH_MAX * ht;

  const tops: number[] = [];
  const heights: number[] = [];
  const firsts: number[] = [];
  const headerTops = new Float32Array(sections.length);
  let y = 0;

  const place = (from: number, to: number, h: number, fill: boolean) => {
    const rh = Math.max(1, Math.round(h));
    let sum = 0;
    for (let k = from; k <= to; k++) sum += sane(aspects[k]);
    const avail = width - g * (to - from);
    let acc = 0;
    let x0 = 0;
    for (let k = from; k <= to; k++) {
      const a = sane(aspects[k]);
      acc += a;
      // Dolu satır: kümülatif yuvarlama, son kenar tam `width`.
      // Son satır: doğal genişlik (a·h), sağda boşluk kalır.
      const right = fill ? Math.round((acc / sum) * avail) : Math.round(acc * h);
      const w = Math.max(1, right - x0);
      const left = x0 + g * (k - from);
      const o = k * 4;
      rects[o] = left;
      rects[o + 1] = y;
      rects[o + 2] = w;
      rects[o + 3] = rh;
      x0 = right;
    }
    tops.push(y);
    heights.push(rh);
    firsts.push(from);
    y += rh + g;
  };

  sections.forEach((sec, si) => {
    if (si > 0) y += gap;
    headerTops[si] = y;
    y += header;
    const end = Math.min(n, sec.start + sec.count);
    let i = sec.start;
    const rowsBefore = tops.length;
    while (i < end) {
      const a0 = sane(aspects[i]);
      if (width / a0 < minH) {
        // Panorama: tam genişlik, kendi satırı.
        place(i, i, Math.min(maxH, width / a0), true);
        i++;
        continue;
      }
      let sum = 0;
      let best = i;
      let bestH = Number.POSITIVE_INFINITY;
      let bestCost = Number.POSITIVE_INFINITY;
      for (let j = i; j < end; j++) {
        const a = sane(aspects[j]);
        if (j > i && width / a < minH) break;
        sum += a;
        const h = (width - g * (j - i)) / sum;
        const cost = Math.abs(Math.log(h / ht));
        if (cost < bestCost) {
          best = j;
          bestH = h;
          bestCost = cost;
        }
        if (h < minH) break;
      }
      const last = best === end - 1;
      if (last && bestH > ht) place(i, best, ht, false);
      else if (!last && bestH > maxH) place(i, best, ht, false);
      else place(i, best, bestH, true);
      i = best + 1;
    }
    if (tops.length > rowsBefore) y -= g;
  });
  if (sections.length === 0) y = 0;

  firsts.push(n);
  return {
    rects,
    rowTops: Float32Array.from(tops),
    rowHeights: Float32Array.from(heights),
    rowFirst: Uint32Array.from(firsts),
    headerTops,
    height: Math.max(0, y),
    width,
  };
}

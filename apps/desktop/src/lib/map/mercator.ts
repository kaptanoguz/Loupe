/**
 * Yerler haritasının hesabı: Web Mercator izdüşümü, görünüm (kaydırma,
 * yakınlaştırma), OSM karo seçimi ve işaretlerin ekranda kümelenmesi.
 *
 * Dünya `WORLD` birimlik bir karedir (world.json bu ölçüde üretildi);
 * görünüm `k` ile birim başına piksel ve `x, y` ile ekranın ortasındaki dünya
 * noktasıdır. Bileşen yalnızca bu fonksiyonları çağırır, böylece hepsi
 * tarayıcı olmadan sınanır.
 */

export const WORLD = 4096;
/** Mercator'ün kare kaldığı enlem sınırı. */
export const MAX_LAT = 85.05112878;
const TILE = 256;
/** OSM'in sunduğu en yüksek karo düzeyi. */
export const MAX_TILE_Z = 19;
/** Dünya birimi başına en çok piksel (≈ sokak düzeyi). */
export const MAX_K = 2 ** MAX_TILE_Z / (WORLD / TILE);

export interface Point {
  x: number;
  y: number;
}

export interface Viewport extends Point {
  /** Dünya birimi başına piksel. */
  k: number;
}

export function project(lat: number, lon: number): Point {
  const l = Math.max(-MAX_LAT, Math.min(MAX_LAT, lat));
  const s = Math.sin((l * Math.PI) / 180);
  return {
    x: ((lon + 180) / 360) * WORLD,
    y: (0.5 - Math.log((1 + s) / (1 - s)) / (4 * Math.PI)) * WORLD,
  };
}

/** Dünyanın tamamı ekrana sığar; daha uzağa gidilmez. */
export function minK(w: number, h: number): number {
  return Math.min(w, h) / WORLD;
}

/** Görünümü sınırlar: yakınlık aralıkta, dünya ekrandan kaçmaz. */
export function clamp(v: Viewport, w: number, h: number): Viewport {
  const k = Math.max(minK(w, h), Math.min(MAX_K, v.k));
  const axis = (c: number, size: number): number => {
    const half = size / 2 / k;
    return half * 2 >= WORLD ? WORLD / 2 : Math.max(half, Math.min(WORLD - half, c));
  };
  return { k, x: axis(v.x, w), y: axis(v.y, h) };
}

/** Noktaları kenarlarda `pad` piksel boşlukla sığdırır; tek nokta şehir düzeyinde açılır. */
export function fit(points: Point[], w: number, h: number, pad = 48): Viewport {
  if (points.length === 0 || w <= 0 || h <= 0) return clamp({ x: WORLD / 2, y: WORLD / 2, k: 0 }, w, h);
  let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
  for (const p of points) {
    x0 = Math.min(x0, p.x);
    y0 = Math.min(y0, p.y);
    x1 = Math.max(x1, p.x);
    y1 = Math.max(y1, p.y);
  }
  const city = 2 ** 11 / (WORLD / TILE);
  const k = Math.min(city, (w - 2 * pad) / Math.max(x1 - x0, 1e-9), (h - 2 * pad) / Math.max(y1 - y0, 1e-9));
  return clamp({ x: (x0 + x1) / 2, y: (y0 + y1) / 2, k }, w, h);
}

export function toScreen(v: Viewport, p: Point, w: number, h: number): Point {
  return { x: (p.x - v.x) * v.k + w / 2, y: (p.y - v.y) * v.k + h / 2 };
}

/** Ekran noktası (`sx, sy`) yerinde kalacak biçimde `factor` kat yakınlaştırır. */
export function zoomAt(v: Viewport, factor: number, sx: number, sy: number, w: number, h: number): Viewport {
  const k = Math.max(minK(w, h), Math.min(MAX_K, v.k * factor));
  const wx = v.x + (sx - w / 2) / v.k;
  const wy = v.y + (sy - h / 2) / v.k;
  return clamp({ k, x: wx - (sx - w / 2) / k, y: wy - (sy - h / 2) / k }, w, h);
}

export function pan(v: Viewport, dx: number, dy: number, w: number, h: number): Viewport {
  return clamp({ ...v, x: v.x - dx / v.k, y: v.y - dy / v.k }, w, h);
}

export interface Tile {
  z: number;
  x: number;
  y: number;
  /** Ekrandaki sol üst köşe ve kenar (piksel). */
  left: number;
  top: number;
  size: number;
}

/** Görünümü örten OSM karoları. Düzey, karoların ~256 px görüneceği biçimde seçilir. */
export function tiles(v: Viewport, w: number, h: number): Tile[] {
  const z = Math.max(0, Math.min(MAX_TILE_Z, Math.round(Math.log2((WORLD * v.k) / TILE))));
  const n = 2 ** z;
  const unit = WORLD / n;
  const size = unit * v.k;
  const tl = { x: v.x - w / 2 / v.k, y: v.y - h / 2 / v.k };
  const [c0, r0] = [Math.max(0, Math.floor(tl.x / unit)), Math.max(0, Math.floor(tl.y / unit))];
  const [c1, r1] = [Math.min(n - 1, Math.floor((tl.x + w / v.k) / unit)), Math.min(n - 1, Math.floor((tl.y + h / v.k) / unit))];
  const out: Tile[] = [];
  for (let y = r0; y <= r1; y++) {
    for (let x = c0; x <= c1; x++) {
      out.push({ z, x, y, left: (x * unit - tl.x) * v.k, top: (y * unit - tl.y) * v.k, size });
    }
  }
  return out;
}

export interface Pin {
  id: number;
  count: number;
  at: Point;
}

export interface Cluster<P extends Pin> {
  /** Ekran konumu: üyelerin fotoğraf sayısıyla ağırlıklı ortası. */
  x: number;
  y: number;
  count: number;
  members: P[];
}

/**
 * Ekranda `cell` pikselden yakın işaretler tek kümede birleşir; böylece
 * dünya görünümünde yüzlerce yer okunabilir kalır. Küme sırası: en kalabalık
 * en sonda (en üstte çizilir).
 */
export function cluster<P extends Pin>(pins: P[], v: Viewport, w: number, h: number, cell = 36): Cluster<P>[] {
  const cells = new Map<string, Cluster<P>>();
  for (const p of pins) {
    const s = toScreen(v, p.at, w, h);
    if (s.x < -cell || s.y < -cell || s.x > w + cell || s.y > h + cell) continue;
    const key = `${Math.floor(s.x / cell)}:${Math.floor(s.y / cell)}`;
    const c = cells.get(key);
    if (c) {
      const total = c.count + p.count;
      c.x = (c.x * c.count + s.x * p.count) / total;
      c.y = (c.y * c.count + s.y * p.count) / total;
      c.count = total;
      c.members.push(p);
    } else {
      cells.set(key, { x: s.x, y: s.y, count: p.count, members: [p] });
    }
  }
  return [...cells.values()].sort((a, b) => a.count - b.count);
}

/** İşaret kenarı (px): fotoğraf sayısının logaritmasıyla 10–30 arası. */
export function markerSize(count: number): number {
  return Math.round(Math.min(30, 10 + 5 * Math.log10(Math.max(1, count))));
}

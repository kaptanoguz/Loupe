/**
 * Düzenleme tarifi ve önizleme matematiği. Çekirdekteki
 * `crates/loupe-media/src/edit.rs` ile birebir aynı formüller: önizleme ile
 * kaydedilen / dışa aktarılan görüntü aynı görünür.
 */
import type { EditCrop, EditRecipe } from "../api/types";

export const IDENTITY: EditRecipe = { quarter: 0, straighten: 0, crop: null, brightness: 0, contrast: 0, temperature: 0 };
export const MAX_STRAIGHTEN = 45;
export const MIN_CROP = 0.01;
export const FULL: EditCrop = { x: 0, y: 0, w: 1, h: 1 };

export function isIdentity(r: EditRecipe): boolean {
  return (
    r.quarter % 4 === 0 &&
    r.straighten === 0 &&
    (r.crop === null || isFull(r.crop)) &&
    r.brightness === 0 &&
    r.contrast === 0 &&
    r.temperature === 0
  );
}

export function isFull(c: EditCrop): boolean {
  return c.x <= 1e-4 && c.y <= 1e-4 && c.w >= 1 - 1e-4 && c.h >= 1 - 1e-4;
}

export function sameRecipe(a: EditRecipe, b: EditRecipe): boolean {
  const ca = a.crop ?? FULL;
  const cb = b.crop ?? FULL;
  return (
    a.quarter % 4 === b.quarter % 4 &&
    a.straighten === b.straighten &&
    a.brightness === b.brightness &&
    a.contrast === b.contrast &&
    a.temperature === b.temperature &&
    ca.x === cb.x &&
    ca.y === cb.y &&
    ca.w === cb.w &&
    ca.h === cb.h
  );
}

/** Düzleştirilen W×H görüntünün W×H çerçeveyi boşluksuz kaplaması için büyütme. */
export function coverScale(w: number, h: number, degrees: number): number {
  if (w <= 0 || h <= 0) return 1;
  const t = (Math.abs(degrees) * Math.PI) / 180;
  const s = Math.sin(t);
  const c = Math.cos(t);
  return Math.max(c + s * (h / w), c + s * (w / h));
}

/** Kanal başına 256 girişli ton tablosu (R, G, B). */
export function luts(r: EditRecipe): [Uint8ClampedArray, Uint8ClampedArray, Uint8ClampedArray] {
  const k = r.temperature / 100;
  const gains = [1 + 0.15 * k, 1, 1 - 0.15 * k];
  const gamma = 2 ** ((-0.8 * r.brightness) / 100);
  const slope = 2 ** (r.contrast / 100);
  const out = [new Uint8ClampedArray(256), new Uint8ClampedArray(256), new Uint8ClampedArray(256)] as const;
  for (let ch = 0; ch < 3; ch++) {
    const g = gains[ch] as number;
    const table = out[ch] as Uint8ClampedArray;
    for (let i = 0; i < 256; i++) {
      let x = Math.min((i / 255) * g, 1);
      x = x ** gamma;
      x = Math.min(1, Math.max(0, 0.5 + (x - 0.5) * slope));
      table[i] = Math.round(x * 255);
    }
  }
  return [out[0], out[1], out[2]];
}

/**
 * Önizleme için SVG `feFuncR/G/B type="table"` değerleri. 256 değerli tabloda
 * tarayıcının doğrusal ara değeri her 8 bit girişte tablonun kendisini verir,
 * yani önizleme çekirdeğin piksel tablosuyla aynıdır (sRGB uzayında).
 */
export function toneTables(r: EditRecipe): [string, string, string] {
  const [lr, lg, lb] = luts(r);
  const s = (t: Uint8ClampedArray) => Array.from(t, (v) => (v / 255).toFixed(4)).join(" ");
  return [s(lr), s(lg), s(lb)];
}

/** Düzenlenmiş en/boy oranı; `base` orijinalinki. */
export function editedAspect(base: number, r: EditRecipe): number {
  let a = r.quarter % 2 === 1 ? 1 / base : base;
  if (r.crop) a = (a * r.crop.w) / r.crop.h;
  return Math.min(10, Math.max(0.1, a));
}

/** 90° döndürmede kırpma da çerçeveyle birlikte döner (`dir` 1: saat yönü). */
export function rotateCrop(c: EditCrop | null, dir: 1 | -1): EditCrop | null {
  if (!c) return null;
  return dir === 1 ? { x: 1 - c.y - c.h, y: c.x, w: c.h, h: c.w } : { x: c.y, y: 1 - c.x - c.w, w: c.h, h: c.w };
}

/**
 * Kırpma oranı: `ratio` = genişlik / yükseklik (piksel). Mevcut dikdörtgenin
 * merkezinde, içine sığan en büyük o oranlı dikdörtgen. `frame` çerçevenin piksel oranı.
 */
export function fitRatio(c: EditCrop, ratio: number, frame: number): EditCrop {
  // Normalleştirilmiş uzayda istenen w/h = ratio / frame.
  const want = ratio / frame;
  let w = c.w;
  let h = w / want;
  if (h > c.h) {
    h = c.h;
    w = h * want;
  }
  if (w > 1) {
    w = 1;
    h = w / want;
  }
  if (h > 1) {
    h = 1;
    w = h * want;
  }
  const cx = c.x + c.w / 2;
  const cy = c.y + c.h / 2;
  return clampCrop({ x: cx - w / 2, y: cy - h / 2, w, h });
}

/** Dikdörtgeni çerçevenin içine iter (boyutu korur). */
export function clampCrop(c: EditCrop): EditCrop {
  const w = Math.min(1, Math.max(MIN_CROP, c.w));
  const h = Math.min(1, Math.max(MIN_CROP, c.h));
  return { x: Math.min(1 - w, Math.max(0, c.x)), y: Math.min(1 - h, Math.max(0, c.y)), w, h };
}

/** Çekirdeğe gönderilmeden önce: tam çerçeve kırpma yok sayılır, değerler yuvarlanır. */
export function normalized(r: EditRecipe): EditRecipe {
  const round = (v: number) => Math.round(v * 100) / 100;
  const crop = r.crop && !isFull(r.crop) ? clampCrop(r.crop) : null;
  return {
    quarter: ((r.quarter % 4) + 4) % 4,
    straighten: round(Math.min(MAX_STRAIGHTEN, Math.max(-MAX_STRAIGHTEN, r.straighten))),
    crop: crop && { x: round4(crop.x), y: round4(crop.y), w: round4(crop.w), h: round4(crop.h) },
    brightness: round(r.brightness),
    contrast: round(r.contrast),
    temperature: round(r.temperature),
  };
}

function round4(v: number): number {
  return Math.round(v * 10000) / 10000;
}

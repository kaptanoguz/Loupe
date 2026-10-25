/** Sayısal biçimler: tümü `data` (monospace, tabular) sınıfıyla gösterilir. */

const UNITS = ["B", "KB", "MB", "GB", "TB"];

/** 26418112 → "26.4 MB" (ondalık, SI). */
export function bytes(n: number | null | undefined): string {
  if (n === null || n === undefined || !Number.isFinite(n)) return "—";
  let v = n;
  let u = 0;
  while (v >= 1000 && u < UNITS.length - 1) {
    v /= 1000;
    u++;
  }
  return `${u === 0 ? v.toFixed(0) : v.toFixed(1)} ${UNITS[u]}`;
}

/** 104233 → "104 233" (dar boşluk: tabular hizada ayırıcı karışmaz). */
export function count(n: number): string {
  return Math.trunc(n)
    .toString()
    .replace(/\B(?=(\d{3})+(?!\d))/g, " ");
}

/** BLAKE3: ilk 8 + son 8. */
export function shortHash(hash: string): string {
  return hash.length > 16 ? `${hash.slice(0, 8)}…${hash.slice(-8)}` : hash;
}

/** UUID'nin ilk grubu: kenar çubuğunda disk kimliği. */
export function shortUuid(uuid: string | null | undefined): string {
  if (!uuid) return "—";
  return uuid.split("-")[0] ?? uuid;
}

export function resolution(w: number | null, h: number | null): string {
  return w && h ? `${w}×${h}` : "—";
}

/** Unix saniye → "2025-10-24 15:32" (yerel saat dilimi kayması verilirse onunla). */
export function dateTime(unix: number | null | undefined, tzOffsetMin: number | null = null): string {
  if (unix === null || unix === undefined) return "—";
  const d = new Date((unix + (tzOffsetMin ?? 0) * 60) * 1000);
  const p = (x: number) => String(x).padStart(2, "0");
  return `${d.getUTCFullYear()}-${p(d.getUTCMonth() + 1)}-${p(d.getUTCDate())} ${p(d.getUTCHours())}:${p(d.getUTCMinutes())}`;
}

/** Unix ms → "2025-10-24 15:32" (yerel saat). Katalog zamanları ms'dir. */
export function dateTimeMs(ms: number | null | undefined): string {
  if (ms === null || ms === undefined) return "—";
  const d = new Date(ms);
  const p = (x: number) => String(x).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** 0:42, 12:05 (geri sayım, kalan süre). */
export function clock(secs: number | null | undefined): string {
  if (secs === null || secs === undefined || !Number.isFinite(secs)) return "—";
  const s = Math.max(0, Math.round(secs));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

/** Bayt/sn → "12.4 MB/s". */
export function rate(bps: number | null | undefined): string {
  return bps === null || bps === undefined || bps <= 0 ? "—" : `${bytes(bps)}/s`;
}

// Mobil aktarım sayfasının saf yardımcıları (DOM ve ağ yok; vitest ile sınanır).

export const TUS_VERSION = "1.0.0";
/** Sunucunun tek PATCH'te kabul ettiği en büyük gövde (MAX_PATCH_BYTES). */
export const CHUNK_BYTES = 8 * 1024 * 1024;
/** Özet hesaplanırken dosyadan bir kerede okunan bayt. */
export const HASH_SLICE_BYTES = 4 * 1024 * 1024;
/** Ön kontrolün bir isteğe koyduğu dosya sayısı. */
export const PRECHECK_BATCH = 500;
/** Kopan bağlantıdan sonra yeniden deneme beklemeleri (ms). */
export const RETRY_DELAYS_MS = [1000, 2000, 4000, 8000, 15000, 30000, 30000, 30000];
export const SESSION_STORAGE_KEY = "loupe.session";

/**
 * QR adresinin parça kısmı: `#t=<belirteç>`. Belirteç sunucuya yalnızca
 * gövdede gider; parça istek satırına ve günlüklere hiç girmez.
 */
export function parseFragment(hash) {
  const params = new URLSearchParams(String(hash || "").replace(/^#/, ""));
  const token = params.get("t");
  return token && /^[A-Za-z0-9_-]{16,128}$/.test(token) ? { token } : { token: null };
}

/**
 * Tarayıcıdaki varlık kimliği. Telefon galerisinde dosya adı varlığı tanımlar;
 * aynı adla farklı içerik "telefonda düzenlenmiş" demektir (sunucu çakışma
 * kararını BLAKE3 ile verir).
 */
export function assetId(file) {
  return `web:${file.name}`;
}

/** `Upload-Metadata`: `anahtar base64(UTF-8 değer)` çiftleri, virgülle. */
export function tusMetadata(fields) {
  return Object.entries(fields)
    .filter(([, v]) => v !== undefined && v !== null)
    .map(([k, v]) => `${k} ${b64(String(v))}`)
    .join(",");
}

function b64(s) {
  const bytes = new TextEncoder().encode(s);
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin);
}

/** Sunucunun kabul ettiği platformlar: android | ios | desktop. */
export function platformFromUA(ua) {
  const s = String(ua || "");
  if (/Android/i.test(s)) return "android";
  if (/iPhone|iPad|iPod/i.test(s) || (/Macintosh/i.test(s) && /Mobile/i.test(s))) return "ios";
  return "desktop";
}

/** Masaüstündeki cihaz listesinde görünen ad (en çok 80 karakter). */
export function deviceNameFromUA(ua) {
  const s = String(ua || "");
  const model = /Android [\d.]+; ([^;)]+?)(?: Build[^;)]*)?\)/.exec(s)?.[1]?.trim();
  const browser = /SamsungBrowser/.test(s)
    ? "Samsung Internet"
    : /Firefox|FxiOS/.test(s)
      ? "Firefox"
      : /EdgA?\//.test(s)
        ? "Edge"
        : /CriOS|Chrome/.test(s)
          ? "Chrome"
          : /Safari/.test(s)
            ? "Safari"
            : "Tarayıcı";
  const base =
    model && model !== "K"
      ? model
      : /iPad/.test(s)
        ? "iPad"
        : /iPhone/.test(s)
          ? "iPhone"
          : platformFromUA(s) === "android"
            ? "Android telefon"
            : "Bilgisayar";
  return `${base} (${browser})`.slice(0, 80);
}

const UNITS = ["B", "KB", "MB", "GB", "TB"];

/** 1000 tabanı: telefon ve işletim sistemleri dosya boyutunu böyle gösterir. */
export function formatBytes(n) {
  if (!Number.isFinite(n) || n < 0) return "—";
  let i = 0;
  let v = n;
  while (v >= 1000 && i < UNITS.length - 1) {
    v /= 1000;
    i++;
  }
  return `${i === 0 ? v : v.toFixed(v < 10 ? 2 : 1)} ${UNITS[i]}`;
}

export function formatRate(bytesPerSec) {
  if (!Number.isFinite(bytesPerSec) || bytesPerSec <= 0) return "—";
  return `${(bytesPerSec / 1e6).toFixed(1)} MB/s`;
}

export function formatClock(secs) {
  if (!Number.isFinite(secs) || secs < 0) return "—";
  const s = Math.round(secs);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

/**
 * Kayan pencereli hız ölçer: son `windowMs` içindeki baytlar / süre.
 * Anlık sıçramalar (tek parça bitişi) MB/s göstergesini oynatmaz.
 */
export class RateMeter {
  constructor(windowMs = 3000) {
    this.windowMs = windowMs;
    this.samples = [];
  }

  add(bytes, now) {
    this.samples.push([now, bytes]);
    this.trim(now);
  }

  trim(now) {
    while (this.samples.length > 0 && now - this.samples[0][0] > this.windowMs) this.samples.shift();
  }

  rate(now) {
    this.trim(now);
    if (this.samples.length === 0) return 0;
    const total = this.samples.reduce((a, [, b]) => a + b, 0);
    const span = Math.max(250, now - this.samples[0][0]);
    return (total * 1000) / span;
  }
}

/** Yeniden deneme beklemesi; liste bitince `null` (vazgeç). */
export function retryDelay(attempt) {
  return attempt < RETRY_DELAYS_MS.length ? RETRY_DELAYS_MS[attempt] : null;
}

/**
 * Ön kontrol yanıtı + seçilen dosyalar → aktarılacaklar ve atlananlar.
 * `known`: sunucunun bu tarayıcıdan aynı ad ve değişiklik zamanıyla daha önce
 * aldığı varlıklar.
 */
export function splitByPrecheck(files, known) {
  const have = new Set(known);
  const send = [];
  const skip = [];
  for (const f of files) (have.has(assetId(f)) ? skip : send).push(f);
  return { send, skip };
}

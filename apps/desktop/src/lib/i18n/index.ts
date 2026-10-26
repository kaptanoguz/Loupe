/**
 * Arayüz dili (Türkçe / İngilizce). Dil açılışta bir kez seçilir; Ayarlar'dan
 * değiştirmek pencereyi yeniden yükler, bu yüzden metinler reaktif değildir.
 *
 * Metinlerde `{ad}` yer tutucudur. `|` ile ayrılmış biçimler sayıya göre seçilir
 * (`{n} photo|{n} photos`: n = 1 ise ilki); Türkçede tek biçim yeterli.
 */

import { en } from "./en";
import { type Key, tr } from "./tr";

export type Lang = "tr" | "en";
export type { Key };

export const LANGS: { key: Lang; name: string }[] = [
  { key: "tr", name: "Türkçe" },
  { key: "en", name: "English" },
];

const STORAGE_KEY = "loupe.lang";
const DICTS: Record<Lang, Record<Key, string>> = { tr, en };

function initial(): Lang {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved === "tr" || saved === "en") return saved;
  } catch {
    // Depolama yoksa sistem dili.
  }
  const nav = typeof navigator === "undefined" ? "" : navigator.language.toLowerCase();
  return nav.startsWith("tr") ? "tr" : "en";
}

let current: Lang = initial();
if (typeof document !== "undefined") document.documentElement.lang = current;

export function lang(): Lang {
  return current;
}

/** Testler için: yeniden yüklemeden dil değiştirir. */
export function useLang(l: Lang): void {
  current = l;
}

/** Seçimi saklar ve arayüzü yeni dille yeniden yükler. */
export function setLang(l: Lang): void {
  try {
    localStorage.setItem(STORAGE_KEY, l);
  } catch {
    // Saklanamazsa yalnızca bu oturum için.
  }
  current = l;
  location.reload();
}

export type Params = Record<string, string | number>;

export function t(key: Key, params?: Params): string {
  let s = DICTS[current][key] ?? tr[key];
  if (params && s.includes("|")) {
    const forms = s.split("|");
    const n = Number(params.n ?? params.count ?? 0);
    s = (n === 1 ? forms[0] : forms[forms.length - 1]) ?? s;
  }
  return params ? s.replace(/\{(\w+)\}/g, (m: string, k: string) => (k in params ? String(params[k]) : m)) : s;
}

/** `toLocaleLowerCase` için etkin dil (Türkçe I/ı kuralları yalnızca tr'de). */
export function locale(): string {
  return current === "tr" ? "tr-TR" : "en-US";
}

const TR_MONTHS = ["Ocak", "Şubat", "Mart", "Nisan", "Mayıs", "Haziran", "Temmuz", "Ağustos", "Eylül", "Ekim", "Kasım", "Aralık"];
const EN_MONTHS = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const TR_DAYS = ["Pazartesi", "Salı", "Çarşamba", "Perşembe", "Cumartesi", "Cuma", "Pazar"];
const EN_DAYS = ["Monday", "Tuesday", "Wednesday", "Thursday", "Saturday", "Friday", "Sunday"];
/** Çekirdeğin ürettiği sabit bölüm başlıkları. */
const CORE_TITLES: Record<string, string> = {
  Tümü: "All",
  "Ada göre": "By name",
  "Boyuta göre": "By size",
  Puansız: "Unrated",
  "En alakalı": "Most relevant",
};
const DATE_WORDS = new RegExp(`(?<![\\p{L}])(${[...TR_MONTHS, ...TR_DAYS].join("|")})(?![\\p{L}])`, "gu");

/**
 * Çekirdeğin Türkçe ürettiği tarih ve bölüm metinleri (`24 Ekim 2026`, `Cumartesi`,
 * `3 yıldız`, `Ada göre`) İngilizce arayüzde çevrilir; kullanıcının verdiği adlar
 * (kişi, albüm, yer) bu kalıplara uymaz ve olduğu gibi kalır.
 */
export function coreText<T extends string | null | undefined>(s: T): T {
  if (current === "tr" || !s) return s;
  const fixed = CORE_TITLES[s];
  if (fixed) return fixed as T;
  const stars = /^(\d) yıldız$/.exec(s);
  if (stars) return (stars[1] === "1" ? "1 star" : `${stars[1]} stars`) as T;
  return s.replace(DATE_WORDS, (w) => {
    const m = TR_MONTHS.indexOf(w);
    return m >= 0 ? (EN_MONTHS[m] ?? w) : (EN_DAYS[TR_DAYS.indexOf(w)] ?? w);
  }) as T;
}

import { t } from "./i18n";
/**
 * İçe aktarma klasör düzeni: isteğe bağlı alt klasör + tarih deseni.
 * `iPhone/{yyyy}/{mm}` → alt klasör `iPhone`, desen `{yyyy}/{mm}`.
 * Çekirdek aynı kuralları `validate_layout` ile yeniden denetler.
 */

export const DATE_PATTERNS: { value: string; label: string }[] = [
  { value: "{yyyy}/{mm}/{dd}", label: t("layout.ymd") },
  { value: "{yyyy}/{mm}", label: t("layout.ym") },
  { value: "{yyyy}", label: t("arrange.year") },
  { value: "", label: t("layout.undated") },
];

export const DEFAULT_LAYOUT = "{yyyy}/{mm}/{dd}";

export function splitLayout(template: string): { sub: string; date: string } {
  const t = template.trim();
  for (const p of DATE_PATTERNS) {
    if (!p.value) continue;
    if (t === p.value) return { sub: "", date: p.value };
    if (t.endsWith(`/${p.value}`)) return { sub: t.slice(0, -p.value.length - 1), date: p.value };
  }
  return { sub: t, date: "" };
}

export function joinLayout(sub: string, date: string): string {
  return [sub.trim().replace(/^\/+|\/+$/g, ""), date].filter(Boolean).join("/");
}

/** Geçersizse nedeni, geçerliyse `null`. */
export function layoutError(template: string): string | null {
  const tpl = template.trim();
  if (!tpl) return t("layout.errSub");
  if (tpl.length > 200) return t("layout.errLong");
  if (tpl.startsWith("/")) return t("layout.errAbs");
  if (tpl.includes("\\")) return t("layout.errBackslash");
  for (const part of tpl.split("/")) {
    if (!part) return t("layout.errEmpty");
    if (part.startsWith(".")) return t("layout.errDot");
    if (/[{}]/.test(part.replace(/\{(yyyy|mm|dd)\}/g, ""))) return t("layout.errToken");
  }
  return null;
}

/** Örnek hedef: `/media/Data/Pictures/iPhone/2025/08/14/IMG_1234.HEIC`. */
export function previewPath(root: string, template: string, file: string, when = new Date(2025, 7, 14)): string {
  const p2 = (n: number) => String(n).padStart(2, "0");
  const dir = template
    .trim()
    .replace(/\{yyyy\}/g, String(when.getFullYear()))
    .replace(/\{mm\}/g, p2(when.getMonth() + 1))
    .replace(/\{dd\}/g, p2(when.getDate()));
  return [root.replace(/\/+$/, ""), dir, file].filter(Boolean).join("/");
}

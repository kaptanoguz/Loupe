/**
 * İçe aktarma klasör düzeni: isteğe bağlı alt klasör + tarih deseni.
 * `iPhone/{yyyy}/{mm}` → alt klasör `iPhone`, desen `{yyyy}/{mm}`.
 * Çekirdek aynı kuralları `validate_layout` ile yeniden denetler.
 */

export const DATE_PATTERNS: { value: string; label: string }[] = [
  { value: "{yyyy}/{mm}/{dd}", label: "Yıl / Ay / Gün" },
  { value: "{yyyy}/{mm}", label: "Yıl / Ay" },
  { value: "{yyyy}", label: "Yıl" },
  { value: "", label: "Tarihsiz" },
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
  const t = template.trim();
  if (!t) return "Tarihsiz düzen için bir alt klasör adı girin.";
  if (t.length > 200) return "Klasör yolu çok uzun.";
  if (t.startsWith("/")) return "Yol kütüphane klasörünün içinde olmalı (/ ile başlamaz).";
  if (t.includes("\\")) return "Klasör ayırıcı olarak / kullanın.";
  for (const part of t.split("/")) {
    if (!part) return "Boş klasör adı (// ) olamaz.";
    if (part.startsWith(".")) return "Klasör adı nokta ile başlayamaz (gizli klasörler taranmaz).";
    if (/[{}]/.test(part.replace(/\{(yyyy|mm|dd)\}/g, ""))) return "Yalnızca {yyyy}, {mm}, {dd} kullanılabilir.";
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

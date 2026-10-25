// Fontları @fontsource-variable paketlerinden public/fonts'a kopyalar.
// Uygulama çevrimdışı çalışır: CDN yok, dosyalar depoya işlenir.
// Türkçe karakterler (ğ ş ı İ) latin-ext alt kümesindedir; app.css iki
// @font-face'i unicode-range ile böler.
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const out = join(root, "public", "fonts");
mkdirSync(out, { recursive: true });

for (const family of ["geist", "inter", "jetbrains-mono"]) {
  const pkg = join(root, "node_modules", "@fontsource-variable", family);
  for (const subset of ["latin", "latin-ext"]) {
    copyFileSync(join(pkg, "files", `${family}-${subset}-wght-normal.woff2`), join(out, `${family}-${subset}.woff2`));
  }
  copyFileSync(join(pkg, "LICENSE"), join(out, `${family}.LICENSE.txt`));
}

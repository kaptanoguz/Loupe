#!/usr/bin/env bash
# Loupe — "No AI Slop" koruması (RFC-001B §4.6).
#
# Arayüz kaynaklarında tasarım kurallarını delen sınıfları ve değerleri arar.
# Eşleşme varsa sıfırdan farklı kodla çıkar; CI'da derlemeyi kırar.
#
# Kullanım: tools/slop-guard/slop-guard.sh [kök_dizin]   (varsayılan: apps/desktop/src)

set -euo pipefail

root="${1:-apps/desktop/src}"
if [[ ! -d "$root" ]]; then
  echo "slop-guard: dizin yok: $root" >&2
  exit 2
fi

# Renk tanımlarının tek kaynağı app.css; orada hex serbest.
token_file="app.css"

fail=0
check() {
  local desc="$1" pattern="$2"
  shift 2
  local out
  # Yorum satırları ve app.css'teki "--ad-*: initial;" sıfırlamaları ihlal değildir.
  out=$(grep -rnE --include='*.svelte' --include='*.ts' --include='*.js' --include='*.css' --include='*.html' \
          "$@" -e "$pattern" "$root" \
        | grep -vE '^[^:]+:[0-9]+:[[:space:]]*(/\*|\*|//|--[a-z-]+-\*:[[:space:]]*initial;)' || true)
  if [[ -n "$out" ]]; then
    echo "✗ $desc"
    echo "$out" | sed 's/^/    /'
    fail=1
  fi
}

# 4px üstü köşe yarıçapı ve keyfi yarıçap
check "4px üstü köşe yarıçapı (rounded-md…full, rounded-[…])" \
  '\brounded(-[trblxyse]{1,2})?-(md|lg|xl|2xl|3xl|4xl|full|\[)'
# Gölgeler (Tailwind sınıfı veya ham CSS)
check "gölge (shadow-*, drop-shadow, box-shadow)" \
  '\b(shadow-|drop-shadow|inset-shadow-)|box-shadow\s*:\s*[^n;]|filter\s*:\s*drop-shadow'
# Bulanık cam
check "bulanıklık / glassmorphism (blur, backdrop-*)" \
  '\b(backdrop-|blur-)|backdrop-filter|filter\s*:\s*blur'
# Gradyanlar
check "gradyan (bg-linear-*, bg-radial-*, gradient)" \
  '\bbg-(linear|radial|conic)-|gradient\s*\('
# Renk geçişli / sıçrayan animasyonlar
check "dikkat dağıtan animasyon (bounce, ping, pulse, spin dışı)" \
  '\banimate-(bounce|ping|pulse)\b'
# Keyfi renkler: hex yalnızca app.css içinde
check "app.css dışında hex renk" \
  '#[0-9a-fA-F]{3,8}\b' --exclude="$token_file"
check "keyfi renk sınıfı (bg-[#…], text-[rgb…])" \
  '\b(bg|text|border|fill|stroke|outline|ring)-\[(#|rgb|hsl|oklch)'
# Serif ve jenerik karşılama metinleri
check "serif yazı tipi" '\bfont-serif\b'
check "jenerik karşılama metni" 'Hoş geldiniz|Welcome to Loupe|Get started'

if [[ $fail -ne 0 ]]; then
  echo
  echo "slop-guard: tasarım kuralı ihlali (bkz. docs/rfc/RFC-001B §4)." >&2
  exit 1
fi
echo "slop-guard: temiz ($root)"

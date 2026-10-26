#!/usr/bin/env bash
# Loupe AI modellerini indirir ve SHA-256 ile doğrular (RFC-001G §3).
#
#   scripts/fetch-models.sh                # ~/.local/share/loupe/models
#   scripts/fetch-models.sh --dir /yol     # başka dizin
#   scripts/fetch-models.sh status         # yalnızca durum
#
# Manifest ikiliye gömülüdür (crates/loupe-ai/models.json). Yarım kalan
# indirme sonraki çalıştırmada kaldığı yerden sürer; özeti tutmayan dosya
# asla model dizinine yazılmaz. Uygulamadaki Ayarlar → Yapay zekâ → Modeller
# aynı kodu kullanır.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cmd="install"
args=()
for a in "$@"; do
  case "$a" in
    status | install | verify | pin) cmd="$a" ;;
    *) args+=("$a") ;;
  esac
done

cargo build --quiet --release --manifest-path "$root/Cargo.toml" -p loupe-ai --bin loupe-models
exec "$root/target/release/loupe-models" ${args[@]+"${args[@]}"} "$cmd"

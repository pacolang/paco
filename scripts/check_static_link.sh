#!/usr/bin/env bash
# Builds a program importing every stdlib module that docs/ecosystem.md does
# NOT mark "marked for extraction" with `--link static`. `--link static`
# rejects any `extern` block anywhere in the program (PACO-E0804), so a
# passing build proves none of the modules staying in stdlib forces dynamic
# linking on a program that imports them (RFC 0030). Modules marked for
# extraction (currently numerics, math, blas) are excluded until
# extract-domain-libraries removes them from stdlib/ and from
# docs/ecosystem.md's table, at which point this script's exclusion list
# shrinks to nothing on its own.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cargo build --quiet --manifest-path "$root/compiler/Cargo.toml" -p paco-driver --no-default-features

excluded="$(grep -oiP '(?<=\| `)[a-z_]+(?=`.*marked for extraction)' "$root/docs/ecosystem.md" | LC_ALL=C sort)"
modules="$(find "$root/stdlib" -maxdepth 1 -name '*.paco' -printf '%f\n' | sed 's/\.paco$//' | LC_ALL=C sort)"
included="$(comm -23 <(echo "$modules") <(echo "$excluded"))"

dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT
{
  while IFS= read -r module; do
    [ -n "$module" ] && echo "use stdlib::$module;"
  done <<< "$included"
  echo
  echo 'fn main() {'
  echo '    print("ok")'
  echo '}'
} > "$dir/main.paco"

PACO_STD="$root/stdlib" "$root/compiler/target/debug/paco" build --link static "$dir/main.paco"
[ "$("$dir/main")" = "ok" ]
echo "check_static_link: clean ($(echo "$included" | grep -c .) stdlib modules, excluded: $(echo "$excluded" | tr '\n' ' '))"

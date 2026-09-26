#!/usr/bin/env bash
# Median wall time of `paco run` against the equivalent Rust workflow for
# hello world and a medium program (structs, generics, Vec, Map), on a cold
# build cache (miss) and a warm one (hit). Exits 1 when Paco is slower than
# Rust in any scenario.
#
#   PACO_BIN   paco binary (default: compiler/target/release/paco)
#   RUNS       runs per scenario (default: 20)
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
paco="${PACO_BIN:-$root/compiler/target/release/paco}"
runs="${RUNS:-20}"
bench="$root/scripts/bench"
[ -x "$paco" ] || { echo "no paco binary at $paco (cargo build --release -p paco-driver)" >&2; exit 2; }
command -v rustc > /dev/null && command -v cargo > /dev/null || { echo "rustc and cargo are required" >&2; exit 2; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
unset CARGO_TARGET_DIR CARGO_BUILD_TARGET_DIR RUSTFLAGS

median_ms() {
  sort -n | awk '{ v[NR] = $1 } END { m = (NR % 2) ? v[(NR + 1) / 2] : (v[NR / 2] + v[NR / 2 + 1]) / 2; printf "%.1f", m / 1e6 }'
}

# measure <prepare> <command>: median of $runs timed runs of <command>, each
# after an untimed <prepare>.
measure() {
  local prepare="$1" command="$2"
  if command -v hyperfine > /dev/null; then
    hyperfine --style none --runs "$runs" --prepare "$prepare" --export-json "$work/h.json" "$command" > /dev/null
    python3 -c 'import json,sys; print("%.1f" % (json.load(open(sys.argv[1]))["results"][0]["median"] * 1000))' "$work/h.json"
    return
  fi
  for _ in $(seq "$runs"); do
    bash -c "$prepare" > /dev/null 2>&1
    local start end
    start="$(date +%s%N)"
    bash -c "$command" > /dev/null 2>&1
    end="$(date +%s%N)"
    echo $((end - start))
  done | median_ms
}

cargo_project() {
  local name="$1" source="$2" dir="$work/cargo-$1"
  mkdir -p "$dir/src"
  printf '[package]\nname = "%s"\nversion = "0.1.0"\nedition = "2024"\n' "$name" > "$dir/Cargo.toml"
  cp "$source" "$dir/src/main.rs"
  echo "$dir"
}

failed=0
printf '%-26s %10s %10s  %s\n' scenario "paco ms" "rust ms" result
row() {
  local name="$1" paco_ms="$2" rust_ms="$3" verdict=ok
  if awk -v p="$paco_ms" -v r="$rust_ms" 'BEGIN { exit !(p > r) }'; then
    verdict=SLOWER
    failed=1
  fi
  printf '%-26s %10s %10s  %s\n' "$name" "$paco_ms" "$rust_ms" "$verdict"
}

for program in hello medium; do
  cp "$bench/$program.paco" "$work/$program.paco"
  cp "$bench/$program.rs" "$work/$program.rs"
  cache="$work/cache-$program"
  project="$(cargo_project "$program" "$bench/$program.rs")"
  run_paco="PACO_CACHE='$cache' '$paco' run '$work/$program.paco'"
  cargo_run="cargo run --quiet --offline --manifest-path '$project/Cargo.toml'"

  paco_miss="$(measure "rm -rf '$cache'" "$run_paco")"
  rustc_miss="$(measure "rm -f '$work/$program'" "rustc '$work/$program.rs' -o '$work/$program' && '$work/$program'")"
  cargo_miss="$(measure "cargo clean --quiet --manifest-path '$project/Cargo.toml'" "$cargo_run")"
  bash -c "$run_paco" > /dev/null
  bash -c "$cargo_run" > /dev/null
  paco_hit="$(measure true "$run_paco")"
  cargo_hit="$(measure true "$cargo_run")"

  row "$program miss vs rustc" "$paco_miss" "$rustc_miss"
  row "$program miss vs cargo run" "$paco_miss" "$cargo_miss"
  row "$program hit vs cargo run" "$paco_hit" "$cargo_hit"
done
exit "$failed"

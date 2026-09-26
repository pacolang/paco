#!/usr/bin/env bash
# Checks a distribution from scripts/dist.sh: its file list, and that its
# `paco` builds and runs native and cross programs with nothing else on
# PATH and no rustup in HOME.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
dist="$(cd "${1:?usage: check-dist.sh <dist dir>}" && pwd)"
expected="bin/paco
bin/paco-compile
lib/paco/bin/rust-lld"
for arch in aarch64 x86_64; do
  expected+="
lib/paco/$arch-unknown-linux-gnu/libpaco_runtime.a"
  for file in crt1.o crtbegin.o crtend.o crti.o crtn.o libc.a libpaco_runtime.a libunwind.a; do
    expected+="
lib/paco/$arch-unknown-linux-musl/$file"
  done
done
expected+="
$(cd "$dist" && ls lib/paco/lib/libLLVM.so.*)"
expected+="
$(cd "$root/stdlib" && find . -type f | sed 's|^\./|lib/paco/stdlib/|')"
actual="$(cd "$dist" && find . -type f | sed 's|^\./||' | LC_ALL=C sort)"

if [ ! -d "$dist/lib/paco/stdlib/core" ]; then
  echo "missing lib/paco/stdlib/core: the distribution does not ship stdlib" >&2
  exit 1
fi
if [ "$actual" != "$(echo "$expected" | LC_ALL=C sort)" ]; then
  diff <(echo "$expected" | LC_ALL=C sort) <(echo "$actual") >&2 || true
  echo "unexpected distribution layout" >&2
  exit 1
fi

home="$(mktemp -d)"
trap 'rm -rf "$home"' EXIT
printf 'fn main() {\n    print(42)\n}\n' > "$home/main.paco"
host_arch="$(uname -m)"
other_arch="aarch64"; [ "$host_arch" = aarch64 ] && other_arch="x86_64"
qemu="$(command -v "qemu-$other_arch" || true)"

env -i PATH="$dist/bin" HOME="$home" paco build "$home/main.paco"
[ "$("$home/main")" = 42 ]
[ "$(env -i PATH="$dist/bin" HOME="$home" paco run "$home/main.paco")" = 42 ]
env -i PATH="$dist/bin" HOME="$home" paco build --target "$other_arch-unknown-linux" "$home/main.paco"
if [ -n "$qemu" ]; then
  [ "$("$qemu" "$home/main")" = 42 ]
else
  echo "skipped running the $other_arch binary: qemu-$other_arch not installed"
fi
echo "distribution OK: $dist"

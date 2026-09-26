#!/usr/bin/env bash
# Prints the KEY=VALUE environment that compiles the runtime's C
# dependency (mimalloc) for <target>: the host C compiler for a Linux host's
# own architecture, otherwise clang with Debian headers (on macOS, llvm-ar
# from `rustup component add llvm-tools`).
set -euo pipefail

target="${1:?usage: runtime-env.sh <target triple>}"
arch="${target%%-*}"
variable="${target//-/_}"
host_arch="$(uname -m)"

first_tool() {
  for tool in "$@"; do
    if command -v "$tool" > /dev/null; then command -v "$tool"; return; fi
  done
}

if [ "$arch" = "$host_arch" ] && [ "$(uname -s)" = Linux ]; then
  echo "CC_$variable=cc"
  echo "CFLAGS_$variable=-U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0"
  exit 0
fi
clang="$(first_tool clang clang-21 clang-20 clang-19 clang-18)"
[ -n "$clang" ] || { echo "clang is needed to build the $target runtime" >&2; exit 1; }
headers="$("$(dirname "$0")/fetch-sysroot.sh" "$arch")"
echo "CC_$variable=$clang"
rust_tools="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin"
echo "AR_$variable=$(PATH="$PATH:$rust_tools" first_tool llvm-ar llvm-ar-21 llvm-ar-20 llvm-ar-19 llvm-ar-18 ar)"
echo "CFLAGS_$variable=--target=$arch-linux-gnu -nostdinc -isystem $("$clang" -print-resource-dir)/include -isystem $headers/usr/include/$arch-linux-gnu -isystem $headers/usr/include -U_FORTIFY_SOURCE"

#!/usr/bin/env bash
# Assembles a self-contained Paco distribution:
#   bin/paco          (launcher: answers cached `paco run`s, otherwise runs paco-compile)
#   bin/paco-compile
#   lib/paco/bin/rust-lld
#   lib/paco/lib/libLLVM.so.*   (rust-lld's own library)
#   lib/paco/<arch>-unknown-linux-musl/{crt1.o,crti.o,crtn.o,crtbegin.o,crtend.o,libc.a,libunwind.a,libpaco_runtime.a}
#   lib/paco/<arch>-unknown-linux-gnu/libpaco_runtime.a
#   lib/paco/stdlib/...  (the standard library, at the same version as the compiler)
# PACO_NO_LLVM=1 builds paco without the LLVM backend (`paco build --release`
# then reports that it is not built in).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/compiler/Cargo.toml" | head -1)"
host="$(rustc -vV | sed -n 's/^host: //p')"
dest="${1:-$root/dist/paco-$version-$host}"
rust_sysroot="$(rustc --print sysroot)"

cargo build --release --quiet --manifest-path "$root/compiler/Cargo.toml" -p paco-driver ${PACO_NO_LLVM:+--no-default-features}
rm -rf "$dest"
mkdir -p "$dest/bin" "$dest/lib/paco/bin" "$dest/lib/paco/lib"
cp "$root/compiler/target/release/paco" "$root/compiler/target/release/paco-compile" "$dest/bin/"
cp "$rust_sysroot/lib/rustlib/$host/bin/rust-lld" "$dest/lib/paco/bin/"
cp "$rust_sysroot"/lib/libLLVM.so.* "$dest/lib/paco/lib/"
cp -r "$root/stdlib" "$dest/lib/paco/stdlib"

for arch in x86_64 aarch64; do
  for env in musl gnu; do
    target="$arch-unknown-linux-$env"
    while IFS= read -r setting; do export "$setting"; done < <("$root/scripts/runtime-env.sh" "$target")
    cargo build --release --quiet --manifest-path "$root/runtime/Cargo.toml" -p paco-runtime-ffi --target "$target"
    mkdir -p "$dest/lib/paco/$target"
    cp "$root/runtime/target/$target/release/libpaco_runtime_ffi.a" "$dest/lib/paco/$target/libpaco_runtime.a"
    if [ "$env" = musl ]; then
      for file in crt1.o crti.o crtn.o crtbegin.o crtend.o libc.a libunwind.a; do
        cp "$rust_sysroot/lib/rustlib/$target/lib/self-contained/$file" "$dest/lib/paco/$target/"
      done
    fi
  done
done
echo "$dest"

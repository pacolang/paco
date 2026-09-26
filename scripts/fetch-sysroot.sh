#!/usr/bin/env bash
# Extracts a Debian glibc system root for <arch> without root privileges or
# dpkg (so it also runs on macOS):
# the shared libraries dynamic-mode links need and the C headers a C
# compiler needs to build the runtime's allocator for that architecture.
set -euo pipefail

arch="${1:?usage: fetch-sysroot.sh <aarch64|x86_64> [dest]}"
case "$arch" in
  aarch64) deb_arch=arm64 ;;
  x86_64) deb_arch=amd64 ;;
  *) echo "unsupported architecture: $arch" >&2; exit 1 ;;
esac
dest="${2:-${XDG_CACHE_HOME:-$HOME/.cache}/paco/sysroot/$arch-linux-gnu}"
mirror="${DEBIAN_MIRROR:-https://deb.debian.org/debian}"
suite="${DEBIAN_SUITE:-trixie}"

if [ ! -e "$dest/usr/lib/$arch-linux-gnu/libc.so.6" ]; then
  work="$(mktemp -d)"
  trap 'rm -rf "$work"' EXIT
  unxz() {
    if command -v python3 > /dev/null; then python3 -c 'import lzma, sys; sys.stdout.buffer.write(lzma.decompress(sys.stdin.buffer.read()))'; else xz -dc; fi
  }
  curl -sfL "$mirror/dists/$suite/main/binary-$deb_arch/Packages.xz" | unxz > "$work/Packages"
  mkdir -p "$dest"
  for package in libc6 libc6-dev libgcc-s1 linux-libc-dev; do
    file="$(awk -v p="$package" '$0 == "Package: " p {found=1} found && /^Filename:/ {print $2; exit}' "$work/Packages")"
    curl -sfL "$mirror/$file" -o "$work/$package.deb"
    (cd "$work" && ar x "$package.deb" && tar xf data.tar.* -C "$dest" && rm -f data.tar.* control.tar.* debian-binary)
  done
fi
for dir in lib lib64; do
  if [ -d "$dest/usr/$dir" ] && [ ! -e "$dest/$dir" ]; then ln -s "usr/$dir" "$dest/$dir"; fi
done
echo "$dest"

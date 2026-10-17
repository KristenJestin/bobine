#!/bin/sh
# Packs a built `bobine` with its desktop entry and icons, the licence and the notices of what it
# is built from, and writes the SHA-256 checksum beside the archive. Prints the archive's file name.
#
#   tools/package.sh <version> <binary> <out-dir>
set -eu

[ "$#" -eq 3 ] || { echo "usage: tools/package.sh <version> <binary> <out-dir>" >&2; exit 2; }
version=$1 binary=$2 out=$3
root=$(cd "$(dirname "$0")/.." && pwd)

name="bobine-$version-linux-x86_64"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

mkdir "$stage/$name"
install -m 755 "$binary" "$stage/$name/bobine"
install -m 644 "$root/LICENSE" "$stage/$name/LICENSE"
install -m 644 "$root/THIRD-PARTY-NOTICES.md" "$stage/$name/THIRD-PARTY-NOTICES.md"
# The desktop entry and the icons of the hicolor theme, laid out as under /usr/share.
cp -r "$root/assets/linux/share" "$stage/$name/share"

mkdir -p "$out"
tar --owner=0 --group=0 -C "$stage" -czf "$out/$name.tar.gz" "$name"
# Relative name, so that `sha256sum --check` works from the folder the two files are in.
(cd "$out" && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
echo "$name.tar.gz"

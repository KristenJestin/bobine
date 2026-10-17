#!/bin/sh
# Tests the `install.sh` of the release archive: packs a fake `bobine` (a shell script) with
# `tools/package.sh`, extracts the archive and installs it into temporary folders. Nothing here
# touches the real system: PREFIX and HOME are always temporary, and no cargo is involved.
#
#   tools/test-install.sh
set -u

root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'kill $(jobs -p) 2>/dev/null; rm -rf "$work"' EXIT

failures=0
fail() { echo "FAIL: $*" >&2; failures=$((failures + 1)); }
ok() { echo "ok: $*"; }
# check <name> <command...>: the command succeeds.
check() { name=$1; shift; if "$@"; then ok "$name"; else fail "$name"; fi; }
# check_not <name> <command...>: the command fails.
check_not() { name=$1; shift; if "$@"; then fail "$name"; else ok "$name"; fi; }

# A fake `bobine` that prints the version the way the real one does.
fake_bobine() { # <path> <version-exit-status>
  # shellcheck disable=SC2016 # the script written must expand its own variables, not this one's
  printf '#!/bin/sh\n[ "${1:-}" = --version ] || exit 0\n[ %s = 0 ] && echo "bobine 9.9.9"\nexit %s\n' "$2" "$2" > "$1"
  chmod 755 "$1"
}

# pack <name> <version-exit-status>: extracts an archive into $work/<name>/ and sets $archive.
pack() {
  fake_bobine "$work/$1.bin" "$2"
  mkdir -p "$work/$1"
  "$root/tools/package.sh" 9.9.9 "$work/$1.bin" "$work/$1/out" >/dev/null || fail "$1: package.sh"
  tar -xzf "$work/$1/out/bobine-9.9.9-linux-x86_64.tar.gz" -C "$work/$1"
  archive=$work/$1/bobine-9.9.9-linux-x86_64
}

# A command that is run with a temporary HOME; PATH keeps the system's tools.
run_install() { # <home> <prefix or empty> <archive>
  if [ -n "$2" ]; then PREFIX="$2" HOME="$1" "$3/install.sh"; else env -u PREFIX HOME="$1" "$3/install.sh"; fi
}

# --- the archive carries the script -------------------------------------------------------------
pack good 0
check "the archive holds an executable install.sh" test -x "$archive/install.sh"
good=$archive

# --- a first install ------------------------------------------------------------------------------
home=$work/home1 prefix=$work/prefix1
mkdir -p "$home/.config/bobine"
echo 'server = "https://example.invalid"' > "$home/.config/bobine/config.toml"
cp -r "$home/.config" "$work/config-before"

run_install "$home" "$prefix" "$good" >"$work/first.out" 2>&1
check "the first install succeeds" test "$?" = 0
check "the binary is executable under PREFIX/bin" test -x "$prefix/bin/bobine"
check "the installed binary starts" sh -c "\"$prefix/bin/bobine\" --version | grep -q '^bobine 9.9.9\$'"
check "the desktop entry lands under PREFIX" cmp -s "$root/assets/linux/share/applications/bobine.desktop" "$prefix/share/applications/bobine.desktop"
(cd "$root/assets/linux/share/icons/hicolor" && find . -type f) | while read -r icon; do
  cmp -s "$root/assets/linux/share/icons/hicolor/$icon" "$prefix/share/icons/hicolor/$icon" || echo "$icon"
done > "$work/missing-icons"
check "every icon lands under PREFIX" test ! -s "$work/missing-icons"
check "the icons are not empty" test -n "$(find "$prefix/share/icons/hicolor" -name 'bobine.*' -type f)"
check "no half-written file is left behind" test -z "$(find "$prefix" -name '.*.new')"
check "the configuration is untouched" diff -r "$work/config-before" "$home/.config"
check "nothing is written outside PREFIX and the configuration" test -z "$(find "$home" -mindepth 1 -not -path "$home/.config*")"

# --- the default prefix is $HOME/.local ---------------------------------------------------------------
home=$work/home2
mkdir -p "$home"
run_install "$home" "" "$good" >/dev/null 2>&1
check "without PREFIX it installs under HOME/.local" test -x "$home/.local/bin/bobine"

# --- a second install while the old binary runs ----------------------------------------------------------
# A real running executable: a copy of `sleep`, which a plain overwrite would refuse (ETXTBSY).
prefix=$work/prefix3 home=$work/home3
mkdir -p "$prefix/bin" "$home"
cp "$(command -v sleep)" "$prefix/bin/bobine"
"$prefix/bin/bobine" 60 &
running=$!
old_inode=$(stat -c %i "$prefix/bin/bobine")
run_install "$home" "$prefix" "$good" >"$work/second.out" 2>&1
check "an install over a running Bobine succeeds" test "$?" = 0
check "the running Bobine is still alive" kill -0 "$running"
check "the binary was replaced by rename (new file)" test "$(stat -c %i "$prefix/bin/bobine")" != "$old_inode"
check "the new binary is the packed one" sh -c "\"$prefix/bin/bobine\" --version | grep -q '^bobine 9.9.9\$'"
kill "$running" 2>/dev/null; wait "$running" 2>/dev/null

# --- the notes -------------------------------------------------------------------------------------------
home=$work/home4 prefix=$work/prefix4
mkdir -p "$home" "$work/fakebin"
printf '#!/bin/sh\necho "\\tlibmpv.so.2 => not found"\n' > "$work/fakebin/ldd"
chmod 755 "$work/fakebin/ldd"
PATH="$work/fakebin:$PATH" run_install "$home" "$prefix" "$good" >"$work/notes.out" 2>&1
check "it says when the bin folder is not in PATH" grep -q "is not in your PATH" "$work/notes.out"
check "it says when libmpv is missing" grep -q "libmpv" "$work/notes.out"
check "a missing libmpv does not fail the install" test -x "$prefix/bin/bobine"

# --- the refusals ----------------------------------------------------------------------------------------
# root: a fake `id` stands for the superuser, so the test does not need to be one.
home=$work/home5 prefix=$work/prefix5
mkdir -p "$home" "$work/rootbin"
# shellcheck disable=SC2016 # same: expanded by the fake `id`, not here
printf '#!/bin/sh\n[ "${1:-}" = -u ] && { echo 0; exit 0; }\nexec /usr/bin/id "$@"\n' > "$work/rootbin/id"
chmod 755 "$work/rootbin/id"
PATH="$work/rootbin:$PATH" run_install "$home" "$prefix" "$good" >"$work/root.out" 2>&1
check_not "it refuses to run as root" test "$?" = 0
check "root: it says why" grep -qi "root" "$work/root.out"
check "root: nothing is installed" test ! -e "$prefix/bin/bobine"

pack broken 1
home=$work/home6 prefix=$work/prefix6
mkdir -p "$home"
run_install "$home" "$prefix" "$archive" >"$work/broken.out" 2>&1
check_not "it fails when bobine --version fails" test "$?" = 0
check "a failing bobine --version is reported" grep -q "does not start" "$work/broken.out"

home=$work/home7 prefix=$work/prefix7
mkdir -p "$home"
rm "$good/bobine"
run_install "$home" "$prefix" "$good" >"$work/nobin.out" 2>&1
check_not "it fails when the binary is missing from the archive" test "$?" = 0
check "a missing binary is reported" grep -q "bobine is missing" "$work/nobin.out"

if command -v shellcheck >/dev/null 2>&1; then
  check "shellcheck is clean" shellcheck "$root/tools/test-install.sh" "$root/tools/package.sh" "$root/assets/linux/install.sh"
else
  echo "skip: shellcheck is not installed"
fi

[ "$failures" = 0 ] && { echo "All checks passed."; exit 0; }
echo "$failures check(s) failed." >&2
exit 1

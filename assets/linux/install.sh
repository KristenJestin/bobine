#!/bin/sh
# Installs Bobine from the folder of its release archive: `bobine` goes into
# ${PREFIX:-$HOME/.local}/bin, and the desktop entry and the icons (the files of `share/`) under
# ${PREFIX:-$HOME/.local}/share, where the launchers and the icon themes look for them. Every file
# is copied beside, then renamed, so a Bobine that is playing keeps its old binary and is never
# stopped: the next start runs the new one. It never asks for root, and never touches your
# configuration (~/.config/bobine) nor your keyring.
#
#   ./install.sh
#   PREFIX=/some/where ./install.sh
set -eu

[ "$(id -u)" != 0 ] || { echo "install.sh: do not run it as root: it installs for your user." >&2; exit 1; }

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
prefix=${PREFIX:-${HOME:?HOME is not set: say where to install with PREFIX}/.local}
bin=$prefix/bin
share=$prefix/share

[ -x "$here/bobine" ] || { echo "install.sh: $here/bobine is missing" >&2; exit 1; }
[ -f "$here/share/applications/bobine.desktop" ] || { echo "install.sh: $here/share/applications/bobine.desktop is missing" >&2; exit 1; }
[ -d "$here/share/icons/hicolor" ] || { echo "install.sh: $here/share/icons/hicolor is missing" >&2; exit 1; }

# <mode> <from> <to>: copied beside, then renamed. A launcher starting the program meanwhile never
# finds a half written file, and a program still running keeps the old one open.
put() {
  mkdir -p "$(dirname "$3")"
  install -m "$1" "$2" "$(dirname "$3")/.$(basename "$3").new"
  mv -f "$(dirname "$3")/.$(basename "$3").new" "$3"
  echo "Installed $3"
}

put 755 "$here/bobine" "$bin/bobine"

# `share/` of the archive is laid out as under /usr/share: each file goes to the same place.
(cd "$here/share" && find applications icons -type f) | while IFS= read -r file; do
  put 644 "$here/share/$file" "$share/$file"
done

# Only caches for the desktops and themes that read them: never required, and their failure does
# not fail the install.
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$share/applications" >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -f -t "$share/icons/hicolor" >/dev/null 2>&1 || true
fi

case ":${PATH:-}:" in
  *":$bin:"*) ;;
  *) echo "Note: $bin is not in your PATH." ;;
esac
# libmpv is the one thing the system must provide (libmpv2 on Ubuntu, mpv on Arch). Looked for
# before the program is run: without it, it does not start.
if command -v ldd >/dev/null 2>&1 && ldd "$bin/bobine" 2>/dev/null | grep -q 'not found'; then
  echo "Note: a library is missing for bobine; install libmpv (see docs/install.md)." >&2
fi
"$bin/bobine" --version || { echo "Note: $bin/bobine does not start." >&2; exit 1; }
echo "Done. The configuration and the keyring were left as they were."

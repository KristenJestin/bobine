# Installing Bobine

No build: a release or the beta is an archive of the Releases page of this repository
(`bobine-<version>-linux-x86_64.tar.gz`, with its `.sha256`), built on Ubuntu 24.04 for Linux
x86_64. It needs glibc 2.39 or newer (Omarchy has it), and a Wayland session.

- **A release** (`vX.Y.Z`) is made when `dev` is merged into `main`.
- **The beta** (`vX.Y.Z-beta.N`, marked pre-release) is made at every push to `dev`, and only the
  newest one exists. It is the patch after the last release, so it is newer than it. Only a push
  that changes code makes one (not documentation, the licence or the git hooks). It is published
  right after the push, even before the tests on `dev` finish: a broken beta is replaced by the
  next push to `dev`.

See [what a version promises](versions.md).

## Install

Bobine plays with libmpv, which the archive does not carry: install mpv's library from your
distribution first (`sudo pacman -S mpv` on Arch and Omarchy, `sudo apt install libmpv2` on
Ubuntu). Then, in an empty folder, with `<tag>` the release (`v0.1.0`) or the beta
(`v0.1.1-beta.21`) shown on the Releases page:

```
gh release download <tag> -R KristenJestin/bobine --pattern 'bobine-*'
sha256sum --check bobine-*.sha256
tar -xzf bobine-*.tar.gz
cd bobine-*-linux-x86_64
./install.sh
```

It puts `bobine` in `~/.local/bin`, and the desktop entry and the icons under `~/.local/share`
(`PREFIX=/some/where ./install.sh` for another place), then runs `bobine --version`. It never asks
for root and never touches `~/.config/bobine` nor the keyring. `~/.local/bin` must be in your
`PATH`; the script says so when it is not. Without `gh`, download the two files from the Releases
page in a browser and run the same commands from `sha256sum` on.

## Update

Download the newer archive the same way and run its `install.sh`: a Bobine that is playing keeps
running its old binary, and the next start runs the new one. The account and the settings in
`~/.config/bobine` stay: no new sign-in is needed within a minor version.

## Remove everything

```
rm ~/.local/bin/bobine ~/.local/share/applications/bobine.desktop
rm ~/.local/share/icons/hicolor/*/apps/bobine.*
rm -r ~/.config/bobine     # the account and the settings; the token is in the keyring (Secret Service)
```

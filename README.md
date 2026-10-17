# Bobine

A native client for films and series on Linux (Wayland), written in Rust: browse a media server's
libraries, then play in a fast native player instead of a browser tab. Jellyfin is the first
provider; others can follow behind the same interface.

[Install a release or the beta](docs/install.md); [what a version promises](docs/versions.md). Licensed under Apache-2.0.

Agents read `AGENTS.md`. Work in progress: see the issues.

## Build

```
cargo build            # libmpv (libmpv-dev, mpv) is needed to build and to play
cargo test
```

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

## Command line

```
bobine                                  opens the window
bobine --version                        prints the version
bobine home --json                      Continue watching, then Next up
bobine search <text> --json             the films and series whose name holds the text
bobine open film|series|episode:<id>    shows that page in the window
bobine play <id> [--from-start]         plays a film, an episode or a series' next episode
```

`home` and `search` print entries for the desktop launcher (the format of poste's
`docs/launcher-format.md`) in JSON on stdout, and never open a window. Each entry plays or
resumes on Enter, and offers "Play from the start" (once started) and "Open the page". The
token is read from the keyring only, and never printed.

`open` and `play` go to the window already running, through the socket
`$XDG_RUNTIME_DIR/bobine.sock`; with none running, they open one that shows or plays it.

| Exit code | Meaning | What is printed |
| --- | --- | --- |
| 0 | done | the entries (`[]` when there is nothing) |
| 1 | not signed in, or the server ended the session | one entry whose action opens Bobine to sign in |
| 2 | the server is unreachable | one entry, its address as subtitle |
| 3 | another failure (the keyring, the account file, a wrong answer) | one entry that says it |
| 4 | a usage error | one entry that says how to use it |

# AGENTS.md — Bobine

Instructions for any coding agent working in this repository. `CLAUDE.md` is a symbolic link
to this file: there is one contract, not two that drift apart.

## Project overview

Bobine is a native client for films and series, for Linux (Wayland), written in Rust. It browses
a media server's libraries and plays in its own fast native player, instead of a browser tab that
is slow and eats memory.

- **Providers** sit behind one interface. **Jellyfin** is the first; others can follow without
  touching the rest of the program.
- **The player** is libmpv: hardware decoding on the graphics card, `gpu-next` rendering, HDR,
  every subtitle and audio track. How its video reaches the window (a Wayland subsurface under
  the window, a texture shared with the window, or a separate playback window) is decided by
  measurement, before any interface is built.
- **The window** is drawn with GPUI, with smooth animations and long lists that never stutter.
- **Jellyfin, fully**: libraries, Continue Watching and Next Up, resume position and watched
  state reported to the server, direct play first and server transcoding only when the file
  cannot play, audio and subtitle tracks (external subtitles included), intro and credits
  skipping (media segments), trickplay thumbnails on the seek bar, multiple versions,
  collections, search.

## Commands

```
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
tools/commit-message.sh --range origin/dev..HEAD
git config core.hooksPath .githooks     # once per clone: the commit-msg and pre-commit hooks
```

## Principles

### 1. Think before coding

**Don't assume. Don't hide confusion. Surface tradeoffs.**

- State your assumptions explicitly. If uncertain, ask.
- If multiple interpretations exist, present them; don't pick silently.
- If a simpler approach exists, say so. Push back when warranted.
- If the code and the issue disagree, the issue wins; if the issue is wrong, say so instead of
  quietly deviating.

### 2. Simplicity first

**Minimum code that solves the problem. Nothing speculative.**

- No features beyond what the task asks. No abstractions for single-use code.
- No configurability that was not requested.
- No error handling for impossible scenarios. If 200 lines could be 50, rewrite.

### 3. Surgical changes

**Touch only what you must. Clean up only your own mess.**

- Don't "improve" adjacent code, comments or formatting. Don't refactor what isn't broken.
- Match existing style. If you notice unrelated dead code, mention it, don't delete it.
- Every changed line must trace directly to the task.

### 4. The failing test first

**Define success criteria. Loop until verified.**

- Write the test that fails first, see it fail, then make it pass.
- "Fix the bug" → a test that reproduces it, then the fix.
- Every scenario of the issue gets a test named after it. A scenario without a test is a
  defect, not a TODO.

## Code style

- Rust 2024, `cargo fmt`, `cargo clippy -- -D warnings` clean.
- Code, identifiers, comments, docs, issues and commits are in English.
- The media server, the player and the display are reached through traits (ports), so the logic
  around them is tested with fakes.
- A comment says why, not what.

## Secrets (non-negotiable)

- The password exists only inside the login, long enough to get the server's access token: no
  structure, file or log keeps it. The token lives only in the keyring (Secret Service) and in
  requests; never in a message, a `Display`, a `Debug`, a panic or an output.
- A stream URL that carries a token is never logged, shown or written.

## Testing

- **Tests never touch a real server, real playback, the real keyring, the GPU or the real
  configuration.** They talk to a fake local server, use a fake player, and use temporary
  folders.
- What only a real machine can show (smoothness, frame drops, HDR, hardware decoding, CPU and
  GPU load) is measured by hand and reported in the pull request, with the numbers.
- A test that fails only under load is rerun alone before the failure is called real.

## How the work is organised

| Who | What |
|---|---|
| The maintainer | decides what is built, tries each tranche on their machine, and merges what they have not delegated |
| The lead agent | turns the maintainer's decisions into issues, launches the developing agents, reviews their pull requests, keeps the tracker honest, and never hides a failure |
| Developing agents | implement one issue each: the failing test first, then the code, the verification and a pull request; they never merge |
| CI | `verify`, `commit-messages` and `shape` on every pull request |

### The tracker

- **Issues are the tickets, and a ticket stands alone.** It holds the what, the where, the "how
  to verify" and the base branch, with no local path. If it lacks one of them, ask rather than
  guess.
- The plan is cut into **tranches**, labels `tranche:1`, `tranche:2`…, beside `type:*` and
  `area:*`. A tranche ends on something the maintainer can use, and they accept it on their
  machine.
- **`dev` is the integration branch** and the default branch: a branch starts from
  `origin/dev` and its pull request targets it. `main` holds released versions.

## Git and pull requests (non-negotiable)

- **Never commit, merge, rebase, push or force-push on `main` or `dev` directly.** If you are
  on one of these branches, create a branch first (`feat/<n>-<topic>`, `fix/<n>-<topic>`). The
  `pre-commit` hook refuses it.
- Never rewrite published history. No `--no-verify`.
- One commit = one intent. No `wip` commits. Don't mix formatting and logic in one commit.
- Commits use the Git user configured on the machine.
- The subject is `<type>(<scope>): <subject>`: the **scope is required**, the subject is 72
  characters at most, starts lowercase, has no final period, and the type is one of feat, fix,
  refactor, test, docs, chore, build, ci, perf. `tools/commit-message.sh` is the judge (the
  `commit-msg` hook and CI run it).
- Scopes: `provider`, `jellyfin`, `library`, `player`, `video`, `gui`, `search`, `repo`, `ci`.
- **Pull requests are merged by squash only.** The title is a plain Angular subject. The
  description is written in the first person as the maintainer and ends with `Closes #<n>`.

## Security and privacy

- No server address, user name, library content or watch history goes into the repository, an
  issue, a pull request or a commit. Examples use invented data.
- Secrets are never printed, logged or committed.
- Nothing is published outside the pull request: no gist, no upload, no paste service.
- **A permission denial is never worked around**: do what remains, say what was refused.

## Working on a machine

- The machine is the maintainer's own, in use while you work. **Never open a window on their
  screens**: a window, player or benchmark check runs in a headless nested compositor, or not at
  all. The graphics card is shared: stop every player you started.
- Never play sound on the real output unless the issue asks for it and the maintainer agreed.
- No artificial load: no stress runs, no parallel benchmark loops. Heavy builds run at low
  priority (`nice -n 19`), one at a time.
- Never send requests to the maintainer's server from tests; a manual check against it is read
  only unless the issue says otherwise (no watched state, no progress written).
- Worktrees, logs and every other scratch file live outside the clone.
- Before removing a worktree, check it holds no uncommitted change and no unpushed commit
  (`git log HEAD --not --remotes`).

## GitHub gotchas

- Prefer REST (`gh api repos/…`) to GraphQL, which gets rate-limited.
- A body holding backticks or apostrophes is written to a file and passed with `--body-file`.

## When done

Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test`, plus the
manual checks the issue asks for, and report the real output. If something fails, say so;
don't claim green.

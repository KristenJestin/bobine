# What a version promises

Nobody edits a version: it comes from the Angular subjects of the pull requests merged into
`main` (`feat` moves the minor in `0.x`, `fix` the patch, a breaking change also the minor), and
the binary reports it with `bobine --version` and in Settings › About. A local build says
`0.0.0-dev`.

Bobine is in `0.x`: a 1.0 is the maintainer's decision. From 1.0, a breaking change makes a new
major version (semantic-release counts it from the `BREAKING CHANGE:` footer of a commit). Until
then it moves the minor, and the promises below hold **within a minor version** (`0.3.x`). A beta
(`vX.Y.Z-beta.N`) is the patch after the last release and promises nothing beyond what that
release did, until it is released.

A change is breaking when it breaks one of two promises.

## The command line

`bobine` and its flags.

Breaking: a flag removed or renamed, or one that no longer does what it did.
Not breaking: a new optional flag.

## What Bobine keeps

The account file (`account.toml`) and the settings (`settings.toml`) in `~/.config/bobine`
(or `$XDG_CONFIG_HOME/bobine`), and the access token in the keyring. An update keeps them and
reads them: no new sign-in within a minor.

Breaking: a change that makes the viewer sign in again or edit a file by hand.
Not breaking: a file that Bobine migrates by itself and keeps the meaning of.

## The first release

The first release is counted from a `v0.0.0` tag the maintainer puts once on the first commit of
`main`: semantic-release starts at 1.0.0 from a history with no release, and `release.yml` stops
with a message instead of publishing it.

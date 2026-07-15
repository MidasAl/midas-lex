# Midas Lex releases

Midas Lex has two installable parts:

- the `midas-lex` wrapper crate from crates.io
- Midas Lex runtime binaries attached to GitHub Releases in this repository

Installing the crate puts the wrapper on your `PATH`. The wrapper downloads the
runtime binary the first time you run `midas-lex`, verifies its published
SHA-256 checksum, stores it under `MIDAS_LEX_VERUS_HOME`, and then starts it.

If `MIDAS_LEX_VERUS_HOME` is not set, the default storage directory is
`$XDG_DATA_HOME/midas-lex/verus` when `XDG_DATA_HOME` is set. Otherwise it is
`$HOME/.midas-lex/verus`.

## Version selection

By default, `midas-lex` uses the newest installed ordinary runtime binary. If no
ordinary runtime binary is installed yet, it downloads the newest published
stable semver GitHub Release and requires that release to include an asset for
your platform. If no stable release exists, it falls back to the newest
non-draft semver pre-release. Draft GitHub Releases are not selected.

A version selector opts in to a specific release, including a pre-release:

```sh
midas-lex +v0.0.1-alpha.1 docs
midas-lex +prerelease docs
```

`+vVERSION` selectors run an exact release tag. `+prerelease` allows the newest
non-draft semver release, including alpha, beta, and release-candidate tags.

Set `MIDAS_LEX_VERUS_VERBOSE=1` to show the selected runtime version tag and
binary path. Set `MIDAS_LEX_VERUS_LOG=info` to show download and update logs.

## Wrapper self-update

Run `midas-lex +self-update` to replace the installed public wrapper executable.
Run `midas-lex +self-update --help` for the wrapper-owned command help. The
leading `+self-update` token accepts no version selector or runtime arguments;
all invocations without that exact leading token retain normal selector parsing
and runtime pass-through.

The command uses only the official `MidasAl/midas-lex` repository. Selection is
the same stable-preferred policy as a default runtime download: the newest
non-draft stable semver release, or the newest non-draft semver pre-release only
when no stable release exists. A same or older wrapper version is left unchanged.
The selected release must contain the public wrapper asset for the current target
and a same-name `.sha256` file with exactly one valid entry naming that asset.

Linux and macOS updates resolve the running executable directly instead of
searching `PATH`. The replacement downloads beside that executable, remains
unusable until its SHA-256 checksum is verified, inherits the existing executable
mode, is synced, and replaces the old path by an atomic same-filesystem rename.
An adjacent lock serializes concurrent commands. A waiter rechecks the executable
before replacement and stops if another command changed it. Errors before rename
remove the staged download and leave the old executable intact; permission errors
identify the unwritable path and suggest `cargo install midas-lex --force`.

Windows wrapper assets remain part of the six-target release set, but a running
Windows `.exe` cannot be replaced safely in place. On Windows this command exits
before downloading or changing a file and directs the user to run
`cargo install midas-lex --force` after it exits.

## Background updates

After starting the installed runtime binary, the wrapper may check for a newer
ordinary release in the background. Background checks are throttled to once every
hour per platform.

If a newer stable release is available, or no stable release exists and a newer
pre-release is available, the wrapper downloads and verifies it for the next
invocation. The running command keeps using the binary it already started.

Runtime downloads and installs use one lock per data directory. The lock file is
`$MIDAS_LEX_VERUS_HOME/locks/install.lock`, or
the active default data directory's `locks/install.lock` when
`MIDAS_LEX_VERUS_HOME` is not set. Other wrapper processes using the same data
directory wait for that lock before installing a runtime.

The normal startup sequence is: resolve the installed verified runtime, start it,
start a separate wrapper child for the throttled update check, and wait for the
runtime child. The update child is identified by a private marker and exits after
checking or installing a newer runtime; it never replaces the binary already
running for the current command. When no verified runtime is installed, the
wrapper installs the selected latest release synchronously and starts it without
spawning that background child for the first invocation.

`cargo test` covers the selector, release ordering, timer, marker, lock,
data-directory, and checksum cases without downloading, publishing, or
uploading a release. `cargo run -- help` exercises the real wrapper path and may
download a runtime when none is installed. To test selector parsing with an
isolated data directory, use `MIDAS_LEX_VERUS_HOME=/tmp/midas-lex-doc-check
cargo run -- +v0.0.1-alpha.1 help`; that command may download the named release.

## Release assets

Runtime assets use this pattern:

```text
midas-lex-private-VERSION-TARGET
midas-lex-private-VERSION-TARGET.exe
midas-lex-private-VERSION-TARGET.sha256
midas-lex-private-VERSION-TARGET.exe.sha256
```

Public wrapper assets used by `+self-update` use this pattern:

```text
midas-lex-VERSION-TARGET
midas-lex-VERSION-TARGET.exe
midas-lex-VERSION-TARGET.sha256
midas-lex-VERSION-TARGET.exe.sha256
```

The wrapper downloads the asset matching your platform and its same-name
`.sha256` file. Installation stops if the checksum does not match.

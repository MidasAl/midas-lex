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

The wrapper downloads the asset matching your platform and its same-name
`.sha256` file. Installation stops if the checksum does not match.

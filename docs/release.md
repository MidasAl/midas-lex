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

On a default invocation with a verified runtime already installed, the wrapper
starts that runtime before it may start a separate background child. Checks are
throttled to once every hour per platform. The marked, one-use child performs one
stable-preferred, non-draft semantic-version release lookup and uses that release
for both wrapper and runtime checks. The running command keeps using the runtime
it already started.

The first invocation installs and starts the selected latest runtime without a
background check. Explicit `+vVERSION` and `+prerelease` invocations also retain
their direct behavior without an automatic check. No other leading `+` token is
reserved by the wrapper: for example, `+self-update` passes to the runtime
unchanged.

Automatic wrapper replacement accepts assets only from the official
`MidasAl/midas-lex` repository. Release validity and semantic version comparison
happen first; a same or older release causes no wrapper download or Windows
notice. Linux and macOS resolve the running executable instead of searching
`PATH`, require the canonical public wrapper asset and its exact one-line
same-name SHA-256 record, stage beside the executable, preserve its mode, sync,
and atomically rename. An adjacent lock and executable digest rechecks serialize
replacement; errors before rename remove staging and preserve the old wrapper.

A running Windows `.exe` is not replaced. Only when the release is newer, the
background child visibly warns that replacement is unsafe, names the canonical
path reported by the executing process, and asks the user to run
`cargo install midas-lex --force` after Midas Lex exits. Windows continues the
automatic runtime check. Wrapper or runtime update failures and the notice do
not change the current runtime command's result.

Runtime downloads and installs use one lock per data directory. The lock file is
`$MIDAS_LEX_VERUS_HOME/locks/install.lock`, or
the active default data directory's `locks/install.lock` when
`MIDAS_LEX_VERUS_HOME` is not set. Other wrapper processes using the same data
directory wait for that lock before installing a runtime.

`cargo test` covers pass-through, first-run policy, release ordering, the timer,
marker and locks, independent wrapper/runtime failures, checksum cleanup, atomic
replacement, permissions, and platform behavior without publication or an
uncontrolled network.

## Release assets

Runtime assets use this pattern:

```text
midas-lex-private-VERSION-TARGET
midas-lex-private-VERSION-TARGET.exe
midas-lex-private-VERSION-TARGET.sha256
midas-lex-private-VERSION-TARGET.exe.sha256
```

Public wrapper assets used by automatic updates use this pattern:

```text
midas-lex-VERSION-TARGET
midas-lex-VERSION-TARGET.exe
midas-lex-VERSION-TARGET.sha256
midas-lex-VERSION-TARGET.exe.sha256
```

The wrapper downloads the asset matching your platform and its same-name
`.sha256` file. Installation stops if the checksum does not match.

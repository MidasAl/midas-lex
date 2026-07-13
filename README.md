# Midas Lex

Midas Lex Helper (`midas-lex`) gives coding agents Verus guidance from the command line.

## Install

```sh
cargo install midas-lex
```

The install step puts `midas-lex` on your `PATH`. It does not download the Midas Lex
binary during crate installation.

## Use

```sh
midas-lex help
midas-lex docs
midas-lex docs read helper_step_protocol
midas-lex docs search invariant
midas-lex next-stage
midas-lex eula
```

The first run downloads the Midas Lex binary for your platform, verifies the
published SHA-256 checksum, stores the binary under `MIDAS_LEX_VERUS_HOME`, and then starts
it. If `MIDAS_LEX_VERUS_HOME` is not set, Midas Lex uses
`$XDG_DATA_HOME/midas-lex/verus` when `XDG_DATA_HOME` is set. Otherwise it uses
`$HOME/.midas-lex/verus`.

Normal invocations use the latest installed ordinary Midas Lex version. After
starting that binary, `midas-lex` may check for a newer release in the
background. Stable releases are preferred, but a pre-release can be selected when
no stable release exists. Background checks are throttled to once per hour per
platform.

Downloads and installs use one lock per data directory. The lock file is
`$MIDAS_LEX_VERUS_HOME/locks/install.lock`, or
the active default data directory's `locks/install.lock` when
`MIDAS_LEX_VERUS_HOME` is not set.

Use a version selector to opt in to a specific release, including a pre-release:

```sh
midas-lex +v0.0.1-alpha.1 docs
midas-lex +prerelease docs
```

The selector is consumed by the launcher, so the real Midas Lex binary receives
the remaining arguments unchanged. Environment variables are inherited by the
real binary.

By default, the launcher prefers ordinary GitHub releases and falls back to a
pre-release only when no stable release exists. Use a `+vVERSION` selector to
run an exact release tag. Use `+prerelease` to allow the newest non-draft semver
release, including alpha, beta, and release-candidate tags.

Set `MIDAS_LEX_VERUS_VERBOSE=1` to show the selected runtime version tag and
binary path. Set `MIDAS_LEX_VERUS_LOG=info` to show download and update logs.

## Releases

The wrapper downloads runtime assets from GitHub Releases using this pattern:

```text
midas-lex-private-VERSION-TARGET
midas-lex-private-VERSION-TARGET.exe
midas-lex-private-VERSION-TARGET.sha256
midas-lex-private-VERSION-TARGET.exe.sha256
```

Examples:

```text
midas-lex-private-v0.0.1-alpha.1-x86_64-unknown-linux-musl
midas-lex-private-v0.0.1-alpha.1-x86_64-pc-windows-msvc.exe
```

The supported targets are:

- `x86_64-unknown-linux-musl`
- `aarch64-unknown-linux-musl`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`
- `x86_64-pc-windows-msvc`
- `aarch64-pc-windows-msvc`

See `docs/release.md` for how wrapper releases and runtime downloads work.
Release notes are committed under `release-notes/`.

## Download Notice

The crates.io package and `cargo install midas-lex` download only this wrapper. The
wrapper downloads and stores the proprietary Midas Lex binary when users invoke
`midas-lex`. By using this CLI, users agree to the EULA available through
`midas-lex eula`.

## Licenses

See `LICENSES/README.md`.

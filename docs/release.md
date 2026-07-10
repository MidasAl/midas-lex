# Midas Lex releases

Midas Lex has two installable parts:

- the `midas-lex` wrapper crate from crates.io
- Midas Lex runtime binaries attached to GitHub Releases in this repository

Installing the crate puts the wrapper on your `PATH`. The wrapper downloads the
runtime binary the first time you run `midas-lex`, verifies its published
SHA-256 checksum, stores it under `MIDAS_LEX_HOME`, and then starts it.

If `MIDAS_LEX_HOME` is not set, the default storage directory is
`$HOME/.midas-lex/verus`.

## Version selection

By default, `midas-lex` uses the newest installed ordinary runtime binary. If no
ordinary runtime binary is installed yet, it downloads the newest published
ordinary semver GitHub Release and requires that release to include an asset for
your platform. GitHub pre-releases, semver pre-release tags, and draft GitHub
Releases are not selected by default.

A version selector opts in to a specific release, including a pre-release:

```sh
midas-lex +v0.0.1-alpha.1 docs
```

## Background updates

After starting the installed runtime binary, the wrapper may check for a newer
ordinary release in the background. Background checks are throttled to once every
30 minutes per platform.

If a newer ordinary release is available, the wrapper downloads and verifies it
for the next invocation. The running command keeps using the binary it already
started.

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

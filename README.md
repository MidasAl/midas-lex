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
published SHA-256 checksum, stores the binary under `MIDAS_LEX_HOME`, and then starts
it. If `MIDAS_LEX_HOME` is not set, Midas Lex uses `$HOME/.midas-lex/verus`.

Normal invocations use the latest installed Midas Lex version. After starting
that binary, `midas-lex` may check for a newer release in the background. Background
checks are throttled to once every 30 minutes per platform.

Use a version selector to run a specific release:

```sh
midas-lex +v0.0.1 docs
```

The selector is consumed by the launcher, so the real Midas Lex binary receives
the remaining arguments unchanged. Environment variables are inherited by the
real binary.

## Releases

Midas Lex release assets use this pattern:

```text
midas-lex-VERSION-TARGET
midas-lex-VERSION-TARGET.exe
midas-lex-VERSION-TARGET.sha256
midas-lex-VERSION-TARGET.exe.sha256
```

Examples:

```text
midas-lex-v0.0.1-x86_64-unknown-linux-musl
midas-lex-v0.0.1-x86_64-pc-windows-msvc.exe
```

The supported targets are:

- `x86_64-unknown-linux-musl`
- `aarch64-unknown-linux-musl`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`
- `x86_64-pc-windows-msvc`
- `aarch64-pc-windows-msvc`

## Download Notice

The crates.io package and `cargo install midas-lex` download only this wrapper. The
wrapper downloads and stores the proprietary Midas Lex binary when users invoke
`midas-lex`. Use of the Midas Lex binary is governed by the EULA available through
`midas-lex eula`.

## Licenses

See `LICENSES/README.md`.

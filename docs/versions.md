# Versions and prereleases

Midas Lex has two versioned parts:

- the public `midas-lex` wrapper installed from crates.io or a GitHub Release
- the private runtime that the wrapper downloads from GitHub Releases

Choosing a wrapper version does not select the runtime version. Choose each
part only when you need something other than the stable default.

## Stable default

Install the current stable wrapper:

```sh
cargo install midas-lex
```

Run Midas Lex normally:

```sh
midas-lex next-stage
```

The wrapper uses the newest verified ordinary runtime already installed. On a
first run, it prefers the newest non-draft stable semantic-version Release. It
falls back to a prerelease only when no stable Release exists.

## Exact wrapper version

Install an exact wrapper crate version from crates.io:

```sh
cargo install midas-lex --version 0.0.2
```

Use `--force` when replacing an installed wrapper:

```sh
cargo install midas-lex --version 0.0.2-alpha.1 --force
```

The requested crate version must have been published to crates.io. Installing
an exact wrapper does not pin the downloaded runtime.

## Exact runtime version

Put an exact GitHub Release tag before the runtime command:

```sh
midas-lex +v0.0.2-alpha.1 next-stage
```

The wrapper consumes `+vVERSION`, downloads that exact tagged Release when
needed, and passes the remaining arguments to the selected runtime. The
selector applies to that invocation; include it again on later commands that
must use the same version.

## Allow prereleases

Allow the highest non-draft semantic version, including alpha, beta, and
release-candidate versions:

```sh
midas-lex +prerelease next-stage
```

`+prerelease` allows prereleases; it is not a prerelease-only channel. A stable
Release wins when its semantic version is higher. Use `+vVERSION` when an exact
prerelease is required.

Explicit `+vVERSION` and `+prerelease` runs do not start the automatic
background update check. Automatic and `+prerelease` latest-version selection
excludes draft GitHub Releases.

## Prerelease Release pages

A GitHub prerelease should distinguish the wrapper from the runtime.

When the stable wrapper can run the prerelease runtime, publish these commands:

```sh
cargo install midas-lex
midas-lex +v0.0.2-alpha.1 help
```

When the prerelease also requires a prerelease wrapper published to crates.io,
publish both exact versions:

```sh
cargo install midas-lex --version 0.0.2-alpha.1 --force
midas-lex +v0.0.2-alpha.1 help
```

In the Cargo command, replace `0.0.2-alpha.1` with the crates.io version (without
the tag's leading `v`). In the runtime command, replace `v0.0.2-alpha.1` with
the exact GitHub Release tag. If the wrapper and runtime versions differ, use
their respective values. If the wrapper prerelease is not on crates.io, direct
users to the public wrapper asset for their platform. Assets containing
`private` are runtimes used by the wrapper and are not the manual-install
choice.

## Storage and diagnostics

The runtime is stored under `MIDAS_LEX_VERUS_HOME`. When it is unset, the
default is `$XDG_DATA_HOME/midas-lex/verus` if `XDG_DATA_HOME` is set, otherwise
`$HOME/.midas-lex/verus`.

Show the selected runtime tag and binary path:

```sh
MIDAS_LEX_VERUS_VERBOSE=1 midas-lex +prerelease help
```

Show download and update logs:

```sh
MIDAS_LEX_VERUS_LOG=info midas-lex +v0.0.2-alpha.1 help
```

Default runs may perform a stable-preferred background update check, throttled
to once per hour. See [Midas Lex releases](release.md) for integrity checks,
asset names, locks, and platform-specific wrapper updates.

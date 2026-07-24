# Midas Lex

**Trust infrastructure for robotics.**

The `midas-lex` CLI guides coding agents through the formal verification of
Rust software using [Verus](https://github.com/verus-lang/verus),
a language that builds on Rust and checks code against formal specifications.
With proof annotations, Verus determines whether
the code satisfies the specification.
When the verifier passes, you have high confidence that the code is correct if
the specification is correct.

**Agents already code fast; with Verus and Midas Lex,
they can code correctly.**

## Why Verus + Agents

Rust makes individual components safer through memory safety, strong types, and
predictable performance.
Midas Lex extends those guarantees across robotics infrastructure by
making configurations, interfaces, assumptions, and system behavior verifiable,
helping teams prevent failures before deployment.

With Midas Lex, humans express program behavior in English and focus on
high-level design and declarative specifications.
Agents write Verus specifications, Rust code, and proofs that
the code satisfies those specifications.

> **Agents supply velocity. Verus supplies correctness. Midas Lex connects them.**

## Install

Recommended: install [Rust](https://rust-lang.org/tools/install/) and run:

```sh
cargo install midas-lex
```

<details><summary>Alternative: directly install from Releases.</summary>

Go to the latest [Releases](https://github.com/MidasAl/midas-lex/releases),
expand <kbd>Assets</kbd> if it is collapsed,
download the `midas-lex-vx.y.z-your-platform` binary (not the `_private` one!)
and put it on your path.
</details>

The installed `midas-lex` is a wrapper that automatically downloads and
runs the latest Midas Lex binary for your platform.

## Use

Tell your agent:

```
Use the `midas-lex` CLI for guidance.
```

That's it!

Midas Lex first maps each English requirement to its public executable path,
contract, abstract state transition, and verifier-checked refinement evidence.
It keeps unresolved or helper-only work visible, bounds repeated proof attempts,
and calls for independent semantic review before completion. Agents keep the
working goal and stage record in task-owned ephemeral state, not generated
files committed to your project.

## Usage Details

```sh
midas-lex help

Midas Lex Helper provides end-to-end guidance for software development in Verus, from English requirements to verifier-checked public behavior.
By using midas-lex, you agree to our EULA per `midas-lex eula`.

Usage: midas-lex [COMMAND]

Commands:
  next-stage  Get guidance on what to do next given your circumstances
  docs        Get guidance for specific Verus, spec, and proof topics
  profile     Show, set, or check the Cargo.toml guidance filter
  eula        Print the EULA notice
  help        Print this message or the help of the given subcommand(s)
```

The first run downloads the Midas Lex binary for your platform,
stores the binary under `MIDAS_LEX_VERUS_HOME`, and then starts it.
`MIDAS_LEX_VERUS_HOME` defaults to `$XDG_DATA_HOME/midas-lex/verus` when
`XDG_DATA_HOME` is set, otherwise it defaults to `$HOME/.midas-lex/verus`.

Normal invocations use the latest installed ordinary Midas Lex version.
After starting that binary, `midas-lex` may check for a newer stable release in
the background. Background checks are throttled to once per hour.

Downloads and installs use one lock per data directory. The lock file is
`$MIDAS_LEX_VERUS_HOME/locks/install.lock`.

Use a version selector to opt in to a specific release, including pre-release:

```sh
midas-lex +v0.0.1 next-stage
midas-lex +prerelease next-stage
```

The `+` selector is consumed by the wrapper, so the internal Midas Lex binary receives
the remaining arguments unchanged.

Set `MIDAS_LEX_VERUS_VERBOSE=1` to show the selected runtime version tag and
binary path. Set `MIDAS_LEX_VERUS_LOG=info` to show download and update logs.

## Automatic updates

On a default invocation with a verified runtime already installed, the wrapper
starts that runtime before starting one throttled background child. The child
uses one stable-preferred, non-draft release lookup to check both the public
wrapper and runtime for the next invocation. Network, integrity, permission, or
replacement failures are warnings and do not change the current runtime command.
The first run installs and starts the runtime without a background check;
explicit version selectors also keep their existing direct behavior.

On Linux and macOS, a newer wrapper is verified against its exact same-name
SHA-256 record, staged beside the resolved running executable, and atomically
renamed over that path with its executable mode preserved. An equal or older
release causes no wrapper download or notice, including when a local build is
newer. On Windows, only a newer release produces a visible background warning;
it names the canonical path of the running `.exe` and asks the user to run
`cargo install midas-lex --force` after Midas Lex exits. Automatic runtime
updates and the current command continue normally.

## Releases

See `docs/release.md` for how wrapper releases and runtime downloads work.
Release notes are committed under `release-notes/`.

## Licenses

See `LICENSES/README.md`.

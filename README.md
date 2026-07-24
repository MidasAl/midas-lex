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

For exact versions, prereleases, update behavior, and prerelease Release-page
instructions, see [Versions and prereleases](docs/versions.md).

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

The first run downloads, verifies, and starts the runtime for your platform.
Default runs prefer stable releases and may check for updates in the background.
See [Versions and prereleases](docs/versions.md) to pin a release or opt in to
prereleases.

## Releases

See `docs/release.md` for how wrapper releases and runtime downloads work.
Release notes are committed under `release-notes/`.

## Licenses

See `LICENSES/README.md`.

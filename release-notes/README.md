# Midas Lex release notes

This directory contains public release notes for Midas Lex versions.

Each version uses a file named `release-notes/VERSION.md`, where `VERSION`
includes the leading `v`, such as `release-notes/v0.0.1.md`.

Each file begins with that version's user-visible Midas Lex and wrapper changes,
then gives the shared installation guidance used on the GitHub Release page.
The page directs users to wrapper assets and identifies `private` assets as
internal runtime artifacts.

For a prerelease, the Release page must also link to
[`docs/versions.md`](../docs/versions.md) and include the applicable exact
commands from its “Prerelease Release pages” section. Do not present ordinary
`cargo install midas-lex` as sufficient to select a prerelease runtime.

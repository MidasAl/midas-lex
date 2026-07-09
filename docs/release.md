# Midas Lex release process

Midas Lex has two public release surfaces:

- the `midas-lex` wrapper crate on crates.io
- locally private-built Midas Lex runtime binaries attached to a GitHub Release
  in this public repository

Use the same version for both surfaces. The first release line is an early alpha,
for example `0.0.1-alpha.1` for Cargo and `v0.0.1-alpha.1` for GitHub. Semver
prerelease tags are GitHub prereleases.

```sh
VERSION=v0.0.1-alpha.1
CRATE_VERSION=${VERSION#v}
```

## Release notes

Release notes live in this public repository under `release-notes/VERSION.md`.
Write them after the private runtime build finishes. The notes are the public
account of the private-built binary, so derive them from the private build
results, generated checksums, and user-visible runtime changes.

The same file is passed as the GitHub Release notes when creating or updating the
draft release. Commit and push that file before uploading the draft. Keep private
build logs, private paths, private source filenames, credentials, and
internal-only debugging details out of the public notes.

Each release note should state:

- the version and whether it is alpha, beta, or stable
- the user-visible runtime changes
- the supported target binaries attached to the GitHub Release
- the SHA-256 checksums for the attached binaries
- the wrapper crate version for this release
- that using this CLI means agreeing to the EULA available through
  `midas-lex eula`

## Wrapper crate

Before publishing the crate, run:

```sh
git status --short
cargo fmt --check
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo package --locked --list
cargo publish --dry-run --locked
```

Commit and push the public version, docs, and release-note changes before
running `cargo package` or `cargo publish`. Inspect the package list before
publication. It should contain only Cargo's generated `.cargo_vcs_info.json`, the
public wrapper source, manifests, lockfile, README, license explanation, docs,
tests, and release notes.

After the public GitHub Release has been reviewed, approved, and published,
publish the wrapper crate:

```sh
gh release edit "$VERSION" \
  --repo MidasAl/midas-lex \
  --draft=false \
  --prerelease
gh release view "$VERSION" \
  --repo MidasAl/midas-lex \
  --json isDraft,isPrerelease,tagName \
  --jq '.'
cargo publish --locked
```

## GitHub Release

Create or update a draft GitHub Release in `MidasAl/midas-lex` using the same
tag as the runtime assets, such as `v0.0.1-alpha.1`. Semver prerelease tags must
be marked as GitHub prereleases. Attach the locally private-built runtime
binaries, public wrapper binaries, and their same-name `.sha256` files. Use
`release-notes/VERSION.md` as the release notes body.

GitHub is the publication surface only. Build release artifacts locally from the
private checkout before creating or updating the draft release.

After the wrapper checks and package dry run pass, create and push the public
repository tag on the reviewed public wrapper commit before uploading the draft
GitHub Release.

Publish the GitHub Release only after the attached binaries, checksums, and
rendered release notes have been reviewed. The GitHub Release must be published
before `cargo publish`, because a newly installed wrapper downloads the runtime
binary from the public GitHub Release on first use.

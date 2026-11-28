# Cutting a release

A release is a git tag. The tags follow the convention VisionCraft shares with GameCraft and
Salvo: `vMAJOR.MINOR.PATCH` is a release, `vMAJOR.MINOR.PATCH-<label>.<n>` (for example
`v0.2.0-rc.1`) is a pre-release, and the tag is annotated, with the release title as its
message. Below 1.0 a MINOR bump may break; a PATCH never does.

The pipeline is [`.github/workflows/release.yml`](../.github/workflows/release.yml). It runs when
a `v*` tag is pushed. Creating or pushing a tag and publishing a release are outward actions:
each needs the owner's go.

## The steps

1. **Bump the version** in `[workspace.package] version` of the root `Cargo.toml`. Every crate
   inherits it with `version.workspace = true`; the pipeline fails when a package disagrees.
   `Cargo.lock` carries the version of every package, so run `cargo update -w` and commit both
   files. (The release builds with `--locked`: a stale lock file stops them.)

2. **Commit** as `chore(release): vX.Y.Z`.

3. **Get the commit onto `main`**, through a pull request that passes CI.

4. **Rehearse.** In Actions, run the "release" workflow on `main`, leave `tag` empty and
   `dry_run` on (the default). It verifies that commit, runs the CI gate on it and builds every
   archive under the workspace version. The archives, `SHA256SUMS` and the notes are kept as
   workflow artifacts; nothing is published. Fix and repeat until it is green. No tag exists
   yet at this point, so nothing can be published by accident.

5. **Check and tag the commit** on `main`:

   ```
   git fetch origin
   git tag -a vX.Y.Z -m "<title>" <commit on main>
   tools/release/verify-tag.sh vX.Y.Z
   ```

   The message is the release title. Annotated tags only: a lightweight tag has no title, and
   the pipeline rejects it. The script runs the pipeline's checks on the tag; it prints what
   the release will be called.

6. **Push the tag**: `git push origin vX.Y.Z`. That push is the go. The pipeline verifies the
   tag, runs the gate, builds, checksums, writes the notes and publishes.

7. **Look at the release page**: the title, the notes, three archives and `SHA256SUMS`.

A tag is never moved. A wrong one is deleted together with its release, and cut again under a
new patch version.

## Pre-releases

A pre-release is the same, with `X.Y.Z-rc.1` as the version in `Cargo.toml` and
`vX.Y.Z-rc.1` as the tag. It may be on any branch; a release tag must be reachable from
`origin/main`. GitHub shows it as a pre-release. Its notes cover everything since the previous
release, the same as the notes of the release that follows it: a pre-release tag is never "the
previous release".

## What the pipeline does

| Job | What it does |
|---|---|
| `verify` | `tools/release/verify-tag.sh`: the tag exists and is annotated, is named `v<workspace version>`, and a release tag is on `main`. Fails before anything is built. |
| `gate` | The CI workflow (`.github/workflows/ci.yml`), called on the tagged commit: fmt, clippy, tests, the wasm32 check and cargo-deny. |
| `build-desktop`, `build-web`, `build-source` | The three archives, below. |
| `checksums` | `SHA256SUMS` over the three archives. |
| `notes` | `tools/release` (the crate `release-notes`): the notes, below. |
| `publish` | `gh release create`, only when every job before it succeeded. |

Every job after `verify` checks out the commit that `verify` verified. The workflow can also be
started by hand (`workflow_dispatch`) with a `tag`: with `dry_run` on it rehearses an existing
tag; with `dry_run` off it builds and publishes again, which is the way to recover when
publishing failed after a correct tag push (GitHub refuses to create a release that exists).

## The archives

Each is `visioncraft-<what>-<version>-<target>.tar.gz` with one top-level directory of the same
name:

- `visioncraft-desktop-<version>-x86_64-unknown-linux-gnu.tar.gz`: `visioncraft` (the desktop
  window) and `visioncraft-cli`, with `LICENSE`, `README.md` and `ATTRIBUTION.md`. Built on the
  CI runner's Ubuntu, so it needs that glibc or newer.
- `visioncraft-web-<version>-wasm32-unknown-unknown.tar.gz`: the Trunk build of
  `apps/visioncraft`, to serve from a directory.
- `visioncraft-source-<version>-any.tar.gz`: `git archive` of the tagged commit, without
  `assets/`.

The asset library is in none of them (see below).

## Release notes

`cargo run -p release-notes -- vX.Y.Z` prints what the release will say; run it before pushing
the tag. The notes are:

- the Conventional Commits since the previous release tag, newest first: breaking changes (`!`
  or a `BREAKING CHANGE:` footer), then `feat`, `fix` and `perf`, each as
  `- **scope:** description`; every other commit is folded away under "Other changes";
- the git tags the manifests pin (`Pins`);
- how to get the asset library.

A commit outside the Conventional Commits scheme (`Curved houses: ...`) lands under "Other
changes" with its subject as is, so a feature that should be listed needs a `feat:` subject.
`--previous TAG` overrides where the notes start.

## The asset library

`assets/` is far too large to ship and is rebuilt from its CC0 and CC-BY sources anyway: build
it from a source checkout with `tools/visioncraft-assets` (see "Assets" in the README). The
desktop application takes the directory with `--assets <dir>`; the web build loads it from
`assets/` beside the page.

## Pinning a release from another project

The crates are not on crates.io (`publish = false`): depend on them by git, by tag. GameCraft
pins the renderer and the plant generator in the root `Cargo.toml`:

```toml
visioncraft-render = { git = "https://github.com/Skediaio/visioncraft", tag = "v0.1.0" }
visioncraft-flora  = { git = "https://github.com/Skediaio/visioncraft", tag = "v0.1.0" }
```

One tag for both. To move to a newer release, change the tag of both lines: the next cargo
command fetches it and rewrites the entry in `Cargo.lock`, which is committed with the change.
A tag can only be pinned once it is pushed, so VisionCraft releases first and GameCraft follows.
`rev = "<commit>"` pins a commit instead; use it only for a change that is not released yet.

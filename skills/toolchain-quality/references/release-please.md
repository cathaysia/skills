# release-please Setup

Use this reference when the repository needs automated release PRs, version
bumps, and tag creation through `release-please`.

Base the setup on repository inspection first. A stable baseline for this
workspace is:

- `googleapis/release-please-action@v4`
- `release-please-config.json`
- `.release-please-manifest.json`
- workflow file at `.github/workflows/release-please.yaml`
- `include-v-in-tag: true`
- `include-component-in-tag: false`
- package `"."` with `release-type: "simple"`

## Inspect before proposing config

Check these signals before writing files:

- default branch name such as `main` or `master`
- whether the repo is single-package or monorepo-like
- whether versions live in `Cargo.toml`, `Cargo.lock`, `package.json`, or mixed
  files
- whether the repo already uses a dedicated token such as `GH_TOKEN`
- whether there is already an existing release workflow or tag naming pattern

## Always generate choices first

`release-please` has many knobs. When the repository has no established setup,
generate a short option list and ask the user to choose before editing files.
Keep it concrete. Recommend one option per category.

Suggested categories:

1. Branch target
2. Tag style
3. Release strategy
4. Token strategy
5. Version sync targets

Example prompt shape:

```text
I found no existing release-please setup. Choose these defaults before I write files:
1. Branch: main (recommended) / master
2. Tag style: v1.2.3 (recommended) / 1.2.3
3. Release type: simple (recommended) / language-specific custom
4. Token: GH_TOKEN (recommended) / GITHUB_TOKEN
5. Extra file sync: minimal manifest only / Rust version files / Rust + Node.js version files
```

If the repository already has clear conventions, follow them instead of asking
about options that are already implied.

## Recommended baseline

When no stronger repo convention exists, recommend this baseline:

### `release-please-config.json`

```json
{
  "$schema": "https://raw.githubusercontent.com/googleapis/release-please/main/schemas/config.json",
  "include-v-in-tag": true,
  "include-component-in-tag": false,
  "packages": {
    ".": {
      "release-type": "simple"
    }
  }
}
```

### `.release-please-manifest.json`

```json
{
  ".": "0.1.0"
}
```

Replace `0.1.0` with the repository's current release version.

### `.github/workflows/release-please.yaml`

```yaml
name: release-please

on:
  push:
    branches:
      - <default-branch>
  workflow_dispatch:

permissions:
  contents: write
  pull-requests: write
  actions: write

jobs:
  release_please:
    runs-on: ubuntu-latest
    outputs:
      releases_created: ${{ steps.release.outputs.releases_created }}
      paths_released: ${{ steps.release.outputs.paths_released }}
    steps:
      - name: Run release-please
        id: release
        uses: googleapis/release-please-action@v4
        with:
          token: ${{ secrets.GH_TOKEN }}
          config-file: release-please-config.json
          manifest-file: .release-please-manifest.json
```

Replace `<default-branch>` with the detected default branch.

## Common option presets

Offer concise presets rather than an open-ended schema dump.

### Branch target

- `main` when the repo default branch is `main`
- `master` when the repo default branch is `master`

Do not guess a different branch from the actual repository default.

### Tag style

- Recommended: `include-v-in-tag: true`
- Alternative: `include-v-in-tag: false`

Keep `include-component-in-tag: false` for single-package repos unless the repo
already relies on component-prefixed tags.

### Release strategy

- Recommended: `release-type: "simple"` for most Rust, Node.js, and mixed
  repos in this workspace
- Alternative: a language-specific strategy only when the repo already depends
  on release-please behavior tied to that ecosystem

Default to `simple` unless there is a concrete reason not to.

### Token strategy

- Recommended: `secrets.GH_TOKEN` when the repository already uses a PAT-backed
  release workflow
- Alternative: `secrets.GITHUB_TOKEN` when the repo intentionally relies on the
  default GitHub token and its permission model is sufficient

Follow existing repo secret naming if present.

### Extra file sync presets

- Minimal:
  only manifest-managed versioning
- Rust:
  add `extra-files` entries for `Cargo.toml`, workspace dependency versions,
  and any `Cargo.lock` package versions that must stay in sync
- Rust + Node.js:
  include the Rust set plus `package.json` version files for web or desktop UI
  packages

Build `extra-files` from actual repository version locations. Do not fabricate
paths or package names.

## Derived usage patterns

The recommended usage pattern is:

- use a single `release-please-config.json` plus `.release-please-manifest.json`
- run `googleapis/release-please-action@v4` from
  `.github/workflows/release-please.yaml`
- keep `include-v-in-tag: true`
- keep `include-component-in-tag: false` for single-package repositories
- start with `release-type: "simple"`
- use an explicit token input in the workflow
- add `extra-files` only for real version-bearing files that must move in lockstep

Use `extra-files` when:

- Rust workspace package versions must stay synchronized
- `Cargo.lock` package entries need the same version bump as the release
- Node.js package versions must stay aligned with the main release version
- mixed Rust and Node.js repositories expose one coordinated release number

Do not add `extra-files` for files that do not actually participate in release
versioning.

## Editing policy

- Reuse an existing release-please workflow if present instead of creating a
  second one.
- Keep workflow permissions explicit.
- Keep file names consistent with the baseline unless the repository already
  uses different names.
- When adding `extra-files`, only include real version-bearing files that need
  synchronized bumps from the release PR.

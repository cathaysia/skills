# Rust Toolchain Setup

Use this reference when the target repository is Rust-based or contains a Rust
subproject that needs toolchain normalization.

## Detection signals

Treat the project as Rust when one or more of these are present:

- `Cargo.toml`
- `Cargo.lock`
- `rust-toolchain.toml` or `rust-toolchain`
- `Cross.toml`
- `.cargo/config.toml`
- Rust jobs in `.github/workflows/*.yml` or `.github/workflows/*.yaml`

## `rust-toolchain.toml`

Create or update `rust-toolchain.toml` with exactly this shape:

```toml
[toolchain]
channel = "<version>"
```

Rules:

- Use a concrete version such as `1.88.0`.
- Do not use `stable`, `beta`, or bare `nightly`.
- If the project uses nightly, pin it as `nightly-YYYY-MM-DD`.
- Do not add `components`.
- Do not add unrelated keys unless the user explicitly requests them.

Examples:

```toml
[toolchain]
channel = "1.88.0"
```

```toml
[toolchain]
channel = "nightly-2026-03-20"
```

## `cross` handling

If the repository uses `cross`, create `Cross.toml` with this content unless the
project already has a more specific target matrix that the user wants to
preserve:

```toml
[target.aarch64-unknown-linux-musl]
image = "ghcr.io/cross-rs/aarch64-unknown-linux-musl:edge"
```

Consider `cross` in use when:

- `Cross.toml` already exists
- `cross` appears in CI, scripts, `Makefile`, or docs
- the user explicitly asks to support cross-compilation through `cross`

## GitHub Actions snippet for `cross`

When CI needs to install `cross`, use this step:

```yaml
- name: Install cross
  uses: baptiste0928/cargo-install@v3
  with:
    crate: cross
    git: https://github.com/cross-rs/cross
    rev: 8633ec65ab914015c2444c732568b414bd3c47cf
```

Rules:

- Do not switch this to a crates.io install by default.
- Keep the pinned `rev` aligned with the repository's Rust toolchain
  expectations so CI does not request a newer `rustc` than the repository uses.
- If the repository already pins another `cross` revision for a documented Rust
  compatibility reason, preserve that existing choice unless the user asks to
  change it.

## Editing policy

- Prefer minimal edits if `rust-toolchain.toml` or GitHub Actions already exist.
- Preserve unrelated workflow steps and job settings.
- If the repository already has an explicit toolchain pin that conflicts with a
  user-supplied version, stop and clarify instead of silently replacing it.

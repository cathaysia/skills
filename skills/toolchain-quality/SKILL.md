---
name: toolchain-quality
description: Set up toolchains, release automation, and baseline code-quality files for Rust projects, frontend or Next.js style Node.js projects, and shared repository bootstrap. Use when Codex needs to pin Rust or Node.js tooling, add release-please, add baseline quality config files, or align CI and local setup with repository standards.
---

# Toolchain Quality

Use this skill when the user wants to initialize or normalize project toolchain
and quality configuration.

## Classify the target project first

Inspect the repository before editing files. Prefer concrete signals over
guessing:

- Rust project: `Cargo.toml`, Rust workspace files, `.rs` sources, `cross`,
  GitHub Actions Rust jobs
- Node.js frontend project: `package.json`, `pnpm-lock.yaml`, `next.config.*`,
  frontend app directories, `src/` with `ts`, `tsx`, `js`, or `jsx`
- Release automation request: repository needs `release-please` config,
  manifest, or workflow support
- Shared bootstrap only: repository needs `.editorconfig` or
  `.pre-commit-config.yaml` regardless of language stack

If the repository spans multiple stacks, apply each relevant reference in a
controlled order instead of treating the project as single-stack.

## Keep this entry point short

Use this file for routing and guardrails. Load the matching reference when you
need exact file content, update rules, or stack-specific decisions:

- `references/rust.md`
- `references/frontend-nodejs.md`
- `references/release-please.md`
- `references/common.md`

## Workflow

1. Inspect the target repository and identify which stack rules apply.
2. Confirm or infer the concrete toolchain versions from user input or existing
   repository context.
3. If the request includes `release-please`, inspect the repo and generate a
   short list of concrete configuration options for the user to choose from
   before writing files.
4. Apply the relevant reference instructions exactly. Do not loosen pinned
   versions into floating channels.
5. Preserve existing project choices where the reference says create only when
   missing.
6. When modifying existing files such as `package.json` or GitHub Actions, keep
   the change minimal and local to the requested toolchain setup.

## Guardrails

- Do not invent version numbers. Use values provided by the user or already
  established in the repository.
- For Rust toolchains, do not write `stable`, `beta`, bare `nightly`, or
  `components` into `rust-toolchain.toml`.
- For Node.js frontend setup, treat Next.js as one example of a Node.js frontend
  project, not the only supported shape.
- For `release-please`, do not silently pick from the many supported knobs when
  the repository has no existing convention. Generate a few concrete options,
  recommend one, and let the user choose.
- For shared quality files, do not overwrite `.editorconfig` or
  `.pre-commit-config.yaml` if the project already has them unless the user
  explicitly asks to replace them.
- When the project uses `cross`, update both repository config and CI guidance
  so the pinned `cross` install path matches the repository's Rust toolchain
  constraints.

# Node.js Frontend Setup

Use this reference for frontend projects built on the Node.js toolchain,
including Next.js and similar web applications.

## Detection signals

Treat the project as a Node.js frontend project when repository inspection finds
signals such as:

- `package.json`
- `pnpm-lock.yaml`
- `next.config.js`, `next.config.mjs`, or `next.config.ts`
- frontend source files under `src/`, `app/`, `pages/`, or `components/`
- build scripts in `package.json` for web frameworks or bundlers

Next.js is one valid case, but this reference applies to broader Node.js
frontend stacks as well.

## `.npmrc`

Create `.npmrc` with:

```text
use-node-version=<version>
```

Replace `<version>` with the concrete Node.js version provided by the user or
already implied by the repository. Do not invent one.

Example:

```text
use-node-version=22.14.0
```

## `package.json`

Ensure `package.json` contains:

```json
{
  "packageManager": "pnpm@<version>"
}
```

Rules:

- Add or update only the `packageManager` field that is relevant to the request.
- Preserve existing formatting and unrelated fields.
- Use the provided pnpm version exactly.

## `biome.jsonc`

If the project needs the shared Biome baseline, create `biome.jsonc` from the
workspace's canonical Biome template rather than rewriting it by memory.

## Editing policy

- Keep changes minimal when the repository already has Node.js tooling config.
- If the project already uses a different package manager by explicit choice,
  stop and clarify before forcing pnpm.
- If a repository already has `biome.jsonc`, preserve it unless the user asks to
  replace or align it.

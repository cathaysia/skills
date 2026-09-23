---
name: doc-lookup
description: Search, look up, and verify technical documentation, APIs, SDKs, libraries, and frameworks across external sources and local code. Always activate whenever researching external libraries, verifying API signatures, checking configuration options, finding SDK methods, diagnosing version differences, or whenever the user asks for documentation. Strictly forbids relying on parametric model memory: treats code as ground truth, trusts auto-generated API docs, vets official docs for staleness, filters out Context7 recommendations to keep only facts, and uses search engines to locate canonical sources.
---

# Documentation Lookup and Verification

Search, retrieve, and factually verify technical documentation, APIs,
frameworks, and libraries. Never answer documentation questions or implement
library calls based on model memory alone. Every claim must be grounded in
verifiable external sources or codebase reality.

## Core Principle: Zero Memory Reliance

Model memory (pre-trained parametric knowledge) is frozen in time and inherently
prone to subtle hallucinations, obsolete parameters, hallucinated methods, and
outdated idioms.

- **Never rely on memory** for API signatures, configuration schemas, method
  names, CLI flags, or library behavior.
- Treat internal knowledge **only as search query hints**, never as factual
  ground truth.
- Always retrieve and verify from empirical sources: code, auto-generated API
  docs, official documentation, Context7, and search engines.

---

## Hierarchy of Truth (Trust Levels)

When researching or verifying documentation, evaluate information sources
strictly according to this hierarchy:

```
[Level 1: Code] (Absolute Ground Truth)
       │
[Level 2: Auto-Generated API Docs] (Near-Code Trust — Direct AST/Type Reflection)
       │
[Level 3: Official Documentation & Hand-Written Guides] (High Trust — Guard Against Staleness)
       │
[Level 4: Context7 MCP] (Model-Mediated — Filter Out Suggestions, Retain Facts Only)
       │
[Level 5: Search Engines] (Discovery & Canonical Source Locator)
       │
[Level 0: Model Memory] (FORBIDDEN as a Source of Truth)
```

### Level 1: Code is Always Trustworthy (Ground Truth)

Source code is executable reality. It does not suffer from human documentation
lag, expired tutorials, or hallucination.

- **What counts as Code**:
  - Local installed packages (e.g., `node_modules`, `vendor/`, `target/`,
    virtualenv `site-packages`, Cargo checkout sources).
  - Concrete type definition files (`.d.ts`, `.pyi`, Rust `.rs` trait and struct
    declarations, Go `.go` interface definitions).
  - Upstream Git repositories at the tagged commit matching the project's
    dependency version.
  - Project code indexed by CodeGraph (`.codegraph/`).
- **Contradiction Rule**: If official narrative prose documentation and actual
  code contradict each other, **code always wins**.

### Level 2: Auto-Generated API Documentation (Near-Code Trust)

Documentation mechanically extracted from source code ASTs, comments, and type
annotations (e.g., `rustdoc` on docs.rs, TypeDoc, godoc / pkg.go.dev, Sphinx
auto-doc, Swagger/OpenAPI generated directly from code models).

- **Why it is trusted**: Unlike hand-written narrative guides, auto-generated
  API references update automatically with code builds and accurately reflect
  exported types, method signatures, parameter names, and default values.
- **Exempt from narrative staleness**: If an auto-generated API reference is
  tied to a specific version tag, its signatures and symbols match that release
  exactly.

### Level 3: Official Documentation & Hand-Written Guides (High Trust, Guard Against Staleness)

Official documentation portals, framework guides, READMEs, migration guides, and
maintainer blog posts have high authority, but:

- **Watch out for outdated information**: Hand-written prose, narrative
  tutorials, and getting-started examples frequently rot across minor and major
  version updates while the framework evolves.
- **Staleness Audit Checklist**:
  1. **Version match**: Check the version selector in the URL or page header.
     Ensure the doc version matches the version pinned in the project's manifest
     (`package.json`, `Cargo.toml`, `go.mod`, etc.).
  2. **Publication / update dates**: Check if the guide was written for a legacy
     major release.
  3. **Deprecation tags**: Look for deprecation notices or "migrated to..."
     banners.
  4. **Cross-check against Level 1/2**: If a guide shows an example that uses
     methods not found in the installed type definitions or auto-generated API
     docs, the guide is stale.

### Level 4: Context7 (Model-Mediated: Ignore Suggestions, Retain Facts Only)

Context7 is an MCP tool (`resolve-library-id`, `query-docs`) that queries
indexed documentation. Because its output is synthesized and mediated by models:

- **Do NOT blindly trust Context7**:
  - Context7 outputs can synthesize commentary, summarize loosely, or inject
    model bias and subjective preferences.
- **Strip suggestions and recommendations**:
  - **Discard**: Opinions on architecture, "best practices", advice on code
    organization, styling suggestions, and speculative commentary.
- **Retain ONLY hard facts**:
  - **Keep**: Exact symbol names, exported function signatures, parameter names
    and types, configuration property keys, error codes, and literal code
    snippets quoting real documentation.
- **Verify before adopting**: When using facts retrieved from Context7, confirm
  their validity against local type definitions or official docs when critical.

### Level 5: Search Engines (Discovery & Verification)

Search engines (`search_web`) find canonical documentation sites, official
repositories, release notes, breaking change issues, and RFCs.

- Use search engines to locate primary sources (Levels 1–3).
- Do not accept secondary aggregators, scrape blogs, or forum answers as fact
  without validating them against official documentation or code.

---

## Step-by-Step Lookup Workflow

Follow this procedure whenever looking up documentation:

### Step 1: Pin the Target Version

Before searching, inspect the project's manifest file to determine the exact
version of the target library:

- Node.js: `package.json` / `package-lock.json` / `pnpm-lock.yaml`
- Rust: `Cargo.toml` / `Cargo.lock`
- Go: `go.mod`
- Python: `pyproject.toml` / `requirements.txt` / `poetry.lock`

_Never search generic or latest unversioned documentation without knowing the
active version in the project._

### Step 2: Check Local Code & Type Definitions First

If the library is already installed in the workspace:

1. Inspect local type definitions (e.g. `node_modules/<pkg>/dist/*.d.ts`,
   `Cargo.lock` crate source, or Go package cache).
2. If indexed by CodeGraph, use `codegraph_explore` or `codegraph explore` to
   inspect symbols and definitions directly.
3. If the type definition or source code confirms the method signature or config
   schema, you have instant Level-1 ground truth.

### Step 3: Query Documentation Channels

When local types are insufficient (e.g., conceptual questions, complex
configurations, or library not yet installed):

1. **Context7 Querying**:
   - Call `resolve-library-id` with the library name and lookup topic.
   - Select the best ID (preferring version-specific IDs when applicable).
   - Call `query-docs` scoped to a single concept.
   - **Filter step**: Immediately discard all opinions, architectural advice, or
     suggestions. Extract only the factual symbols, signatures, and config keys.

2. **Web Search & Canonical Docs**:
   - Use `search_web` with targeted queries including the library name, pinned
     version, and exact keyword (e.g.
     `"<library>" "<version>" "<feature>"
     docs`).
   - Use `read_url_content` to read official documentation pages directly.

### Step 4: Cross-Check and Audit for Staleness

- Compare retrieved documentation against the pinned version.
- If narrative tutorials conflict with auto-generated API docs or type
  definitions, discard the narrative tutorial and follow the auto-generated docs
  or types.
- Check changelogs or GitHub release notes for breaking changes between the
  version in the tutorial and the pinned project version.

### Step 5: Answer with Verifiable Citations

When presenting documentation findings to the user or incorporating them into
code:

- State the confirmed library version.
- Cite the authoritative source (e.g., file path in `node_modules`, official
  docs URL, or docs.rs API reference).
- Provide the verified signature or configuration block without synthetic
  embellishment.

---

## Anti-Patterns and Guardrails

| Anti-Pattern                                         | Why It Fails                                                                                                           | Required Behavior                                                       |
| ---------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------- |
| **Relying on Memory** ("I remember method X exists") | Training cutoffs and hallucination produce subtle bugs or non-existent APIs.                                           | Search or inspect code before writing.                                  |
| **Trusting Context7 Opinions**                       | Context7 is model-mediated and may output speculative advice or stylistic opinions.                                    | Strip advice. Keep only concrete symbols, types, and schema keys.       |
| **The Stale Tutorial Trap**                          | Maintainers frequently update code but forget to update narrative guides and blog posts.                               | Check doc version tags; verify against auto-generated API docs or code. |
| **Overruling Code with Docs**                        | If a guide says an option exists, but the installed `.d.ts` shows it does not, trusting the doc causes build failures. | Code is ground truth. Follow the code/types.                            |
| **Unversioned Search**                               | Searching for "Next.js routing" without checking v12 vs v14/15 yields obsolete patterns.                               | Always include the pinned version in searches and URL inspections.      |

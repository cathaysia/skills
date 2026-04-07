# Shared Quality Bootstrap

Use this reference for repository-wide quality files that apply regardless of
language stack.

## `.editorconfig`

If the target repository does not already have `.editorconfig`, create it from:

- `/Users/loongtao/xidl/.editorconfig`

Rules:

- Copy the canonical local template content.
- Do not overwrite an existing `.editorconfig` unless the user explicitly asks
  for replacement.

## `.pre-commit-config.yaml`

If the target repository does not already have `.pre-commit-config.yaml`, create
it from:

- `/Users/loongtao/xidl/.pre-commit-config.yaml`

Rules:

- Copy the canonical local template content.
- Do not overwrite an existing `.pre-commit-config.yaml` unless the user
  explicitly asks for replacement.

## Inspection order

Before creating either file:

1. Check whether the target repository already defines the file.
2. If it does not exist, copy the canonical template.
3. If it already exists, preserve it and mention that the skill treats these as
   create-if-missing defaults.

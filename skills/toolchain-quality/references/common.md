# Shared Quality Bootstrap

Use this reference for repository-wide quality files that apply regardless of
language stack.

## `.editorconfig`

If the target repository does not already have `.editorconfig`, create it from
the workspace's canonical shared template.

Rules:

- Copy the canonical local template content.
- Do not overwrite an existing `.editorconfig` unless the user explicitly asks
  for replacement.

### Examples

```ini
root = true

[*]
charset = utf-8
end_of_line = lf
insert_final_newline = true
trim_trailing_whitespace = true
max_line_length = 80

[*.{json,toml,yml,gyp, scm}]
indent_style = space
indent_size = 2

[*.js]
indent_style = space
indent_size = 2

[*.rs]
indent_style = space
indent_size = 4

[*.{c,cc,h}]
indent_style = space
indent_size = 4

[*.{py,pyi}]
indent_style = space
indent_size = 4

[*.swift]
indent_style = space
indent_size = 4

[*.go]
indent_style = tab
indent_size = 4

[Makefile]
indent_style = tab
indent_size = 8
```

## `.pre-commit-config.yaml`

If the target repository does not already have `.pre-commit-config.yaml`,
create it from the workspace's canonical shared template.

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

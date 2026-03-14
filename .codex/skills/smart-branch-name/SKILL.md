---
name: smart-branch-name
description: Create intelligent git branch names for the current repository. Use when the user wants a branch name or branch naming help based on the requested change, with Gitflow-style prefixes and conditional YouTrack issue prompting when local branch names or git history suggest issue-linked conventions.
---

# Smart Branch Name

Create a branch name in three stages:

1. Classify the change type.
2. Build a short kebab-case slug.
3. Decide whether to append a YouTrack issue key.

## Classify the change type

Choose the prefix that best matches the request:

- `feature/`: new feature, enhancement, or user-visible capability
- `bugfix/`: defect fix, regression fix, or behavior correction
- `hotfix/`: urgent production fix, release blocker, or critical incident fix
- `feat/`: use only when the repository inspection output reports `feature_prefix=feat`

If the request is ambiguous, ask a short follow-up before finalizing the name.

## Build the slug

- Extract the core change intent from the user request.
- Convert it to lowercase kebab-case.
- Drop filler words such as `the`, `a`, `an`, `for`, `to`, `with`, `please`, `help`, `support`.
- Keep the slug short enough to scan quickly.

Examples:

- `add user authentication flow` -> `user-authentication-flow`
- `fix flaky payment retry logic` -> `flaky-payment-retry-logic`

For more examples, read [references/examples.md](references/examples.md).

## Detect issue-linked naming

Run `bash scripts/detect_youtrack_usage.sh` from the repository root.

Interpret the result like this:

- If `detected=true`, ask the user for the related YouTrack issue key.
- If `detected=false`, do not ask for an issue key and return a branch name without one.
- If `feature_prefix=feat`, prefer `feat/` for feature work. Otherwise use `feature/`.
- If the script cannot inspect the repository cleanly, explain the limitation and ask the user whether issue-linked naming is expected.

Ask with a short plain-text question such as:

`Which YouTrack issue should this branch link to? Reply with a key like ABC-123 or none.`

If the user answers `none`, `no`, `无`, or equivalent, omit the issue key entirely.

## Compose the final branch name

Use this format:

- Without issue key: `<prefix><slug>`
- With issue key: `<prefix><ISSUE-123>-<slug>`

Examples:

- `feature/user-authentication-flow`
- `bugfix/fix-login-timeout`
- `hotfix/OPS-431-payment-timeout`

## Output

- Return only the final suggested branch name unless the user asked for explanation.
- If you had to ask a clarifying question, wait for the answer before returning the final name.

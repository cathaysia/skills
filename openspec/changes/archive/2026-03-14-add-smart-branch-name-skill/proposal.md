## Why

Creating branch names is repetitive and inconsistent when the project expects both Gitflow-style prefixes and optional YouTrack issue linkage. A dedicated skill would make branch creation faster, reduce naming drift, and preserve existing repository conventions when issue keys are in active use.

## What Changes

- Add a new skill that derives a branch name from the requested change and chooses an appropriate prefix such as `feature/`, `feat/`, `bugfix/`, or `hotfix/`.
- Detect whether the current repository appears to use YouTrack issue keys by inspecting branch names and git history for keys matching `[A-Z]+-\\d+`.
- Require the skill to ask the user for the issue key only when repository signals suggest YouTrack integration is expected.
- Omit the issue suffix when the user explicitly says no issue should be linked.
- Define the skill workflow, decision rules, and output format so the behavior is reusable and deterministic.

## Capabilities

### New Capabilities
- `smart-branch-naming-skill`: Define a skill that generates repository-aware branch names with Gitflow-style prefixes and optional YouTrack issue association.

### Modified Capabilities
- None.

## Impact

- Adds a new skill under the local Codex skills workspace.
- May add supporting references or helper scripts if lightweight repository inspection is needed.
- Introduces a documented workflow for branch naming decisions and user prompting behavior.

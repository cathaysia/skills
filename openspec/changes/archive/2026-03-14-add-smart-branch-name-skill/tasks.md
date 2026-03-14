## 1. Define the skill package

- [x] 1.1 Create the new skill directory and add `SKILL.md` metadata that clearly advertises smart branch-name generation and conditional YouTrack prompting.
- [x] 1.2 Write the skill workflow for classifying change type, generating a kebab-case slug, and composing the final branch name format.
- [x] 1.3 Add any minimal supporting references or examples needed to keep the main skill instructions concise.

## 2. Encode repository-aware issue detection

- [x] 2.1 Implement the repository inspection steps the skill should use to look for `[A-Z]+-\\d+` patterns in local branches or git history.
- [x] 2.2 Define the decision rule for when the skill must ask for a YouTrack issue key versus when it should proceed without prompting.
- [x] 2.3 Document the opt-out behavior so a user answer of “none” produces a branch name without an issue segment.

## 3. Validate the skill behavior

- [x] 3.1 Review the skill against representative examples for feature, bugfix, hotfix, and ambiguous change requests.
- [x] 3.2 Verify the skill instructions produce consistent output when issue-linked naming is detected and when it is not detected.
- [x] 3.3 Confirm the final skill layout and metadata are ready for use in the local Codex skills workspace.

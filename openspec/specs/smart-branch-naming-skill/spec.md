# smart-branch-naming-skill Specification

## Purpose
Define how the smart branch naming skill generates repository-aware branch names with Gitflow-style prefixes and optional YouTrack issue linkage.
## Requirements
### Requirement: Branch names use an intent-appropriate prefix
The skill SHALL analyze the requested change and generate a branch name that starts with a Gitflow-style prefix appropriate for the work, including prefixes such as `feature/`, `feat/`, `bugfix/`, or `hotfix/`.

#### Scenario: Feature work gets a feature prefix
- **WHEN** the user describes building a new capability
- **THEN** the skill returns a branch name whose prefix is a feature-oriented prefix

#### Scenario: Fix work gets a fix prefix
- **WHEN** the user describes correcting a defect or regression
- **THEN** the skill returns a branch name whose prefix is a fix-oriented prefix such as `bugfix/` or `hotfix/`

#### Scenario: Ambiguous work requires clarification
- **WHEN** the requested change cannot be confidently classified into a known branch type
- **THEN** the skill MUST ask the user to clarify the intended change type before finalizing the branch name

### Requirement: Branch names include a normalized change slug
The skill SHALL derive a concise branch slug from the requested change by normalizing it into lowercase kebab-case and removing irrelevant filler words.

#### Scenario: Freeform request becomes a kebab-case slug
- **WHEN** the user provides a natural-language description of the work
- **THEN** the skill returns a slugged branch segment in lowercase kebab-case that reflects the requested change

### Requirement: Repository conventions determine whether issue linkage is prompted
The skill SHALL inspect repository context for YouTrack-style issue usage before deciding whether to ask for an issue key.

#### Scenario: Repository signals issue-linked naming
- **WHEN** existing branch names or git history contain identifiers matching `[A-Z]+-\\d+`
- **THEN** the skill MUST ask the user whether a related YouTrack issue should be linked

#### Scenario: Repository does not signal issue-linked naming
- **WHEN** repository inspection finds no identifiers matching `[A-Z]+-\\d+`
- **THEN** the skill MUST generate the branch name without prompting for a YouTrack issue key

### Requirement: Users can opt out of issue association
When issue-linked naming is indicated by repository context, the skill SHALL allow the user to decline adding an issue key and omit the issue segment from the final branch name.

#### Scenario: User declines issue linkage
- **WHEN** the skill asks for a related YouTrack issue and the user answers that there is none
- **THEN** the final branch name MUST omit any issue-key suffix or segment

#### Scenario: User provides an issue key
- **WHEN** the skill asks for a related YouTrack issue and the user provides a valid-looking issue key
- **THEN** the final branch name MUST include that issue key using the skill’s documented format

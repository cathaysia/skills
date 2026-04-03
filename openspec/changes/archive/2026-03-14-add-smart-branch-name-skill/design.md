## Context

This change adds a new Codex skill that helps users generate consistent branch
names for the current repository. The skill needs to balance two sources of
convention: the requested change type, which determines a Gitflow-style prefix,
and repository-specific evidence that issue keys such as `ABC-123` are part of
the expected branch format. The project currently has no dedicated branch-naming
skill, so the behavior, prompts, and inspection heuristics must all be defined
from scratch.

## Goals / Non-Goals

**Goals:**

- Provide a deterministic workflow for converting a user request into a branch
  name.
- Map common change intents to a small set of branch prefixes such as
  `feature/`, `feat/`, `bugfix/`, and `hotfix/`.
- Inspect repository context for YouTrack-style issue usage by checking existing
  branch names and git history for patterns matching `[A-Z]+-\\d+`.
- Ask the user for an issue key only when the repository appears to use
  issue-linked naming.
- Allow the user to opt out of attaching an issue key.

**Non-Goals:**

- Automatically creating the branch as part of the initial skill behavior.
- Integrating with the YouTrack API or validating that an issue key exists
  remotely.
- Supporting every possible branch taxonomy; the skill only needs clear default
  mappings and an escape hatch for ambiguous cases.

## Decisions

The skill will be implemented as a concise workflow-driven skill rather than a
standalone script-first utility. Rationale: the main task is decision-making and
user prompting, which fits a skill’s instruction model well. If repository
inspection commands become repetitive, a helper script can be added later
without changing the interaction contract. Alternative considered: building the
skill around a shell script immediately. Rejected because it would
over-constrain the first version before the branch format rules are settled.

Repository convention detection will use local git evidence only. Rationale:
checking current branches and recent commit history is cheap, reliable in this
workspace, and sufficient to infer whether issue keys are part of normal
practice. Alternative considered: asking the user every time whether YouTrack is
used. Rejected because it adds avoidable friction when the repository already
signals its convention.

The skill will separate three decisions: classify change type, derive a slug,
then optionally append an issue key segment. Rationale: this keeps the
branch-name construction explainable and makes the final naming rule easier to
document and test. Alternative considered: a single freeform naming prompt.
Rejected because it would make the output inconsistent and harder to reuse.

When the repository suggests issue integration, the skill will explicitly ask
for the related issue key and accept a “none” answer. Rationale: the user
request requires asking only when issue integration appears relevant, and it
also requires omitting the suffix when the user declines to link an issue.
Alternative considered: silently omitting the issue key if one was not provided.
Rejected because it could produce names that violate team conventions.

## Risks / Trade-offs

- [Repository heuristics miss conventions because local history is sparse] →
  Mitigation: inspect both branches and git history, and document that ambiguous
  cases should fall back to a user confirmation.
- [Prefix classification is subjective for some requests] → Mitigation: define
  default mappings and require the skill to ask when the change type cannot be
  classified confidently.
- [Teams may want a different issue-key placement] → Mitigation: document the
  chosen default format clearly so a follow-up change can extend it without
  changing the detection logic.
- [A pure instruction-based skill may drift over time] → Mitigation: keep the
  workflow explicit and add supporting references or scripts only if real usage
  shows ambiguity.

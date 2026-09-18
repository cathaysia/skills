---
name: research-report-creator
description: >-
  Conduct rigorous, objective research on technologies, architectures, or products,
  and synthesize structured research reports. Use when initiating research, surveying
  tools, investigating competitors, or writing research reports. Coordinates a two-stage
  process: Stage 1 (Target Definition for Main Agent) and Stage 2 (Independent Subagent Execution).
---

# Research Report Creator

A two-stage framework for conducting objective, evidence-based research and authoring authoritative technical reports.

The workflow is strictly split into two operational stages across two specialized guides:
- **Stage 1 (Main Agent)**: Determining the research purpose, technical constraints, and candidate targets.
- **Stage 2 (Subagents)**: Executing isolated, evidence-grounded investigations into each target.

---

## Two-Stage Architecture Overview

```
┌────────────────────────────────────────────────────────────────────────┐
│                        STAGE 1: MAIN AGENT                             │
│  Read: references/target-definition.md                                 │
│  - Define factual purpose, constraints, and scope                      │
│  - Select Direct Solutions & Conceptual/Philosophical Analogues        │
└──────────────────────────────────┬─────────────────────────────────────┘
                                   │ Spawns 1 Subagent per Target
                                   ▼
┌────────────────────────────────────────────────────────────────────────┐
│                        STAGE 2: SUBAGENTS                              │
│  Read: references/subagent-execution.md                                │
│  - Subagent 1 ──► Target 1 ──► research_notes_target1.md               │
│  - Subagent 2 ──► Target 2 ──► research_notes_target2.md               │
│  - Subagent N ──► Target N ──► research_notes_targetN.md               │
│  (Zero cross-editing, zero cross-referencing, strict source hierarchy) │
└──────────────────────────────────┬─────────────────────────────────────┘
                                   │ Collects independent notes
                                   ▼
┌────────────────────────────────────────────────────────────────────────┐
│                   FINAL SYNTHESIS: MAIN AGENT                          │
│  Compile 4-Part Report: Purpose ➔ Targets ➔ Findings ➔ Recommendations │
│  (Findings: separate chapters, no purpose mentioned, no comparisons)   │
└────────────────────────────────────────────────────────────────────────┘
```

---

## Orchestration Workflow

### Step 1: Execute Stage 1 (Main Agent)
1. The Main Agent **MUST** read [references/target-definition.md](file:///Users/loongtao/skills/skills/research-report-creator/references/target-definition.md).
2. Clarify the factual research purpose, requirements, and constraints (no subjective speculation).
3. Select the research targets:
   - **Direct Solutions**: Industry-standard, identical, or competitor solutions.
   - **Conceptual Analogues**: Cross-domain products sharing underlying design philosophies or architectures.
4. Record official names, homepages, repository URLs, and selection rationales.

### Step 2: Dispatch Subagents for Stage 2 (Subagent Execution)
1. For each identified target, spawn a dedicated subagent (e.g., using `invoke_subagent`).
2. Pass [references/subagent-execution.md](file:///Users/loongtao/skills/skills/research-report-creator/references/subagent-execution.md) as the operating instructions for each subagent.
3. Use the following invocation template:

```markdown
Role: Technical Researcher for [Target Name]
Prompt:
You are assigned to investigate [Target Name].
Follow the rules in `references/subagent-execution.md` strictly:
1. Output your findings exclusively to an independent file: `research_notes_[target_name].md`.
2. Do NOT open or edit other subagents' files; do NOT compare [Target Name] to other products.
3. Do NOT mention our project's overarching research purpose.
4. Ground every statement in facts using the Source Credibility Hierarchy (Code > Official Docs/Blogs > Third-Party).
5. Cover all three dimensions: (1) Achieved Outcome & Effect, (2) Underlying Mechanism, and (3) Costs & Trade-offs (Optional).
```

### Step 3: Assemble Final Report (Main Agent)
Once all subagents finish and their independent notes files are ready, the Main Agent compiles the final research report adhering to the strict four-part sequence:

```
1. Research Purpose (Strictly factual; no subjective editorializing)
2. Research Targets (Factual specifications and classifications)
3. Research Findings (Directly assembled from subagent notes)
   - Distinct, unmixed chapter per target
   - Strictly NO mention of the research purpose
   - Strictly NO cross-target comparisons
   - Full citation links for all facts and data; verbatim quotes must link to source URLs
4. Recommendations (Actionable guidance and tradeoffs logically derived from Findings)
```

---

## Standard Final Report Template

```markdown
# Research Report: [Subject / Topic Name]

## 1. Research Purpose
- **Background**: [Factual description of existing architecture or scenario]
- **Requirements & Constraints**: [Throughput, latency, licensing, or runtime boundaries]
- **Scope**: [Exact boundaries of what this report covers and excludes]

## 2. Research Targets
### 2.1 Direct Solutions
- **[Target 1 Name]** ([URL](https://...)): [Vendor, license, architecture class, selection rationale]
- **[Target 2 Name]** ([URL](https://...)): [Vendor, license, architecture class, selection rationale]

### 2.2 Conceptual & Philosophical Analogues
- **[Target 3 Name]** ([URL](https://...)): [Cross-domain product sharing relevant philosophy or pattern]

## 3. Research Findings

### 3.1 [Target 1 Name]
#### 3.1.1 Achieved Outcome & Effect
[Factual capabilities, metrics, and outcomes with source citations]

| Characteristic / Metric | Specification | Source |
| :--- | :--- | :--- |
| Throughput / Latency | [Value] | [Benchmark / Doc Link](https://...) |

#### 3.1.2 Underlying Mechanism & Implementation
[Factual architecture, algorithms, and workflows with source citations]
- Architecture Topology: [Details] ([Source](https://...))
- Core Mechanism: [Details] ([Source](https://...))

#### 3.1.3 Costs & Trade-offs (Optional)
- Cost / Overhead: [Details] ([Source](https://...))
- Verbatim Excerpt: > "[Text]" — [Source Link](https://...)

---

### 3.2 [Target 2 Name]
...

---

### 3.3 [Target 3 Name (Analogue)]
...

## 4. Recommendations
- **Architecture Strategy**: [Actionable proposal rooted in factual findings]
- **Target Suitability**: [Clear rationale for which target matches which requirement]
- **Next Steps & Prototyping**: [Concrete verification steps]
```

---

## Quality Checklist Before Finalizing

- [ ] Did the Main Agent follow `references/target-definition.md` to define factual purpose and targets?
- [ ] Were candidate targets divided into direct solutions and conceptual analogues?
- [ ] Was each target investigated independently by a dedicated subagent guided by `references/subagent-execution.md`?
- [ ] Did each subagent produce its own isolated `research_notes_<target>.md` without cross-editing or cross-referencing?
- [ ] Does the final report follow the exact order: Purpose ➔ Targets ➔ Findings ➔ Recommendations?
- [ ] Are Purpose and Targets free from subjective speculation?
- [ ] Does every target have its own dedicated, unmixed chapter in Research Findings?
- [ ] Are Research Findings completely free of mentions of the research purpose?
- [ ] Are Research Findings completely free of cross-target comparisons?
- [ ] Does each target's findings cover outcome, mechanism, and costs/trade-offs (optional)?
- [ ] Are factual claims substantiated by primary, authoritative sources (Code > Official Docs/Blogs)?
- [ ] Have third-party reviews and commentaries been treated with caution and verified against primary sources?
- [ ] Are all verbatim quotations accompanied by their original source URLs?
- [ ] Are all recommendations directly traceable to the verified findings?

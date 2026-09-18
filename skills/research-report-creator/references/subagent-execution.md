# Stage 2: Independent Target Investigation

> **Target Audience**: Dedicated Subagent (Technical Researcher)  
> **Objective**: Execute an exhaustive, objective technical investigation into exactly ONE assigned target, strictly adhere to source credibility standards, and produce an independent research document.

---

## 1. Subagent Operating Rules & Isolation

Every subagent tasked with investigating a target must strictly obey the following isolation constraints:

1. **One Target Exclusively**:
   - Investigate only the single target assigned to you.
2. **Dedicated Independent Output**:
   - Write all notes and findings exclusively to an independent document named `research_notes_<target_name>.md`.
3. **Strict Prohibition on Cross-Editing**:
   - Never open, write to, or modify any other subagent's working file or output document.
4. **Strict Prohibition on Cross-Referencing & Comparison**:
   - Do **NOT** read or reference other subagents' notes.
   - Do **NOT** compare your assigned target against any other product, tool, or framework (no *"Target A is faster than Target B"*).
   - Evaluate your assigned target entirely on its own individual merits.
5. **Negative Constraint — No Research Purpose**:
   - Do **NOT** mention, reference, or restate the project's overarching research purpose in your document. Present the target's architecture and capabilities autonomously.

---

## 2. Source Credibility Hierarchy

Evaluate the accuracy and reliability of all evidence according to this strict hierarchy:

### Tier 1 (Highest Authority / Ground Truth — Source Code)
- **Artifacts**: Source code repositories, unit/integration test suites, formal schemas (Protobuf, OpenAPI, JSON Schema), commit histories, and configuration files.
- **Rule**: Code is the ultimate ground truth and cannot posture. When documentation or marketing claims contradict the code implementation, the code is always authoritative.

### Tier 2 (Authoritative First-Party — Official Docs & Staff Statements)
- **Artifacts**: Official documentation, architecture whitepapers, official release notes, company engineering blogs, and public talks or writings by verified company engineers or core maintainers.
- **Rule**: Authoritative for design intent, recommended configurations, and documented limits.

### Tier 3 (Derivative / Low Authority — Third-Party Commentary)
- **Artifacts**: Third-party review blogs, tech influencer videos, social media commentary, and forum discussions (Reddit, Hacker News, Stack Overflow).
- **Rule**: **Exercise extreme caution**. Third-party commentary is frequently subjective, speculative, or outdated. **Never** cite technical claims or performance numbers from Tier 3 without cross-verifying them against Tier 1 (Code) or Tier 2 (Official sources).

---

## 3. Mandatory Investigation Dimensions

Your research notes must address the following three structured dimensions:

### Dimension 1: Achieved Outcome & Effect
- What observable results, capabilities, and performance characteristics does the target actually deliver?
- Document verified throughput, latency numbers, scalability limits, fault-recovery behaviors, and real-world performance.
- Use tables or diagrams where applicable. Every single metric must cite an authoritative source (Tier 1 or Tier 2).

### Dimension 2: Underlying Mechanism & Implementation
- How does the target achieve this outcome?
- Detail the core architecture, data structures, consensus/replication algorithms, execution models, and scheduling pipelines.
- Trace the actual implementation flow (e.g., append-only log, LSM-tree compaction, zero-copy network paths, goroutine worker pools).
- Cite source code files, architecture whitepapers, or official specifications.

### Dimension 3: Costs & Trade-offs (Optional)
- What price, compromises, or operational overhead did the target pay to reach this objective?
- Document concrete costs:
  - Resource consumption (high memory footprint, disk write amplification).
  - Operational complexity (zookeeper/etcd dependencies, multi-node operational burdens).
  - Functional trade-offs (eventual consistency instead of strong consistency, restricted query capabilities).
- Cite documented constraints or code-level limitations.

---

## 4. Citation & Quotation Standards

- **Ground Every Sentence**: Every factual statement, architectural claim, and specification must be accompanied by a markdown citation link (e.g., `[Official Docs](https://...)` or `[GitHub Source](https://...)`).
- **Verbatim Quotations**: If quoting text from documentation or technical posts, enclose it in quotation marks and attach the direct source URL immediately.
- **Data & Tables**: All data presented in tables or charts must have an explicit "Source" column linking to the benchmark or primary document.

---

## 5. Output Document Template (`research_notes_<target_name>.md`)

Subagents must format their independent document using the following template:

```markdown
# Research Notes: [Target Name]

- **Official Homepage**: [URL](https://...)
- **Source Repository**: [URL](https://...)
- **License**: [License Type]

## 1. Achieved Outcome & Effect
[Factual presentation of capabilities, performance, and operational outcomes]

| Characteristic / Metric | Verified Value | Source (Tier 1 / Tier 2) |
| :--- | :--- | :--- |
| Throughput / Latency | [Value] | [Benchmark / Doc URL](https://...) |
| High-Availability Model | [Model] | [Architecture Guide URL](https://...) |

## 2. Underlying Mechanism & Implementation
[Detailed technical explanation of architecture, data structures, and algorithms]
- Architecture Topology: [Description] ([Source](https://...))
- Core Mechanism: [Explanation of how results are achieved] ([Source](https://...))

## 3. Costs & Trade-offs (Optional)
[Documented costs, resource overheads, and design compromises]
- Cost / Overhead: [Description] ([Source](https://...))
- Verbatim Excerpt: > "[Exact text]" — [Source Link](https://...)
```

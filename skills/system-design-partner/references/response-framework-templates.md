# Response Framework Templates & Dynamic Adaptation

This document defines core response templates for architectural consulting and
idea exploration, alongside rules for adapting to formatting constraints and
real-time user corrections. Templates are domain-agnostic and seamlessly handle
database modeling, microservice decoupling, domain entities, API contracts, and
state workflows.

---

## 1. Five-Step Architectural Reasoning Framework

When the user introduces a new business requirement, system proposal, or
hypothesis, follow this five-stage progression:

```
┌──────────────────────────────────────────────────────────────┐
│ 1. Verdict & Myth Busting                                    │
│    - Decisive opening stance (Strongly recommend / Avoid)    │
├──────────────────────────────────────────────────────────────┤
│ 2. Disambiguation & Metaphor                                 │
│    - ASCII diagrams or metaphors clarifying core roles       │
├──────────────────────────────────────────────────────────────┤
│ 3. Vulnerability Analysis & Benchmarking                     │
│    - Trace failure modes (ownership vacuum, key rotations)   │
├──────────────────────────────────────────────────────────────┤
│ 4. Minimal Blueprint & Unified Modeling                      │
│    - Single-kernel/consolidated abstractions, SQL/pseudocode │
├──────────────────────────────────────────────────────────────┤
│ 5. Phased Evolution & Forward Escapes                        │
│    - Define Phase 1 (MVP) vs Phase 2/3; zero-cost escapes    │
└──────────────────────────────────────────────────────────────┘
```

---

## 2. High-Frequency Structural Templates

### Template 1: Domain Responsibility Inventory (No Implementation Details)

> **Trigger Condition**: User explicitly specifies: "Which
> [tables/entities/components/APIs] do I need, and what does each do? Do not
> explain schemas or implementation code"; or corrects a prior response back to
> this format.\
> **Core Principle**: Strictly prohibit dumping field types, code blocks, or
> internal configurations. Group items by domain; define each item with a
> single, highly condensed sentence explaining authority and boundaries.

```markdown
### 1. [Domain / Cluster Name, e.g., Identity & Tenant Domain / Control Plane]

- **[Entity/Component Name] (`[identifier/table_name/service_name]`)**

  **Role:** [One-sentence explanation of core responsibilities. If managed
  externally, specify authoritative origin and ID-only pass-through
  conventions].

- **[Entity/Component Name] (`[identifier/table_name/service_name]`)**

  **Role:** [One-sentence explanation of relationships, lifecycle states, or
  isolation boundaries].

---

### 2. [Domain / Cluster Name, e.g., Core Asset Domain / Data Plane]

- **[Entity/Component Name] (`[identifier/table_name/service_name]`)**

  **Role:** [One-sentence explanation of business payload and state management].

---

### 3. [Domain / Cluster Name, e.g., Policy & Scheduling Domain / Policy Engine]

- **[Entity/Component Name] (`[identifier/table_name/service_name]`)**

  **Role:** [Globally consolidated decision/scheduling model].

#### Detailed Rules & Key Guidelines:

1. **Subject Abstraction (Who)**: Supports multiple caller types; downstream
   services match opaque ID strings only.
2. **Resource Granularity (What)**: Covers individual resources,
   containers/groups, and dynamic tags.
3. **Role & Action Tiers**: Clearly defined levels (Viewer, Operator, Admin)
   with top-priority explicit deny (`DENY`).
```

---

### Template 2: Unified Policy Model & Evaluation Engine

> **Trigger Condition**: Use when designing resource sharing, permissions, state
> machine transitions, or scheduling arbitration.

````markdown
### 1. Core Design: Unified Policy Model

Design Principle: Reject fragmented parallel tables for different scenarios;
unify under "Subject (Who) + Role/Action (What) + Target (Resource)".

```sql
CREATE TABLE unified_policies (
    id            BIGINT PRIMARY KEY AUTO_INCREMENT,
    context_id    BIGINT NOT NULL,          -- Top-level isolation context (Tenant / Workspace / Cluster)
    subject_type  VARCHAR(20) NOT NULL,     -- 'USER' | 'GROUP' | 'ROLE'
    subject_id    VARCHAR(64) NOT NULL,     -- Opaque ID string pass-through
    resource_type VARCHAR(20) NOT NULL,     -- 'ITEM' | 'CONTAINER' | 'TAG'
    resource_id   VARCHAR(64) NOT NULL,     -- Opaque ID string pass-through
    effect        VARCHAR(20) NOT NULL,     -- 'ALLOW' | 'DENY'
    created_by    VARCHAR(64) NOT NULL      -- Delegation & audit tracking
);
```
````

### 2. Unified Evaluation Engine & Precedence Rules

1. **Single Evaluation Query**: One query matches direct caller grants,
   inherited group grants, and dynamic tag assignments.
2. **Evaluation Precedence**:
   - **`DENY` Takes Absolute Precedence** (Override / Blacklist veto).
   - **Direct Subject Grants Override Inherited Group Grants**.
   - **Specific Resource Grants Override Broad Tag/Container Grants**.
   - **Default Fail-Closed**.

````
---

### Template 3: Cross-System Collaboration & Boundary Matrix
> **Trigger Condition**: Use when discussing microservice decomposition, frontend-backend separation, third-party integrations, or Control/Data Plane boundaries.

```markdown
| Domain / Dimension | Core Need | External / Control Plane (System A) | Internal / Data Plane (System B) | Protocol & Source of Truth |
| :--- | :--- | :--- | :--- | :--- |
| **Identity & Credentials** | Token issuance & verification | Issues signed JWT/tokens; maintains master state | Validates keys statelessly; operates solely on IDs | **System A Authoritative** |
| **Namespace Isolation** | Multi-tenant boundaries | Manages workspace lifecycle; injects context ID | Enforces context ID as hard query filter | **System A Authoritative** |
| **Core Business Execution** | Runtime execution & persistence | *Uninvolved (Blind)* | Owns execution runtime and data storage | **System B Authoritative** |
| **Policy Evaluation** | Fine-grained arbitration | Provides group membership associations | Queries unified policy engine (`DENY` priority) | **System B Authoritative** |
````

---

## 3. Dynamic Adaptation & Structural Correction Rules

During conversations, users may impose format constraints or redirect output
structure. The AI must enforce these rules:

### 1. Negative Constraint Enforcement

- When the user specifies "no table schemas", "no implementation details", or
  "no code":
  - **Prohibited**: Emitting full DDLs, column definitions, or complex class
    implementations.
  - **Required**: Emitting high-density, conceptual inventories focusing
    strictly on definitions, boundaries, and lifecycles.

### 2. Real-Time Structural Correction

- **Trigger**: When the user requests:
  > "Explain this using the structure from '[previous prompt]'"\
  > "Do not use tables; use a list"\
  > "Just like the table we had earlier"
- **Execution**:
  - **Zero Friction**: Never justify or defend why a different format was used
    previously.
  - **Exact Alignment**: Immediately pull the user's referenced structure and
    format the new content to match its headers, bullet styles, and indentation.
  - **Inject Fresh Concepts**: Populate the requested structure with the newly
    discussed technical entities without leaking unwanted trivia.

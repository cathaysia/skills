# Stage 1: Research Purpose Formulation & Target Selection

> **Target Audience**: Main Agent (Orchestrator)  
> **Objective**: Clarify the factual research purpose, establish concrete constraints, and select qualified research targets before launching investigations.

---

## 1. Clarifying Research Purpose

Before initiating any research or dispatching subagents, formulate and record the exact purpose of the investigation.

### Rules:
- **Strictly Fact-Based**: State only concrete technical facts, business context, architectural limits, and constraints.
- **Zero Subjectivity**: Do not include subjective assumptions, pre-judged conclusions, or marketing rhetoric (e.g., write *"Evaluate systems capable of sustaining 100k writes/sec with sub-5ms p99 latency"* instead of *"Find out why System A is superior to System B"*).
- **Explicit Constraints**: Detail all known boundaries:
  - Throughput, latency, and resource limits (CPU, memory, storage IOPS).
  - Deployment environment (Kubernetes, bare metal, edge, serverless).
  - Software licenses permitted (e.g., Apache 2.0, MIT vs. AGPL/SSPL).
  - Language and runtime requirements (e.g., Rust, Go, Java, C++).

---

## 2. Selecting Research Targets

Identify candidate targets that directly or conceptually address the research purpose. Candidate selection must incorporate two distinct categories:

### Category A: Direct / Identical Solutions
Industry-standard, competitive, or functionally identical solutions addressing the exact same problem domain.
- *Examples*:
  - Researching distributed message queues: Apache Pulsar, Apache Kafka, RabbitMQ.
  - Researching vector databases: Qdrant, Milvus, Chroma.

### Category B: Conceptual & Philosophical Analogues
Products, engines, or systems from completely different domains that share similar core philosophies, architectural patterns, data structures, or execution semantics.
- *Examples*:
  - Designing a document versioning engine: Study **Git** (immutable content-addressable DAG storage).
  - Designing a distributed append-only audit log: Study **Apache BookKeeper** or **LevelDB WAL** (write-ahead log architecture).
  - Designing an event-driven actor framework: Study **Erlang/OTP** or **Akka**.

---

## 3. Target Specification Record

For each selected target, the Main Agent must record:
1. **Official Name**: Exact project or product name.
2. **Official URLs**: Project homepage and source code repository (e.g., GitHub, GitLab).
3. **Classification**: Direct Solution or Conceptual Analogue.
4. **License**: Open-source license or proprietary terms.
5. **Selection Rationale**: Factual technical reason why this target is being evaluated (e.g., specific storage engine, consensus protocol, or indexing algorithm).

---

## 4. Stage 1 Completion Checklist

Before proceeding to Stage 2 and dispatching subagents, verify:
- [ ] Is the research purpose defined solely through objective technical requirements and constraints?
- [ ] Are all subjective assumptions and pre-determined preferences eliminated?
- [ ] Does the target list include both direct solutions and relevant conceptual analogues?
- [ ] Are official repositories and homepages identified for all targets?
- [ ] Is each target assigned to be investigated by exactly one dedicated subagent?

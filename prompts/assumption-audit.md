# Assumption Audit

Status: Draft

Use this prompt to pressure-test a plan, proposal, or design decision before committing to implementation. The goal is to surface hidden assumptions, missing constraints, unstated dependencies, and second-order effects through structured questioning.

## When to Use

- Before committing to an implementation plan that touches protocol behavior, transport, IPC, or security
- Before a steelman challenge (use this to strengthen the plan first)
- When exploring a solution space with multiple viable paths
- When a plan feels right but has not been pressure-tested
- When stakes are high enough that hidden assumptions matter (consensus-critical, identity, peer pinning)

## When NOT to Use

- During active implementation — use steelman challenge instead
- For pure information gathering
- When the plan is already finalized and approved
- When the user wants validation, not interrogation

## Inputs

- A plan, proposal, or design decision to interrogate
- Any context about the decision space
- Relevant spec sections (`spec/SPEC.md`, `spec/IPC.md`, `spec/WIRE_FORMAT.md`, `spec/MESSAGE_TYPES.md`)

## Procedure

### Step 1: Surface assumptions as questions

Read the plan and identify every assumption it makes — stated or unstated. Turn each into a concrete question that tests whether the assumption holds. Present as a numbered list grouped by impact area.

Priority order for AXON:
1. **Protocol correctness** — Does this preserve wire-format compatibility? Does it violate any spec invariant?
2. **Security** — Does this weaken peer pinning, identity verification, or TLS guarantees?
3. **Operational** — What happens if the daemon restarts mid-change? Is state left consistent?
4. **Interoperability** — Would a non-Rust implementation still interoperate after this change?
5. **Performance** — Does this affect hot paths (message dispatch, QUIC streams, IPC broadcast)?

Order questions by impact — the ones where being wrong would hurt the most come first.

### Step 2: Grill one question at a time

For each question, dig deeper:
- **What would need to be true** for the current plan to be correct here?
- **What happens at the boundaries** — edge cases, scale limits, unusual inputs?
- **What does failure look like** — and how would you notice?
- **What does this force** — if this assumption holds, what other options does it close?
- **Was this the only option**, or the first one that seemed reasonable?

If one question's answer reveals coupling with other questions, call that out explicitly.

### Step 3: Cross-reference with AXON invariants

Verify the plan against load-bearing invariants:
- Agent ID = SHA-256(pubkey) — does the plan preserve this?
- Peer pinning — does the plan maintain transport-layer rejection of unknown peers?
- Address uniqueness — does the plan preserve at-most-one-non-static-peer-per-address?
- PeerTable owns the pubkey map — does the plan respect this ownership boundary?

### Step 4: Summarize and confirm

After grilling a question, summarize what was resolved and what remains open. Get explicit confirmation before moving to the next question.

### Step 5: Stop conditions

Stop when:
- The user signals alignment on all questions
- All questions have been resolved or explicitly deferred with stated risk
- The user says to stop

## Guardrails

- This is interrogation, not criticism. The goal is to make the plan stronger.
- Do not invent objections to increase question count. Every question should target a real gap.
- Do not drift into solving. Surface the problem; let the user decide the fix.
- Do not assume domain expertise. Ask about the domain when needed rather than guessing.
- Keep rounds focused. One question at a time, confirm, move on.

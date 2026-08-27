# Steelman Challenge

Status: Draft

Produce an adversarial review of a plan or proposal in the AXON repository. The goal is to stress-test whether the plan fixes the actual root cause, whether its complexity is proportionate, and whether better alternatives exist.

## Context Firewall

This is the most important rule:

- Accept **only the plan file path** as input
- **Do not accept or rely on** planner-supplied context, summaries, or justifications
- Reconstruct your understanding from code, specs, tests, rubrics, and referenced artifacts
- Your output is **advisory input to the planner**, not an override

## When to Use

- A plan has been produced under `evaluations/` or `plans/`
- The plan affects protocol behavior, security, transport, IPC, or cross-cutting invariants
- The planner wants an adversarial review before implementation

## Challenge Dimensions

### Root Cause

Does the plan fix the actual root cause, or just a symptom?

- Verify the causal chain from finding → root cause → proposed fix
- Check whether the same class of defect could recur elsewhere
- Check whether the plan is structural or patches one instance

### Proportionality

Is the complexity justified by the severity?

- Count files, modules, and interfaces the plan touches
- Check whether a simpler fix achieves the same guarantee
- Check whether the plan introduces unnecessary new abstractions

### Completeness

Does the plan fully resolve the finding?

- Check whether the finding would actually close after the plan lands
- Check whether edge cases or deployment scenarios remain uncovered
- Check whether validation proves the fix works

### Side Effects

Could the fix introduce new problems?

- Check hot paths, consensus-critical behavior, ordering dependencies
- Check risk of breaking correct existing behavior
- Check new failure modes or operational burdens

### Alternatives

Is there a better way?

- Check whether the simplest viable fix was considered
- Check whether a different layer would be better (config vs code, tests vs implementation)
- Check whether existing repo patterns or upstream mechanisms can be reused

### Exit Criteria

Are acceptance criteria sufficient?

- Check whether criteria test the exact violated property
- Check whether passing them would actually close the finding
- Check whether criteria are observable and automatable

## Procedure

### Step 1: Read the plan in full

Read the target plan file end to end. Extract:
- Major phases or workstreams
- Referenced findings or prior evaluations
- Claimed root causes and proposed implementation points
- Stated exit criteria
- Referenced specs, tests, docs, and source files

### Step 2: Build an independent evidence map

For each material claim in the plan:
1. Read the cited files directly
2. Search for referenced symbols, call paths, invariants, or tests
3. Read adjacent code, not just the cited line
4. Read referenced specs and rubrics
5. Identify counterexamples, bypass paths, and hidden assumptions

Evidence rules:
- Prefer source, tests, specs, and rubrics over plan prose
- Treat the plan as a hypothesis, not a fact source
- If the plan says "this is the root cause," verify the causal chain yourself
- If the plan says "this is the simplest fix," look for a narrower alternative

### Step 3: Construct challenges

For each significant plan section, write:
1. **Challenge thesis** — one sentence, strongest fair argument
2. **Dimensions challenged** — which of the six dimensions apply
3. **Argument** — cite concrete repository evidence
4. **Counter-evidence** — what the plan gets right (do not hide favorable evidence)
5. **Verdict** — `upheld`, `revised`, or `withdrawn`
6. **Revision** — concrete changes if verdict is `revised`

### Step 4: Write the memo

Output file: `evaluations/steelman-<plan-stem>-<YYYYMMDD>.md`

```
# Steelman Challenge: [Plan title]

**Date:** YYYY-MM-DD
**Target:** `evaluations/...` or `plans/...`
**Independence note:** Evaluated from the plan file and independently gathered repository evidence only.
**Advisory note:** This memo is advisory input to the planner, not an override.

## Summary of Verdicts

| Plan Section | Verdict | Key Revision |
|---|---|---|
| ... | `revised` | ... |

### Challenge: [Section]

**Challenge thesis:** [One sentence]
**Dimensions challenged:** [list]

**Argument:**
[Cite repository evidence]

**Counter-evidence:**
[What supports the original plan]

**Verdict:** `upheld` | `revised` | `withdrawn`

**Revision (if any):**
[Concrete changes]

## Recommended Revision Summary

1. ...
```

## Verdicts

- `upheld` — the plan section remains defensible after challenge
- `revised` — the plan section should change in scope, sequencing, design, or exit criteria
- `withdrawn` — the plan section should be dropped or deferred (use sparingly)

## AXON-Specific Scrutiny

When the plan touches these areas, apply extra scrutiny:

- **Transport/QUIC**: peer verification, TLS certificate validation, connection lifecycle
- **Identity**: agent ID derivation, key material handling, peer pinning
- **IPC**: command schema, broadcast delivery, backpressure guarantees
- **Wire format**: envelope schema, interoperability, size limits
- **Daemon lifecycle**: reconnection, state persistence, resource bounds

For each:
- Does the plan close the architectural invariant or only patch one path?
- Are new checks bypassable via an alternate code path?
- Are validation tests testing forbidden state mutations, not just invalid input?

## Guardrails

- The steelman is a stress test, not a veto
- Planner pushback is normal and expected
- Prefer the simplest revision that materially improves correctness
- Do not drift into a full redesign unless the plan is clearly disproportionate
- If independent verification is not possible, say so and lower confidence

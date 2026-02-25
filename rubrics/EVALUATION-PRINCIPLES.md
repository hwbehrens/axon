# Shared Evaluation Principles

Status: Normative

All rubrics in this directory inherit these principles. Apply them once per review to avoid duplicate deductions.

## Core principles

- **Evidence, not intent.** Every deduction must cite a concrete, verifiable signal in the diff, code, spec, or documentation. Never penalize on vague intuition. Equally, never award points on good intentions; verify the actual artifact.
- **First-principles thinking.** Evaluate what the change *actually does*, not what the commit message claims. Read the code; read the spec; check that they agree. If they disagree, that is a finding.
- **100 means flawless.** A perfect score in any category means you examined every applicable check, found zero issues, and would stake your reputation on it. Do not round up. If in doubt, deduct; the author can rebut.
- **This is not a rubber stamp.** Assume the change has defects until proven otherwise. Actively look for spec drift, missing tests, broken invariants, naming violations, stale docs, unnecessary complexity, security regressions, and resource leaks.
- **Thorough, not cursory.** Read the actual files, not just the diff summary. Cross-reference spec text against implementation constants and behavior. Check test assertions against spec requirements. Verify documentation links resolve.
- **Deductions are cumulative and specific.** State the category, the issue, the evidence (file + line or spec section), and the point cost. One issue may cause deductions in multiple categories if it violates multiple rubric checks.
- **Proportional severity.** A silent protocol-behavior divergence or security regression warrants a larger deduction than a minor naming inconsistency. Use judgment, but always explain the reasoning.
- **Material impact only.** Deduct for issues that can materially affect safety, correctness, security, interoperability, reliability, or meaningful performance. Do not deduct for stylistic preferences or low-impact implementation taste.
- **No over-constraint.** Do not penalize alternative designs that preserve required invariants and externally observable behavior.
- **Substance over preference.** Focus on issues that concretely affect AXON's goals. A finding is substantive if ignoring it would degrade agent experience, violate a stated project principle, or introduce measurable complexity without justification. A finding is a nit if it reflects a reviewer preference that reasonable engineers would disagree on. Deduct for substantive issues; do not deduct for nits.

## Severity calibration

- **Critical:** can cause protocol-behavior divergence, key compromise, persistent outage, or severe DoS under realistic conditions.
- **Major:** can break interoperability, significantly degrade correctness/performance/reliability, or block progress under common fault scenarios.
- **Moderate:** bounded impact or requires narrower conditions but still affects core goals.
- **Minor:** low-impact quality gaps; usually non-blocking unless repeated.

## Scoring method

- Start each category at max points, then deduct.
- One issue may affect multiple rubrics, but avoid double-deducting the same exact failure mode inside a single rubric.
- If uncertainty remains due to missing evidence, deduct proportionally and record confidence.

## Required review output format

1. `Total score: X/100`
2. Category scores with deductions.
3. Findings with severity, deduction, evidence, and impacted property.
4. Residual risk summary.
5. Confidence level (`high`/`medium`/`low`) with reason.

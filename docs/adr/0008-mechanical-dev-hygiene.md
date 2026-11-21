# ADR-0008: Mechanical Dev Hygiene Over Discipline-Based Rules

**Status:** Accepted  
**Date:** 2026-04-19  
**Supersedes:** None  
**Decision makers:** Matt Carano, Brit Freed

## Context

On 2026-04-19, we spent one day converting a month of accumulated scar tissue into mechanical enforcement.

The trigger chain: investigating auto-compaction → discovering bootstrap truncation (files silently cut at 12K defaults) → realizing AGENTS.md rules had been missed for months because they were buried in 22K of text that got truncated before agents ever read them. Rules we thought were enforced — spec-first discipline, no-symptom-fixes, Git commit-before-ship — existed only as text that no agent reliably loaded.

Deeper inspection revealed the pattern was systemic:
- **AGENTS.md** (22,415 chars) contained process rules that duplicated what should live in a playbook
- **MEMORY.md** (25,134 chars) mixed identity, projects, and stale technical details
- **trove.md** (15K+ chars) was frozen at March 13, competing with the actual codebase as source of truth
- **Quinn's E2E gate** was discipline-only — 3 DMGs shipped without test reports (v0.59.1, v0.60.0, v0.61.0)
- **340+ builds** pushed directly to main with no PR review, no branch protection, no CI gates

The lesson: rules that rely on agent memory don't survive compaction. Rules that rely on human memory don't survive busy weeks. If it's not mechanized, it will be forgotten.

### What if we didn't decide?

We'd continue accumulating rules in workspace files that get truncated or compacted away, discovering months later that gates we assumed were active had never been enforced. Each failure would get a new rule added to AGENTS.md, making the file longer, making truncation more likely, making the next failure more likely. A degenerative loop.

## Decision

Commit a full day to structural fixes rather than patching symptoms. Convert discipline-based rules into mechanical enforcement wherever possible, and add tripwires where mechanization isn't practical.

### What we built (13 PRs in one day):

**Mechanical gates (CI blocks merge):**
1. `test` — frontend tests
2. `rust-tests` — Rust unit tests  
3. `spec-first-check` — code changes require spec update + CHANGELOG entry
4. DMG/Quinn — no build announced without timestamped test report

**Infrastructure:**
- Branch protection on main (no direct pushes, no force pushes, 1 review required)
- Self-hosted GitHub Actions runner (`ci-runner-1`)
- GitHub bot account (`britfreed`) for agent-authored PRs
- PR template with forced ADR yes/no question

**Documentation (the playbook replaces scattered rules):**
- Dev Hygiene Playbook (15K+ bytes, 10 sections)
- Matt's Checklist (9 sections — what Matt does at each stage)
- Rollback Procedure (7 steps for data-severity incidents)
- Review Checklist (10 sections — what Reed checks on every review)
- 7 foundational ADRs (platform, sync, data model, encryption, schema, architecture)
- Spec split into 12 domain files + baseline audit

**Bootstrap reduction:**
- AGENTS.md: 22,415 → 6,585 chars (71% reduction)
- MEMORY.md: 25,134 → 8,192 chars (67% reduction)
- trove.md: 15K+ → archived
- Total: ~50K chars removed from active workspace

**Tripwires (where mechanization isn't practical):**
- ADR filing: 3-week lag threshold before escalating to CI enforcement
- Weekly spec audits, ADR reviews, playbook reviews via Matt's checklist

## Alternatives Considered

1. **Keep patching AGENTS.md** — Add more rules, hope agents read them. Already proven to fail. Truncation makes this worse over time, not better.

2. **Mechanize everything** — Every rule as a CI gate. Rejected: some decisions (like "does this need an ADR?") are judgment calls that can't be reduced to pattern matching. Over-mechanization creates bypass fatigue.

3. **Do nothing, wait for Pane to solve it** — Pane will eventually be the context system. But Pane isn't launched yet, and the development process for building Pane was itself broken. Can't wait for the product to fix the process that builds the product.

## Consequences

**Positive:**
- Bootstrap files now fit comfortably under caps (24K/120K vs. 70K+ before)
- No rule can be silently lost to truncation — critical rules are either in CI or in a single playbook file
- Every code change goes through a reviewable PR with mechanical gates
- Matt has a concrete checklist for his role at each stage
- Scar tissue is documented where it happened (e.g., "Added after discovering 3 DMGs shipped without Quinn reports")

**Negative:**
- PR workflow adds friction to every change (intentional — friction is the point)
- Self-hosted runner is a maintenance surface (launchd plist, PATH config, Rust toolchain)
- Playbook is another document to keep current (mitigated by weekly review cadence)

**Risks:**
- Discipline-based gates (Reed review, ADR filing) can still fail — tripwires reduce but don't eliminate this
- CI gates can be bypassed with `--admin` merge — acceptable pre-launch when Matt is the only reviewer

## References

- Dev Hygiene Playbook: `docs/dev-hygiene-playbook.md`
- Matt's Checklist: `docs/matts-checklist.md`
- Review Checklist: `docs/REVIEW-CHECKLIST.md`
- Rollback Procedure: `docs/rollback-procedure.md`
- Milestone #16 (Dev Hygiene): all issues #499-#533
- Bootstrap discovery: OpenClaw loads exactly 8 hardcoded files at startup

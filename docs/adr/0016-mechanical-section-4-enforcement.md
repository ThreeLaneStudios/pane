# ADR-0016: Mechanical §4 Enforcement

**Status:** Accepted
**Date:** 2026-04-29
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None (extends the §4 enforcement layer added 2026-04-28)

## Context

§4 of the Dev Hygiene Playbook ("state-mutating fix ships with an integration test calling the production function") is one day old. It already saved the gateway from shipping with six audit blockers — all six the same shape: tests pass on building blocks, full call path is untested. The rule works.

It also got violated twice on the same task in the day after it shipped. PR #772 (memory router threshold fix, issue #719) is the reference incident:

**Attempt 1.** Stia shipped a test in `*_integration_test.rs` named after the production behavior, but the test body called an internal helper (`route_facts_to_file`, made `pub(crate)` for testability) and asserted against its own hardcoded threshold rather than driving the production entry point through `route_memory_facts`. The test passed; the rule was structurally violated. Pattern: **stub-shaped test, named like an integration test, calling a `pub(crate)` helper instead of the entry point.** This is the exact failure mode item #744 was filed to catch by manual review, and it still got past Reed.

**Attempt 2.** Sent back. Stia shipped a test that genuinely tried to drive the production worker, but couldn't get past `tauri::AppHandle` — the worker's entry point is coupled to a Tauri runtime handle that doesn't have a test constructor. Stia filed #774 to track the architectural untestability and shipped a test that drove the closest reachable function, with a comment explaining why. Pattern: **architecturally untestable production code,** mitigated by a tracking issue and a comment but not enforced by anything.

Two violations on one task is signal, not noise. Both pass current §4 enforcement (the grep for any test in `*_integration_test.rs`); neither would pass the rule's intent. The discipline floor is leaking.

The lesson from ADR-0008 applies directly: **discipline-only floors don't survive compaction.** Reed forgot the artifact-vs-claim distinction between attempts. Stia rationalized attempt 1's stub shape because nothing mechanical caught it. The pattern that lives in agent memory ("call the production function") evaporates the moment the agent doesn't notice they're not doing it.

The rule itself is fine. What's missing is mechanical enforcement of what the rule already says. This ADR mechanizes it.

### What if we didn't decide

Three predictable failure modes:

- **§4 erodes the same way Quinn's E2E gate eroded pre-ADR-0008.** Three DMGs shipped without test reports because "Quinn runs E2E" lived in agent memory only. §4 will follow the same path: violations slip in occasionally, then frequently, then nobody remembers what the rule was supposed to catch.
- **The audit blockers come back.** The six gateway-build saves were all the same pattern (router middleware, idempotency placeholder, idempotency error path, reconciliation short-circuit, session mapping, plus one more). The framework caught them once. Without mechanization, future iterations of the same pattern will ship.
- **Untestability becomes the standing escape hatch.** PR #774-shaped problems (production code architecturally hard to test) are the genuinely hard case. Without a structured exit path, agents will rationalize "this is one of those untestable cases" for code that is in fact testable, just not conveniently. The escape hatch becomes the default.

The rule has been live for a day. The prudent move is mechanizing it now while the violation pattern is fresh, not after the floor has eroded for a week.

## Decision

Mechanize §4 in three layers, ordered by leverage. Layer 1 and Layer 2 ship in this sprint. Layer 3 is named for the post-launch backlog.

### Layer 1 — AST grep in CI: integration tests must call a production entry point

A new CI workflow scans every test file matching `*_integration_test.rs` (and equivalent patterns under `src-gateway/tests/integration/`). For each `#[test]` or `#[tokio::test]` function, the workflow parses the test body and verifies it contains at least one call to a production entry point — a function exported from the crate's non-test modules.

**What counts as a production entry point.** A function defined under `src-tauri/src/` (excluding `src-tauri/src/tests/` and `*_test*.rs`) or `src-gateway/src/` (excluding `src-gateway/tests/`) and visible to the test (`pub` or `pub(crate)`). Helper functions defined inside the test module or imported from `*_test_utils.rs` do not count. Standard library calls, third-party crates, and `assert!`/`assert_eq!` do not count.

**Implementation shape.** Layer 1 uses `syn` to parse the test file's AST. Grep over source text is unreliable (false negatives on macros, false positives on comments). `syn` is already a build-time dependency for the workspace; the CI step compiles a small standalone analyzer that walks `ItemFn` bodies and flags ones with no call to a known-production-symbol set. Symbol set is derived per-PR from the source crate's public API surface.

**Allowlist.** Two annotation forms, both visible in PR diffs:

- `// §4-helper-test: <justification>` — used when an integration-shaped test legitimately tests a helper rather than a production entry point. Rare. PR review catches abuse; the annotation makes the abuse pattern visible.
- `// §4-untestable: #<issue-number>` — used when the production entry point is architecturally untestable today (the PR #774 case). Requires a linked tracking issue. CI separately verifies the linked issue exists and is open. **Closes the loophole where the annotation never gets resolved** — when the tracking issue closes, CI re-flags the test on the next PR that touches the file, forcing either re-test or re-annotation against a new issue.

The §4-untestable allowlist is deliberately friction-bearing. It costs an issue file. It re-fires when the issue closes. It is meant to be uncomfortable enough that agents prefer fixing the testability problem.

### Layer 2 — Test-the-test required as PR artifact

CI inspects PR bodies on PRs that touch `*_integration_test.rs` files. The body must contain a structured section in this shape:

```
## §4 test-the-test
Reverted: <commit SHA, file:hunk, or "deleted production function X">
Output:
<paste of cargo test failure including the assertion that fired>
Restored: <commit SHA or "reapplied">
```

CI checks for the `## §4 test-the-test` header and the three required fields. Empty fields fail the check; placeholder text ("TODO", "n/a") fails the check. The same workflow that runs the spec-first gate is the natural home for this — same shape, same enforcement model.

**Why this layer.** Layer 1 catches *structural* stub-shapes (test doesn't call the production function). Layer 2 catches *behavioral* stub-shapes (test calls the production function but doesn't actually depend on it — the test passes whether the production code is right, broken, or deleted). Stia's revised PR #772 produced exactly the test-the-test artifact voluntarily; mechanize that voluntary discipline.

The artifact distinction matters. "I ran the test and it failed" is a claim. The pasted output is an artifact. The pasted-output convention is what makes Reed's PR review productive in this dimension — Reed reads the artifact, not the claim. Without the artifact, "I tested it" is a checkbox; with the artifact, a stub test is visible at review time.

### Layer 3 (deferred, post-launch) — Weekly cargo-mutants on production paths

`cargo-mutants` runs the test suite against systematically-mutated versions of production code. Mutants that survive (i.e., tests still pass) are tests that don't actually depend on the code's behavior — stub tests by construction.

This is the strongest version of the rule. It's also slow (full mutation runs are hours), expensive (CI minutes, contributor friction), and has high noise (some mutants are equivalent or unreachable). Wrong tool for per-PR enforcement; right tool for periodic audits.

**Named here, not built.** Post-launch, when the gateway has stabilized and the team has bandwidth for a slower verification loop, run `cargo-mutants` weekly on the highest-risk production paths (memory pipeline, sync, gateway). Surviving mutants become issues for the test suite to address. Don't spec the implementation here — spec it then, when the constraints are real.

### Layer responsibilities — what each layer does and does not catch

Naming this explicitly so future readers don't expect Layer 1 to close gaps Layer 2 owns:

- **Layer 1 catches structural stubs.** Tests in `*_integration_test.rs` whose body contains zero calls to any production symbol. Reproducer: hand-rolled assertions over hardcoded values, no production code touched.
- **Layer 2 catches behavioral stubs.** Tests that *do* call production symbols (passing Layer 1) but where the call is incidental — the test would still pass if the production code were reverted, deleted, or replaced with `todo!()`. The PR #772 attempt 1 reference incident is exactly this shape: `route_facts_to_file` was `pub(crate)` and therefore counted as a production symbol under Layer 1's definition, but the test asserted against its own hardcoded threshold rather than depending on `route_facts_to_file`'s behavior. Layer 2's test-the-test artifact catches this — reverting `route_facts_to_file` would not have changed the test's outcome.
- **Layer 1's `pub(crate)`-permissive definition is deliberate.** Tightening "production entry point" to "reachable from a `tauri::command` handler or exported via `lib.rs`" produces false positives on legitimate `pub(crate)` entry points in subsystems (memory router, sync queue, watcher) and adds review friction every time. The defense-in-depth split — Layer 1 catches structural stubs cheaply, Layer 2 catches behavioral stubs via artifact discipline — closes the loophole at lower aggregate cost.
- **Neither layer catches "test asserts on the wrong behavior."** A test that calls the production function, depends on its output (Layer 2 passes), but checks the wrong invariant is judgment-only. Reed catches this in review. No mechanization can substitute.

### What the rule does NOT mechanize



Three things deliberately stay discipline-only:

- **Whether a test "really" exercises the right behavior** beyond calling the production function. This is judgment. Layer 1 ensures the call exists; Layer 2 ensures the test depends on the production code; verifying the test covers the *right* behavior is Reed's job. No grep can catch a test that's structurally valid but checks the wrong thing.
- **Whether a function is "state-mutating" and therefore subject to §4.** The §4 rule applies to state-mutating functions, not pure helpers. Classifying a function is judgment. Layer 1 fires on every test in `*_integration_test.rs` — which is a proxy that's tight enough to be useful. If a pure-helper test ends up in `*_integration_test.rs`, the agent uses `// §4-helper-test:` to opt out. Filename convention as the trigger keeps the rule mechanical without trying to classify functions automatically.
- **Whether to file an ADR for genuinely-untestable production code.** Some `// §4-untestable:` cases (like #774) reveal an architectural problem worth an ADR, not just an issue. Whether the issue rises to ADR-level is judgment. Layer 1 + the linked-issue check ensures the problem is *tracked*; whether it gets an ADR is Reed/Matt's call.

## Consequences

### Positive

- **Stub-shaped tests can no longer pass §4 enforcement.** Layer 1 catches the attempt-1 pattern from PR #772. Layer 2 catches the subtler version (test calls production function but doesn't depend on it).
- **Untestability becomes friction-bearing rather than free.** `// §4-untestable: #NNN` requires a tracking issue. When the issue closes, CI re-fires. The standing escape hatch closes itself.
- **Test-the-test discipline becomes an artifact, not a claim.** PR reviewers (Reed, Matt) can see the failure output instead of trusting it happened. Same pattern as the spec-first and changelog gates: the gate enforces existence of evidence, not absence of badness.
- **Pattern is reusable.** The §4 rule itself is not the only discipline-floor that's leaking. Once the AST-grep and PR-body-artifact patterns are in CI, future rules can use the same shape (cf. ADR-0008's "convert discipline into mechanism" framing). This ADR builds the toolkit, not just the gate.
- **Aligns with ADR-0008's mechanization principle.** "If it's not mechanized, it will be forgotten." §4 was discipline-only for one day; the failure pattern was already visible. Mechanizing now is consistent with the framework, not premature.

### Negative

- **Adds CI complexity.** Layer 1 needs an `syn`-based analyzer maintained as part of the CI surface. Layer 2 adds another required-status-check. More moving parts, more places for CI to fail unrelated to the actual change. Mitigated by: the analyzer is small (~200 lines of Rust), runs in seconds, and Layer 2 is a regex check on PR body — both are simple as enforcement workflows go.
- **Friction on legitimate untestable cases.** The PR #774 case (AppHandle coupling) is real: production code that genuinely can't be unit-tested without architectural change. Forcing an issue file + comment annotation costs time on each occurrence. This is intentional friction — the design wants untestability to be uncomfortable — but it does mean some PRs get slower for legitimate reasons.
- **`syn` parsing is not bulletproof.** Macros that hide function calls (e.g., `tauri::command!` expanding to internal helpers) can produce false negatives. Mitigated by: the analyzer treats macros conservatively (counts calls inside macro arguments toward the production-call set when the macro is on the known list), and the allowlist annotations cover edge cases the analyzer can't.
- **Layer 2's PR-body check is bypassable by anyone willing to paste fake output.** A PR author could paste fabricated cargo test output. The check is artifact-existence, not artifact-truth. Mitigated by: this is the same trust model as the spec-skip and ROOT_CAUSE_CONFIRMED gates — the gate raises the cost of bypass and makes the bypass visible in the PR diff. Reed catches fabricated artifacts in review. Pre-launch, the marginal attacker is "agent rationalizing under context pressure," not "agent deliberately faking output."

### Risks

- **Layer 1 by itself does not enforce "test drives the production entry point."** A `pub(crate)` helper counts as a production symbol under Layer 1, which means PR #772's attempt-1-shape (test calls `route_facts_to_file` directly with hardcoded asserts) would *pass* Layer 1 if it had been written with one direct call instead of zero. This is intentional — see "Layer responsibilities" in the Decision section. The behavioral guard is Layer 2 (test-the-test artifact). Tightening Layer 1 was considered and rejected as producing false positives on legitimate `pub(crate)` entry points; the layered model is the answer.
- **Over-mechanization fatigue.** ADR-0008 explicitly warned against mechanizing every rule. This ADR mechanizes one rule. The risk is precedent: each future "discipline floor leaked" event triggers another mechanization layer until CI runtime balloons or every PR carries five required-status-checks. Mitigation: §4 mechanization is justified by two violations on the same task in 24 hours, which is the bar. If a future rule has the same evidence base, mechanize. If it has weaker evidence, don't.
- **Layer 1 false-negatives on dynamic dispatch.** Tests that call production code through trait objects, channels, or async runtimes may not present as direct calls in AST. Mitigated by: trait method calls are still resolvable to a production symbol via type inference, which `syn` can approximate. Cases that genuinely defeat AST analysis become `// §4-helper-test:` annotated, which is correct behavior (the test isn't structurally proving what its name claims, even if it's behaviorally fine).
- **Issue #774 sets a pattern.** If `// §4-untestable: #774` becomes a frequent annotation, the underlying problem (AppHandle coupling, or whatever architectural fact is forcing untestability) is a real ADR-worthy concern, not just a tracking issue. Mitigation: weekly playbook review (per existing cadence) flags untestability annotations as a category. If three or more untestable annotations exist for related reasons, the playbook review files an ADR for the architectural fix.
- **"§4 is one day old, premature to mechanize" is a real argument.** Two violations on one task could be teething pain rather than a stable pattern. Address explicitly in Alternatives Considered.

## Alternatives Considered

**Wait one more cycle before mechanizing.** Track §4 violations for another week, then decide. Defensible argument: rule is 24 hours old, two violations on one task is a small sample, mechanization has overhead, ADR-0008 warns against over-mechanizing. **Rejected** because the violation pattern is structural, not random. Attempt 1 (stub-shape) is exactly the failure mode #744 was filed to catch by *manual* review, and the manual review still missed it. Attempt 2 (untestability with no structured exit) is exactly the failure mode that creates standing escape hatches if not addressed early. Both are predictable, both will recur, both are cheap to mechanize now and expensive to mechanize after the floor has eroded for a week. The "wait" alternative trades 1 week of latency for measurably worse signal — the violations that happen during the wait period get harder to attribute to "rule is too new" vs. "mechanism is missing." The decision sequence that produced ADR-0008 explicitly applies: if it's not mechanized, it will be forgotten.

**Mechanize Layer 1 only, defer Layer 2.** Ship the AST grep, leave test-the-test as discipline. Defensible argument: Layer 1 is higher-leverage (catches structural stubs entirely), Layer 2 is incremental (catches behavioral stubs that Layer 1 lets through), splitting them reduces ADR scope. **Rejected** because PR #772's two attempts are exactly the two failure modes Layer 1 and Layer 2 catch respectively. Shipping only Layer 1 closes the structural failure but leaves the behavioral failure (test calls function but doesn't depend on it) as an open hole. Both layers ship together because both failure modes already happened on the same task.

**Block PRs with untestable production code instead of allowlisting.** Force refactor before merge — no `// §4-untestable:` escape hatch. Defensible argument: the escape hatch is the path of least resistance and will be abused. **Rejected** because some untestability is genuinely hard architectural work (AppHandle in PR #774 needs Tauri redesign, not a quick fix). Blocking the merge stalls fixes that are valuable independent of testability. The friction-bearing allowlist (issue required, CI re-fires when issue closes) raises the cost of the escape without making it impossible. If the allowlist gets abused (3+ similar annotations), the weekly playbook review escalates to architectural ADR.

**Mechanize via cargo-mutants directly (skip Layer 1 + 2, jump to Layer 3).** Strongest version of the rule, no incremental layers. Defensible argument: cargo-mutants catches everything Layer 1 and Layer 2 catch, plus things they don't. **Rejected** for now: cargo-mutants is too slow for per-PR CI and too noisy for per-PR enforcement (false-positive mutants in equivalent-code paths). It's the right tool for periodic audits, not for the per-PR mechanization §4 needs. Filed as Layer 3 for post-launch.

**Do nothing, rely on Reed review.** Reed already reads PRs; tightening the §4 review checklist (#744) should catch stubs without CI machinery. **Rejected** because #744 was already in place when PR #772's attempt 1 shipped, and it didn't catch the stub. Manual review is the right last line of defense, not the right first line. ADR-0008's framing applies: discipline-based gates can fail; mechanical gates fail loudly.

## Coordination

Three coordination points flagged at spec-lock time. None block ADR acceptance; all need attention before or during implementation.

**Playbook §4 patch overlap with #779.** This ADR's session produced a patch to `docs/dev-hygiene-playbook.md` §4 that adds the state-mutating-function subsection and references ADR-0016 for enforcement. #779 (E2E testing ADR session) is also expected to patch §4, specifically the smoke/full-hybrid section that's currently aspirational. The two patches don't touch the same lines, so a sequential merge resolves cleanly either order. **Recommended sequence:** #779's patch lands first (broader scope, restructures §4 around what E2E gates now look like), this session's patch rebases onto its result. If timing forces this session's patch first, the §4 subsection lands cleanly and #779's patch absorbs the smoke/full-hybrid section in its own pass. Both spec sessions should know the other exists; this ADR was drafted assuming #779 has not yet landed.

**Smoke E2E status for PR #772 (resolved 2026-04-29).** Smoke ran and passed for PR #772. E2E doesn't catch unit-level stub patterns, and shouldn't be expected to. The §4 violation slipped through despite E2E green — strengthens the case for Layer 1 / Layer 2 mechanization.

**Sequencing vs #774 (AppHandle untestability follow-up).** ADR-0016 implementation lands *before* #774's AppHandle refactor. The dependency runs ADR-0016 → #774, not parallel:

- If ADR-0016 lands first: #774's refactor lands into a working enforcement layer. Its new integration tests are validated against the gates that prompted #774's existence. The `// §4-untestable: #774` annotation can resolve cleanly.
- If #774 lands first: its new integration tests pass `cargo test` but bypass enforcement that doesn't exist yet. The annotations get added retroactively, the issue-open check runs against a closed issue when it eventually lands, and the cycle is messier than necessary.

Brit coordinates the merge order. Workflow stubs (steps 1–6 in `section-4-workflow-stubs.md`) take 3.75–4.75 days; #774's AppHandle refactor estimate isn't in this session's scope but should be checked against this dependency before either work item is queued.

## References

- Spec section: `docs/dev-hygiene-playbook.md` §4 (testing discipline) — patched in this sprint to reference this ADR
- Related ADRs: ADR-0008 (mechanical dev hygiene over discipline-based rules — the pattern this ADR follows)
- Reference incident: PR #772 (memory router threshold fix, issue #719). Both attempts cited in Context.
- Tracking issues: #739 (CI workflow for §4 enforcement, base layer), #744 (manual return-checklist enhancement for §4), #774 (AppHandle untestability follow-up — first `// §4-untestable:` candidate)
- Bootstrap discovery for the lesson: gateway audit 2026-04-29 found 6 issues, all the same shape ("tests pass on pieces, integration was missing")
- Future work: Layer 3 (`cargo-mutants` weekly audits) deferred to post-launch, no spec yet

## Shadow Window Outcome

- **Window start:** 2026-04-30 ~1:15pm EDT (v0.65.0, PR #793)
- **Criteria:** 3 PRs touching integration tests OR 2 days (whichever longer)
- **Actual exposure:** 4 PRs, 20 workflow runs, ~21 hours

| PR | Branch | AST Check | Test-the-Test | Notes |
|-----|--------|-----------|---------------|-------|
| #797 | fix/adr-0017-migration-and-path-fix | ✅ pass | ✅ pass | Clean |
| #802 | fix/796-800-project-memory-router-and-autocreate | ✅ pass | ❌→❌→✅ | **Real catch** — test-the-test section missing, added on 3rd push |
| #805 | feature/issue-803-memory-backfill | ✅ pass | ✅ pass | 5 runs, all green |
| #808 | fix/805-backfill-deleted-at-crash | ✅ pass | ✅ pass | 6 runs, all green |

- **False positives:** 0
- **Real catches:** 1 (PR #802, missing test-the-test section — the exact behavioral-stub-shape signal the gate was designed to detect)
- **Flip date:** 2026-05-01
- **Decision:** Matt approved early flip. Shadow data exceeded calendar-time exposure expectations. 2-day floor was a safety buffer; 4 PRs with 20 runs provided better signal than waiting.

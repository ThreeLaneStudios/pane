# ADR-0018: CI Smoke Check for Startup Log Severity

**Status:** Accepted
**Step 4 activated:** 2026-05-03 — Smoke check promoted to required-status after 20+ consecutive PR passes with zero failures.
**Date:** 2026-04-30
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None

## Context

PR #690 (transcript archive feature, commit `4015735`, landed before v0.64.0 on 2026-04-28) introduced a code path that emits `[CRITICAL] Migration failed: SCHEMA_MISMATCH:61->62` on every app launch for any user upgrading from v0.63.x. The bug shipped through every existing gate undetected:

- CI passed (rust-tests pass on fresh-DB fixtures; the upgrade path isn't exercised).
- Reed audited PR #690 (audit found no PR-vs-spec mismatch; the bug is a spec-vs-spec contradiction, not a PR-vs-spec issue).
- Quinn ran Tier 1 + Tier 2 Playwright on v0.64.0, v0.64.1, v0.65.0 (E2E tests don't read startup logs).
- Matt verification didn't catch it (DMG launch suppresses stderr).

Detection latency: 2 days, 3 releases. The bug surfaced only because Matt launched from Terminal during an unrelated investigation. Full timeline and gate-by-gate analysis: `docs/audits/2026-04-30-pr690-migration-incident.md`.

The systemic finding from the postmortem is more important than the migration bug itself: **a `[CRITICAL]` log firing on every fresh launch shipped through every gate undetected for two days.** The discipline of "read the startup log" exists implicitly — somebody should have noticed — but lives in nobody's job description, which is the same as nobody's. ADR-0008's mechanization principle applies directly: discipline-only floors don't survive compaction. The incident proves the rule; mechanize.

ADR-0016 provided the immediate template (one day before this incident, in fact). The shape there: convert "read the test output" discipline into a CI artifact gate. Same shape applies here: convert "read the startup log" discipline into a CI smoke check.

### What if we didn't decide

Three predictable failure modes:

- **The next §2B.3-style spec contradiction produces the same shape of incident.** ADR-0017 reconciles §2B.2 and §2B.3, but the broader pattern (spec contradictions producing logs that nobody reads) is unresolved. The next pair to contradict each other ships the same way.
- **`[CRITICAL]` becomes meaningless over time.** If `[CRITICAL]` lines are emitted by code paths that nobody verifies are absent in production, the severity prefix degrades into noise. The next genuine critical log is harder to spot because the existing ones have been firing for weeks.
- **Detection lives in Matt's eyeballs.** The current detection mechanism is "Matt launches from Terminal occasionally." That's not a gate; that's a coincidence. Coincidence-as-gate is the discipline-only failure mode this codebase has been systematically removing.

## Decision

Add a CI smoke check that builds the app, launches it against a fixture DB, captures the first N seconds of stderr, and fails the build if the captured output contains any line matching a configurable severity-pattern set. The severity patterns at launch:

- `[CRITICAL]`
- `[FATAL]`
- `panicked at`
- `thread '.*' panicked`
- `RUST_BACKTRACE=`

The check ships in three layers, mirroring ADR-0016's structure but smaller in scope (no test-the-test artifact equivalent — the smoke check is the artifact).

### Layer 1 — Smoke launch with fresh-DB fixture

CI builds the Tauri app and launches it under a smoke harness that:

1. Initializes a clean fixture directory (`~/.pane-smoke-test/` or equivalent in the runner).
2. Launches the app with stderr captured to a file.
3. Waits for either (a) a "ready" signal (the existing post-init log line, e.g., `[MIGRATIONS] ✓ Schema verified` or equivalent), or (b) a 30-second timeout.
4. Sends the app a clean shutdown signal.
5. Greps the captured stderr for severity patterns. Any match → fail with the offending line(s) printed in the CI log.

The fixture is fresh each run. This catches the basic class of failure: a launch that emits `[CRITICAL]`, `[FATAL]`, or panics on a clean DB.

### Layer 2 — Smoke launch with upgrade fixture

The fresh-DB launch wouldn't have caught PR #690 directly because PR #690's bug only fires on existing DBs being upgraded. To close that gap, the smoke harness also runs against an upgrade fixture:

1. The fixture is a v60-stamped DB shipped in the test fixtures directory (`tests/fixtures/upgrade-from-v60.db`). It contains the schema as-of v0.62.0 (v60) plus a small amount of seed data.
2. The smoke harness copies the fixture into the test workspace, launches the app pointing at it, captures stderr.
3. The launch should succeed: the v60→v61 and v61→v62 explicit migrations (per ADR-0017) run, schema stamps to v62, app reaches "ready."
4. Same severity-pattern grep as Layer 1.

The upgrade fixture is updated whenever a new schema version ships. Specifically: when bumping `SCHEMA_VERSION` from N to N+1, the engineer updates the fixture to the schema-as-of-N (capturing the previous canonical state). This is mechanical and lives in the same PR as the migration; ADR-0017's per-pair migration model makes the ownership clear.

**This layer is the one that would have caught PR #690 on its own PR CI run.** A v61-stamped fixture, launched against PR #690's binary, would have produced the `[CRITICAL]` log.

### Layer 3 — Allowlist annotations for known-safe severity emissions

Some `[CRITICAL]`-prefixed logs are legitimate (e.g., panics in test-only code paths that the smoke harness exercises). Mirroring ADR-0016's allowlist pattern:

- `// §smoke-allowlist: <regex> — <justification>` placed in source code immediately above the `eprintln!` (or equivalent) that emits the line. CI parses these annotations during build, builds the per-PR allowlist, and excludes matches from severity-fail.
- The annotation's regex must be specific enough that it doesn't mask future real critical logs in the same file. A blanket `.*` regex fails the parse (CI rejects the annotation); regexes must contain a non-trivial literal substring.
- Each allowlist annotation includes a justification visible in the PR diff. Reed catches abuse during review.

There is no `// §smoke-untestable:` analogue. Untestability is a meaningful concept for `*_integration_test.rs` (some production code is architecturally hard to test); it is not a meaningful concept for startup logs (any startup-log emission is testable by definition — the smoke harness sees it).

### What this layer does NOT mechanize

- **Whether a startup log line is "really" a problem if it doesn't match a severity pattern.** Plain `[INFO]` and `[WARN]` lines are not gated. A bug that causes silent feature failure with no log signature isn't caught by this layer. Mitigation: spec coverage, test coverage, Reed audits — same as before.
- **Logs emitted *after* the smoke harness's ready signal or 30-second timeout.** The check is bounded by app-start time. Bugs that fire `[CRITICAL]` only after the user does something don't surface here. Mitigation: Layer 1 catches startup; runtime issues are separate (E2E and Quinn checklist).
- **The contents of the upgrade fixture.** The fixture is a static DB file; it captures one historical state. Bugs that only fire on data shapes the fixture doesn't include aren't caught. Mitigation: the fixture is updated when schemas change; for diversity, additional fixtures can be added later (e.g., `large-data.db`, `multi-agent.db`) — out of scope for v1 of this gate.

## Consequences

### Positive

- **PR #690's exact failure pattern would have been caught on the PR itself** (Layer 2, upgrade fixture). Detection latency goes from 2 days to before-merge.
- **Closes the discipline-only "read the log" floor.** Same mechanization principle as ADR-0008 and ADR-0016 — convert "somebody should notice" into "CI fails loudly."
- **Severity prefixes (`[CRITICAL]`, `[FATAL]`) regain meaning.** If a `[CRITICAL]` line in source guarantees CI failure, the prefix becomes a contract instead of a comment. Makes future genuine criticals easier to spot.
- **Catches a broader class than PR #690's specific bug.** Any future startup-time panic, `[CRITICAL]` emission, or unhandled severity log fails CI. Future contributors don't have to know about the specific PR #690 pattern; they just stop emitting critical logs that fire on launch.
- **Fixture-driven approach is extensible.** Once the harness exists, additional fixtures cost ~zero. The first ones (fresh, v60-stamped) cover the immediate need; later additions can cover other startup paths.

### Negative

- **CI runtime cost.** Building the Tauri app, launching it, waiting for ready signal — adds 1-3 minutes to CI per smoke fixture. With 2 fixtures (fresh + upgrade), 2-6 minutes added per PR. Mitigation: the smoke check runs in parallel with rust-tests, frontend tests, etc. — additive to wall-clock, not additive to elapsed.
- **Tauri build complexity in CI.** The smoke harness needs a working Tauri build environment in CI. The self-hosted runner (`ci-runner-1` per ADR-0008) already builds Tauri for DMGs, so the build environment exists; the smoke step layers onto it. Cross-platform Linux smoke (if added later) requires more setup — out of scope for v1.
- **Upgrade fixture maintenance.** Every schema bump requires updating the fixture. Mitigation: the fixture update is a checklist item in the migration-PR template (lands alongside the playbook patch). Mechanical, not discretionary. ADR-0017's per-pair migration model already makes the schema-bumping engineer think about each version pair; updating the fixture is in the same scope.
- **Allowlist abuse risk.** `// §smoke-allowlist:` could be applied liberally to make CI green without fixing the underlying issue. Mitigation: regex-must-contain-literal-substring rule + Reed review + weekly playbook review of allowlist annotations (parallel to ADR-0016's `// §4-untestable:` review cadence).

### Risks

- **The 30-second ready timeout is wrong.** Too short, and slow-launching paths fail spuriously; too long, and CI eats real time on every PR. Mitigation: empirical tuning during implementation. Start at 30s; raise if false-failures > 1 in 100 runs; lower if it's never been close.
- **The upgrade fixture goes stale.** Engineer forgets to update it on schema bump; smoke check still passes against the old fixture even though the new migration is broken. Mitigation: a CI step that asserts the fixture's recorded schema-version is exactly `SCHEMA_VERSION - 1`. Forces the fixture update to land with the bump or CI fails on the bump PR for an obvious reason.
- **Stderr noise from third-party crates.** Some crates emit `panicked at` in non-fatal ways (caught panics, library warnings). Mitigation: if observed, add `// §smoke-allowlist:` at the boundary or filter the regex set. Empirical, not pre-spec'd.
- **Self-hosted runner availability.** The smoke check runs on `ci-runner-1`. If the runner is offline, smoke checks queue. Same risk as existing rust-tests on the runner; same mitigations (Brit owns runner uptime, alerting in place).
- **Layer 2 (upgrade fixture) is the high-value layer, not Layer 1.** If implementation pressure forces shipping only one layer first, Layer 1 is materially less useful — the bug we're closing the gate on requires upgrade fixture to catch. Mitigation: ship both. The spec session estimate is bounded for both layers together; splitting them breaks the value of the gate.

## Alternatives Considered

**Quinn checklist item only.** Add "review startup logs for CRITICAL/FATAL/panic markers" to Quinn's pre-DMG checklist. Discipline-based, captured in the playbook §8 patch. **Rejected as the only mechanism** because Quinn's checklist failed PR #690 (the smoke check exists implicitly; nobody ran it). The same pattern the incident exposed. Quinn's checklist can carry it as a redundant layer (and the playbook patch in this session adds it), but mechanization is the primary defense.

**Mechanize via DMG-time check, not PR-time CI.** Check startup logs at DMG-build time (before announcing). Catches the bug before user-facing distribution. **Rejected** because DMG-time is too late: the bug has already merged, the fix is more PRs and more friction. PR-time catches it before merge. ADR-0008's principle: catch at the earliest mechanical point. (DMG-time may still be useful as an additional layer, but PR-time is primary.)

**Read existing CI logs for severity patterns instead of building a smoke harness.** rust-tests already produces stderr; grep for severity patterns there. **Rejected** because rust-tests run with mocked databases and don't exercise the full launch path. The bug PR #690 introduced specifically requires running the actual app against a real fixture; unit-test stderr doesn't cover it.

**Mark `[CRITICAL]`-emitting code paths as `unreachable!()` post-fix.** Convert the migration-failure path from "log and continue" to "panic," so any `[CRITICAL]` emission would crash CI. **Rejected** because §2B.2 specifically requires graceful-fallthrough behavior on migration failure (data preservation rule from v0.15.0 incident). The right behavior on schema mismatch is to log loudly and continue with old schema; converting to panic violates ADR-0017's reconciliation. The smoke check's job is detecting the log emission, not preventing it.

**Cargo-based logging level filtering (e.g., `RUST_LOG=error`-driven gate).** Use Rust's logging levels instead of grep-based severity-prefix matching. **Rejected** for v1 because Pane uses `eprintln!` directly in many places (not a structured log framework). Migrating to `log` or `tracing` is a separate scope. Grep-based works against the current codebase; structured-logging migration can be a future ADR that tightens this gate.

**Defer to post-launch.** This bug is feature-unavailability, not data-loss. Ship the migration fix, defer the smoke check until after launch. **Rejected** because (a) the next §2B.3-style failure ships the same way, (b) launch-pressure is exactly when discipline-only floors fail, and (c) the implementation cost is bounded — Stia velocity, ~2-3 days per the workflow stubs in `adr-0018-workflow-stubs.md`. Cheap to do now; expensive when the next incident lands.

## Coordination

Three coordination points before implementation:

**Reed-Gate-protected paths.** The smoke check workflow lands under `.github/workflows/`, which per the playbook §1 path-restrictions requires `reed-reviewed` label. Spec session doesn't handle that; flag noted for Brit when queuing the implementation PR.

**ADR-0017 sequencing.** ADR-0017's migration fix and ADR-0018's smoke check are independent but mutually reinforcing. Recommended sequence: ADR-0017's migration fix lands first (closes the immediate v61→v62 gap), then ADR-0018's smoke check (prevents the next one). If the smoke check lands first, the upgrade fixture's v61-stamped DB will fail the smoke check until ADR-0017's migration ships — which is the right behavior, but means the smoke check goes red on main until ADR-0017 lands. Sequencing ADR-0017 first avoids the temporary red state.

**ADR-0016 (§4 enforcement) interaction.** ADR-0017's migration fix ships with a §4 integration test. That test is subject to ADR-0016's enforcement layers (Layer 1 AST grep, Layer 2 test-the-test artifact). The integration test specifically exercises the `init_database` upgrade path with a v61 fixture — same fixture mechanism the smoke check uses. The two ADRs share the fixture infrastructure; coordinate the fixture format so it's reused, not duplicated.

## References

- Postmortem: `docs/audits/2026-04-30-pr690-migration-incident.md` — full incident record, gate-by-gate analysis, residual-risk discussion
- Reference incident: PR #690 (commit `4015735`), Issue #794
- Related ADRs: ADR-0008 (mechanical dev hygiene — pattern this ADR follows); ADR-0016 (mechanical §4 enforcement — closely-related mechanization, same shape, reuses fixture infrastructure); ADR-0017 (migration fix — produces the v61-stamped fixture this ADR's Layer 2 consumes)
- Spec sections: `docs/dev-hygiene-playbook.md` §4 (testing discipline — patched to reference this ADR), §8 (DMG announcement gate — patched to add Quinn-side log review as a redundant discipline layer)
- Workflow stubs and acceptance criteria: `adr-0018-workflow-stubs.md` (this session)
- Future work (deferred to post-launch): structured logging migration to `log`/`tracing`, additional fixture diversity (large-data, multi-agent), cross-platform smoke (Linux/Windows)

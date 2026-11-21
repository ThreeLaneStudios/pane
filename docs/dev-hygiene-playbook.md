# Dev Hygiene Playbook - Pane

*Line citations predate the #3097 module split.*

*Baseline dev discipline for a solo non-dev builder working with AI agents.*
*Last updated: 2026-04-30*

## Core principles

1. **Nothing ships without a paper trail.** Every change traces back to an issue, a spec section, and a test.
2. **Spec first, code second.** If the spec doesn't describe the change, update the spec before writing code.
3. **Reversibility is cheaper than perfection.** Make every change revertable; recover fast when things break.
4. **Agents are fast but miss context.** Multi-stage verification catches what individual agents miss.
5. **Discipline compounds.** Small habits kept consistently outperform heroic one-time efforts.
6. **If it's not mechanized, it will be forgotten.** Rules that rely on agent memory don't survive compaction. Push enforcement into GitHub, CI, and scripts - not AGENTS.md.

---

## 1. Spec management

### Structure

- **Spec is the source of truth.** Lives in the repo at `docs/spec/`.
- **Split by domain.** Not one monolithic doc. `spec/core-data-model.md`, `spec/memory-pipeline.md`, `spec/sync.md`, `spec/tool-calling.md`, `spec/ai-integration.md`, `spec/ui.md`, etc.
- **Principles doc at the top.** `docs/spec/principles.md` - a short (≤2 page) overview of what Pane is, what it isn't, and non-negotiable architectural rules. Read once, referenced often.
- **Cheat sheet as index.** `docs/architecture-cheat-sheet.md` - short nav doc pointing to the right domain spec per topic.

### Discipline

- **No code change without spec update.** Either the PR updates the spec, or the PR explicitly notes "no spec change needed" with reason.
- **Spec changes land first, code second.** Update the spec in the same PR or as a prerequisite PR. Matt approves. Then coder implements against the now-canonical spec.
- **Periodic spec-vs-reality audit.** Monthly (or per major release), researcher audits whether the spec still matches what's actually built. Flag drift.
- **Spec is written for the reader, not the writer.** If Matt can't find the answer to a common question in the spec within 30 seconds, the spec needs better structure - not more content.
- Grep before declaring a key "new." When spec work introduces an ai_settings (or similar schema) key, search the existing spec and source first. internal_endpoint was treated as new across spec session, strategist review, and Brit's audit prep on 2026-05-19 - it had already been in docs/spec/sync.md's cascade-wipe table. Reed caught it. Cost was zero this time; cost is non-zero when the "new" key collides with an existing reader or migration.
- **Trace the §29 boundary for any AI call spec work.** When spec work touches an AI call path (chat, classification, extraction, rerank, synthesis, future), explicitly identify which side of the §29 dual-provider boundary the call lives on - primary (gateway-proxied for OC users) or internal (direct dispatch). Verify with execution-log evidence, not intent-log evidence. ADR-0038's 2026-05-19 amendment caught extraction running through the gateway despite Pane resolving the correct model identifier; the gateway rewrote the model field at chat.rs:37 and the intent log showed Pane's resolved name, masking the bug. Intent logs say what Pane meant; execution logs say what shipped. The two diverge at every gateway boundary.

### Enforcement

- **Machine-enforced:** CI workflow (`spec-first.yml`) fails if a PR touches `src/` without touching `docs/`. PR cannot merge.
- **Bypass:** `spec-skip: [reason]` in PR body - but **blocked on core behavioral paths** (`src-tauri/src/ai/`, `sync/`, `agents.rs`, `database.rs`, `migrations.rs`, `notes.rs`, `projects.rs`, `system_files.rs`, `sync_client.rs`, `import.rs`, `backup.rs`, `watcher.rs`, `src/lib/services/`, `src/lib/chat-memory.ts`). Core-path PRs must update `docs/spec/` - no skip allowed.

### External-system contracts must be captured at spec time

If a spec invokes an external system - third-party API, sibling service, OS interface, MCP server, OpenClaw RPC, BYOK provider - the spec must include the actual response shapes for every call, captured from the external system's source-of-truth: source code, official docs, or a live probe. "We'll figure it out during implementation" is not an acceptable spec output. Inferred shapes, plausible-looking field names, and naming conventions invented during drafting are not contracts; they are guesses that produce drift the moment code meets the real system.

If shapes are unstable or version-dependent, capture as "current as of version X" with a versioning policy. If shapes can't be captured for legitimate reasons (system not yet built, access restricted, vendor docs missing), file a spike issue and exit the spec session without lock - the spec is not ready.

The check happens at spec session output, before any ADR is filed against the spec or any implementation issue is queued. Spec sessions ask, for every external system the spec invokes, "do we have the actual response shapes? captured where? versioned how?" If the answer is "no" or "deferred to implementation," the spec session output is incomplete.

This rule was added 2026-05-02 after the §826 audit found 8 of 11 OpenClaw RPC methods had field-name or shape mismatches in §22C and the gateway implementation, with 3 cases hard-failing at runtime and 5 silently drifting. Root cause: the §22C spec session invented a third convention for OC field names without OC's actual response shapes in front of it. The pattern is general - every external integration is exposed to it. See ADR-0023.

**Format for captured shapes.** Inline JSON examples in the relevant spec subsection, alongside the Pane-facing surface. Each capture includes:

- The exact response body (request examples for non-trivial shapes)
- The source: "captured from `<source-path>` at version `<X.Y.Z>` on `<date>`"
- A versioning note: how this shape behaves under external-system version changes (stable, breaking-on-major, unstable)

Example (illustrative - actual format settles during the §22C amendment session):

> **OC `agents.list` response (current as of OC v0.71.0, captured 2026-05-01 from `~/.openclaw/source/.../agents.ts:142`):**
> ```json
> { "defaultId": "...", "mainKey": "...", "scope": "...", "agents": [{ "id": "...", "name": "...", "identity": "...", "workspace": "...", "model": "..." }] }
> ```
> Versioning: stable through OC 0.x. Audited each minor release.

The capture lives in spec, not in code comments. Code that adapts the shape references the spec capture; the spec capture references the source-of-truth. This produces a single canonical reference per integration.

### §2 Mechanical-Text-Edit Exception - Exhaustive Boundary

The following spec edits may land in a code PR without a separate spec PR:

**✅ Allowed (mechanical - no reviewer could disagree):**
- Status blockquotes updating build state: `> Status: BUILT (vX.Y.Z, PR #NNNN) - [description]`
- Status changes: `NOT BUILT → BUILT`, `PARTIAL → BUILT`, adding `SM-REMOVED`
- Version number bumps in spec status lines
- Strikethrough markers on deleted/scrapped sections
- Typo/spelling corrections to existing spec text (field names, spec identifiers, etc.) - no semantic change
- ADR status field updates: `Status: Accepted → Superseded by ADR-NNNN`, `Status: Accepted → Deprecated`, etc. - records lifecycle, not behavior

**Worked examples (mechanical - correctly landed in code PRs):**
- PR #1149: `> Status: BUILT (... field name alignment PR #1149)` in §22C.11A - records what shipped, no behavioral claim
- PR #1109: `> Status: SM-REMOVED` markers - records deletion, no behavioral claim
- PR #1154: `> Status: BUILT (... watcher load_extension fix PR #1154)` in §2A.1 - records bug fix, no behavioral claim

**❌ Not allowed (behavioral prose - requires separate spec PR first):**
- Descriptions of how code works ("The command extracts the wire message field from the 401 JSON body and returns it as the error string")
- Error-path documentation
- Implementation notes
- New prose of any kind, even if factually accurate
- Any text a spec reviewer could disagree with, rephrase, or request changes to

**Worked example (behavioral - should NOT have landed in a code PR):**
- PR #1124: Added "The `gateway_exchange_pairing_code` command extracts the wire message field from the 401 JSON body and returns it as the error string, enabling frontend error discrimination" to spec. This describes *how the code works*, not *what version it shipped in*. Even though factually accurate, it's spec authoring - a reviewer could disagree with the phrasing, request clarification, or restructure it.

**Litmus test:** Could a spec reviewer disagree with this text? If yes → it's behavioral prose → separate spec PR. Status markers are mechanical facts (merged = built) - no reviewer would dispute them.

### `spec-covered-by:` Marker

When a core-behavioral code PR's spec changes landed in a prior merged PR, use **both** markers in the PR body:

```
spec-skip: Spec landed in separate PR per going-forward rule (spec PR first, then code PR)
spec-covered-by: #NNNN [justification]
```

`spec-covered-by:` is evaluated **inside the `spec-skip:` exception path** in the CI workflow. Without `spec-skip:`, the gate hits the basic "source changed without docs/" check first and fails before reaching the `spec-covered-by:` verification. The `spec-skip:` line enters the exception path; `spec-covered-by:` provides the verifiable justification within it.

The CI gate verifies the referenced PR exists, is merged, and touched `docs/spec/`.

**One `spec-covered-by:` marker per PR.** If your change spans multiple prior spec PRs, reference the most recent one that covers the current change. The gate verifies only the first `#NUM` found.

---

## 2. Architecture Decision Records (ADRs)

### What they are

Short markdown files capturing architectural decisions. One file per decision. Numbered sequentially.

### Structure

Each ADR has:
- **Title** - "ADR-0007: Use Tauri instead of Electron"
- **Status** - Proposed / Accepted / Superseded by ADR-NNNN
- **Context** - What problem were we solving? What constraints?
- **Decision** - What did we choose?
- **Consequences** - What does this mean going forward? What did we give up?
- **References** - Link to related issues, spec sections, discussions

### Discipline

- Location: `docs/adr/` in the repo. Numbered `0001-short-name.md`, `0002-short-name.md`, etc.
- **Write an ADR for any decision you'd hate to re-litigate in six months.** Platform choices, sync strategy, data model changes, trade-offs with lasting consequences.
- Don't write ADRs for trivial decisions (variable naming, minor UI tweaks).
- **Reference ADRs from spec sections and issues.** When spec says "Pane uses CRDTs for sync," link to the ADR explaining why.
- ADRs are immutable once accepted. If the decision changes, write a new ADR that supersedes the old one. Never edit the original.

### Tripwire

ADR filing is discipline-based today. If it lags - three weeks pass with architectural changes shipping but no ADRs filed - revisit with lightweight mechanization: keyword-trigger CI comment, automated weekly sweep of merged PRs with architectural language. Don't wait for someone to remember to ask "is this discipline still working?"

### Enforcement

- **PR template includes ADR checkbox.** Every PR asks: "ADR written (if architectural decision) - yes / not needed." Reviewer verifies.
- **Spec sections link to ADRs.** When a spec references a major decision, it must cite the ADR. Researcher catches missing references during audit.

---

## 3. Issue management

### Every change has an issue

- No work happens without an issue first. Applies to bugs, features, refactors, even doc updates.
- Issues reference the relevant spec section(s) and ADR(s).
- Issue title is descriptive enough to understand without opening it.

### Issue lifecycle

1. **Opened** - Matt or agent identifies need
2. **Scoped** - Researcher (Reed) audits: what's the change, what does it touch, any compatibility risks?
3. **Spec updated (if needed)** - PR with spec change, approved by Matt
4. **Implemented** - Coder (Stia) executes against spec, on a feature branch
5. **PR created** - CI runs automatically (tests, spec check, lint)
6. **Reviewed** - Researcher verifies code matches spec/intent. Pass/fail.
7. **Merged** - Only after CI green + review approval
8. **Tested** - Playwright E2E + agent testing on the merged build
9. **Matt verified** - Matt confirms on real workflow (Appendix B)
10. **Closed on merge** - Auto-closes via `Fixes #X` when the PR merges; reopened in the live-test report (step 9) if the fix fails

### Issue hygiene

- **Issues close on merge via `Fixes #X`, not on Matt's confirmation.** The verification gate didn't disappear — it moved: if Matt's live test fails, reopen the issue in that same test report (Appendix B, step 6/7).
- Link PRs to issues — `Fixes #NNN` when the PR fully resolves the issue, `Part of #NNN` for an earlier PR in a multi-PR issue. These auto-close (or stay open, for `Part of`) on merge via GitHub's native linking; that's intentional, not a mistake to avoid.
- If an issue sits untouched for 30+ days, decide: still relevant? Close it or prioritize it. Weekly sweep catches these.

### Enforcement

- **PR template requires issue reference.** Every PR must link an issue. Reviewer checks.
- **Issue close rule:** Issues close automatically on merge via `Fixes #X`/`Part of #X`. If Matt's live test fails, Matt or Brit (on Matt's behalf) reopens the issue in that same test report. Agents never manually close or reopen issues.

---

## 4. Testing discipline

### What gets tested

- **Playwright E2E for user-facing flows.** Every critical path has at least one E2E test. Currently: 20 test files across 3 tiers.
- **Unit tests for logic-heavy functions.** Pure functions, algorithms, parsers. Currently: 178+ Rust tests.
- **Integration tests for cross-component behavior.** Memory pipeline, sync, tool calling.

### State-mutating functions: integration test required

Every state-mutating function ships with an integration test that calls the production entry point through its full call path. Stub-shaped tests that call internal helpers and assert against hardcoded values do not satisfy this rule, even if they live in `*_integration_test.rs`.

The rule is mechanically enforced - see ADR-0016 for the enforcement design and rationale. What CI checks for:

- **Layer 1 (AST grep).** Every `#[test]` and `#[tokio::test]` in `*_integration_test.rs` or `src-gateway/tests/integration/` must contain at least one call to a production entry point (function exported from `src-tauri/src/` or `src-gateway/src/`, excluding test modules and helpers). Calls to internal helpers, test utilities, or third-party crates do not count.
- **Layer 2 (test-the-test artifact).** PRs touching integration test files must include a `## §4 test-the-test` section in the PR body with three fields: what was reverted, the cargo test failure output, and confirmation of restoration. Empty or placeholder fields fail the check.

Allowlist annotations, both visible in PR diffs:

- `// §4-helper-test: <justification>` - for the rare case where an integration-shaped test legitimately tests a helper. PR review catches abuse.
- `// §4-untestable: #<issue-number>` - for production code that is architecturally untestable today. CI verifies the linked issue exists and is open. When the issue closes, CI re-flags the test on the next PR that touches the file. Friction-bearing by design - see ADR-0016 for the rationale.

The §4 rule was added 2026-04-28 after the gateway audit found 6 issues all of the shape "tests pass on pieces, integration was missing." It was mechanized 2026-04-29 (ADR-0016) after PR #772 violated it twice on the same task - first as a stub-shape, then as architecturally untestable production code with no structured exit path. Discipline-only floors don't survive compaction.

### Test tiers

| Tier | What | When | Who | Time |
|------|------|------|-----|------|
| Unit (cargo test) | Rust logic, DB operations | Every build | Stia | ~2 min |
| Tier 1 (Playwright) | Core UI flows: CRUD, nav, auth | Every build | Stia | ~30s |
| Tier 2 (Playwright) | AI features: tool use, routing, memory | Before DMG ships | Quinn | ~7 min |
| Tier 3 (Playwright) | Edge cases: settings, shortcuts, kanban | Weekly / major release | Quinn | ~3 min |
| svelte-check | Type checking | Every build | Stia | ~30s |

**Stia NEVER runs Tier 2.** Quinn owns the Tier 2 gate.

### Smoke vs Full E2E (hybrid strategy)

Tauri has no headless webview support (upstream `tauri-apps/tauri#1061`, open since 2020). Our tests are 90% UI/integration - they need a real rendered webview. Hybrid strategy:

| Suite | Location | Runs in | Requires webview | What it catches |
|-------|----------|---------|------------------|-----------------|
| **Smoke** | `tests/e2e/smoke/` | CI (every PR) | No - mocked IPC, browser-only | Core regressions: app loads, routing, critical state transitions |
| **Full** | `tests/e2e/full/` | Local (pre-DMG) | Yes - real Tauri webview | UI rendering, DOM assertions, integration flows |

**Criteria for placing a test:**
- If it can run with mocked IPC and no webview → `smoke/`
- If it asserts on rendered DOM, visibility, layout, or real Tauri commands → `full/`
- When in doubt → `full/`. False negatives in smoke are worse than missing coverage.

### When tests run

- **On every PR:** CI runs unit tests + svelte-check + Tier 1 E2E. Fail = PR cannot merge.
- **Before DMG ships:** Quinn runs Tier 1 + Tier 2. Fail = no DMG.
- **Weekly (Sunday):** Full suite (all tiers). Quinn or cron.

### Test hygiene

- **Tests are documentation.** Name them clearly: `notes_and_tasks_use_the_same_data_model`, not `test_note_task_1`.
- **Broken tests get fixed, not skipped.** Skipping tests is technical debt that silently grows.
- **New features ship with tests.** If coder writes code, coder writes tests. Researcher audits test coverage.
- **Test shape: integration over unit for state mutations.** Every function that writes to persistent state - database, filesystem, network, in-memory store consumed by other code - ships with at least one test that:
  1. Sets up the full preconditions the function expects in production
  2. Calls the function under test
  3. Asserts the observable outcome of the mutation (the store/file/DB/network actually has the expected change)

  Unit tests of helpers, parsers, or building blocks do not satisfy this requirement. The test must exercise the call path the function takes in production and verify the side effect actually happened.

  Reviewers reject PRs that introduce or substantially modify state-mutating functions without a test of this shape, regardless of unit-test count or coverage percentage.

  Enforced mechanically: CI workflow (`integration-test-gate.yml`) flags PRs touching state-mutating paths and requires a corresponding test diff or matt-only `integration-test-skip: [reason]` label. Reed's pre-audit and post-audit verify the test exercises the actual call path - the CI tripwire ensures a test exists, the audit ensures it tests the right thing.
- If a bug makes it past testing, add a regression test before closing the issue.
- **Test-the-test before claiming green.** For integration tests on state-mutating code, revert the production change (or delete the function being tested), run the test, confirm it fails, restore, run it again, confirm it passes. Paste the failure output into the PR body's `## §4 test-the-test` section. The artifact is the gate, not the claim. See ADR-0016.
- **Review question for every new test:** "Delete the thing it tests — does it go red?" If nobody can answer that, the test isn't evidence. (Matt's rule, added after two false-green tests discovered in one day, 2026-07-25.)
- **Severity-prefix logs are contracts.** `[CRITICAL]` and `[FATAL]` lines emitted from production code paths fail CI on every PR via the startup-smoke check (ADR-0018). Don't use them for ordinary debug noise; if you do, allowlist them explicitly. The prefix means "CI breaks if this fires."

### Startup smoke: severity-prefix logs fail CI

Every PR runs a CI smoke check that builds the app, launches it against fresh and upgrade DB fixtures, and captures the first 30 seconds of stderr. The build fails if any captured line matches a severity-prefix pattern: `[CRITICAL]`, `[FATAL]`, `panicked at`, `thread '.*' panicked`, or `RUST_BACKTRACE=`. The full mechanism is spec'd in ADR-0018.

The smoke check exists because of PR #690 (chat archive migration, landed before v0.64.0 on 2026-04-28). PR #690 emitted `[CRITICAL] Migration failed: SCHEMA_MISMATCH:61->62` on every launch for two days across three releases (v0.64.0, v0.64.1, v0.65.0). CI passed, Reed audited, Quinn ran Tier 1 + Tier 2 - none read the startup log. The bug surfaced only when Matt launched from Terminal during an unrelated investigation. Full incident: `docs/audits/2026-04-30-pr690-migration-incident.md`. Discipline-only "read the logs" failed; mechanization is the floor.

Allowlist annotation: `// §smoke-allowlist: <regex> - <justification>`. Same shape as `// §4-untestable:` (ADR-0016). Regex must contain at least 4 consecutive literal characters; trivial regexes (`.*`, `.+`, `.`) fail the build. Justification visible in PR diff; Reed catches abuse.

The smoke check runs against two DB fixtures: a fresh install (catches startup-time crashes on clean DB) and an upgrade fixture stamped at `SCHEMA_VERSION - 1` (catches PR #690's exact failure shape). Schema bumps that miss the upgrade fixture update fail CI before merge, per ADR-0017's per-pair migration model.

### Schema PR Checklist

**Every PR that bumps `SCHEMA_VERSION` must:**

1. **Add a migration arm** (if users are on the previous version) OR **regenerate the smoke-upgrade-db fixture** (if pre-launch — no shipped users on the previous version).

2. **Pre-launch schema bumps (no migration arm):** Regenerate `tests/fixtures/upgrade-from-v{N-1}.db` as a v{N} database using `src-tauri/tests/build_vN_fixture.rs`. The fixture is stamped at the _current_ version so the app emits `Schema verified` (no migration attempted). `verify-upgrade-fixture.sh` accepts this.

3. **Never add a migration arm for a version that has never shipped.** Pre-launch schema bumps have zero users on the prior version; a migration arm is dead weight and a future maintenance hazard.

> **Rule:** Every pre-launch schema version bump must regenerate the smoke-upgrade-db fixture to the new version. No migration arms for unshipped versions.

### Enforcement

- **Machine-enforced (Rust tests):** CI required status checks block merge if tests fail. No human can override.
- **Machine-enforced (§4 integration test rule):** AST-grep workflow on `*_integration_test.rs` files (Layer 1) + PR-body artifact check (Layer 2). See ADR-0016.
- **Machine-enforced (startup smoke):** Smoke launch + severity grep on fresh and upgrade DB fixtures. See ADR-0018.
- **Pre-build script:** `scripts/pre-build.sh` is checks-only (architecture invariants, version parity, spec check, svelte-check) before `scripts/build.sh` (the only sanctioned path to `tauri build`, see `docs/INVARIANTS.md`). `cargo test`/`npm test` were removed from it (#3243, PR D2): CI (`rust-tests.yml`, `ci.yml`) and `pnpm test` own those now.

---

## 5. Verification gates

Every change passes through these gates in order. Skipping a gate = high risk of regression.

| Gate | Who | What | Enforced by |
|------|-----|------|-------------|
| 1. Pre-audit | Reed | Does this conflict with existing code? What does it touch? | Brit (spawn discipline) |
| 2. Implementation | Stia | Execute against spec, on feature branch | PR workflow |
| 3. CI checks | GitHub Actions | Tests pass, spec updated, types clean | Branch protection (machine) |
| 4. Post-audit | Reed | Code matches spec/intent? Read `~/Projects/example-app/docs/REVIEW-CHECKLIST.md` before every review. External-call capture check per ADR-0023. | PR review requirement (machine) |
| 4b. UX-flow exercise | Brit | For UI PRs: structural code-path trace of each user flow before requesting Matt's label. Catches wiring bugs (missing buttons, state clobber, scope traps) that Reed's code review misses. **Does NOT replace Matt's exercise** - catches structural/wiring only, not runtime/visual. Matt's exercise (Gate 6) remains mandatory. Added after cascade-UX incident. Proved value: caught provider-switch cascade bypass before reaching Matt. | Brit (discipline) |
| 4c. Source-read verification | Brit | Before claiming "X is built/fixed/deployed," run `git log main --oneline \| grep '<keyword>'`. If the commit isn't on main, it isn't shipped. No exceptions for "I tested it on a branch" or "the PR was merged." Three ghost-commit incidents in one week (#876, /pair claim, a2670bf) proved memory-based claims don't survive. See #895. | Brit (discipline) |
| 4d. Fresh-install visual smoke | Brit | For UI PRs (path-triggered): nuke + DMG + walk the changed flow per the PR's `## 4d walkthrough` section. Catches state timing, CSS positioning, and lifecycle bugs that code review and CI miss. ~5 min. Stia never runs 4d on own work (author-bias). Added after #1225/#1239/#1254 all passed Reed+CI but failed Matt's first fresh-install test. | CI bot comment (machine) + Brit (discipline) |
| 5. Manual test | Quinn | Exploratory testing on actual workflows | Brit (spawn discipline) |
| 6. Matt verification | Matt | Real usage on real data. Issue closed on merge via `Fixes #X`; Matt/Brit reopens it here if verification fails. | Issue close rule |

**Rule**: No gate is optional. A gate failure sends the change back to the appropriate earlier stage - don't push through.

### Enforcement status (as of 2026-04-19)

| Gate | Type | How it's enforced | What happens if violated |
|------|------|-------------------|-------------------------|
| Spec-first (no code without spec update) | **Mechanical (CI)** | `docs-issue-check.yml` + `issue-spec-check.yml` workflows | PR blocked, CI fails |
| Rust tests | **Mechanical (CI)** | `rust-tests.yml` - required status check | PR blocked, can't merge |
| Frontend tests | **Mechanical (CI)** | `ci.yml` - required status check | PR blocked, can't merge |
| E2E tests (Playwright Tier 1) | **Discipline** | Quinn runs before DMG, Brit asks before delivery | Process failure - missed bugs reach Matt |
| UX-flow exercise (Gate 4b) | **Discipline** | Brit exercises user flows in dev build for UI PRs before requesting label | Process failure - wiring bugs reach Matt (incident: cascade-UX PRs #885/#889 passed Reed 3x, broke in production each time) |
| Reed code review | **Mechanical (CI)** | `reed-gate.yml` - requires `reed-reviewed` label (matt-only, #627) | PR blocked; label can only be applied by Matt after reading Reed's PR comments |
| No-symptom-fixes | **Discipline** | Removed from pre-build script (#3243, PR D2), was `ROOT_CAUSE_CONFIRMED=1`, honor-system even when present | No mechanical prompt; pure honor-system |
| Build approval | **Discipline** | Removed from pre-build script (#3243, PR D2), was `BUILD_APPROVED_BY=matt`, honor-system even when present. The tag push is now the approval point. | No mechanical prompt; pure honor-system |
| Matt verification | **Discipline** | Issue close rule - closes on merge via `Fixes #X`; Matt/Brit reopens on a failed live test | Process failure - a failed fix stays closed unreported |

| Fresh-install visual smoke (Gate 4d) | **Mechanical (CI comment)** | `visual-smoke-check.yml` - posts bot comment on PRs touching `*.svelte`/`*.css`/`*.ts` in `src/`. Requires `4d: PASS` or `4d-skip: [40+ char reason]` in PR body. `4d-skip:` triggers visible ⚠️ bot comment. Not a merge blocker initially - promote if comment-only proves insufficient. | Bot comment audit trail - skips are visible, not buried |

**Why this matters**: When a gate is violated, the diagnosis is different. CI failure = fix the code. Discipline failure = fix the process (or mechanize it).

**Reed audit pass ≠ CI green ≠ ready for merge.** Brit doesn't call "ready for merge" until both Reed audit AND CI checks are green. Reed reviews code against spec; CI exercises the actual run. Both must pass before label/merge.

### Gate 4d: Fresh-install visual smoke

**Trigger:** PRs that modify files matching `src/lib/components/**/*.svelte`, `src/lib/components/**/*.css`, `src/lib/components/**/*.ts`, or `src/lib/stores/*.ts`. CI action (`visual-smoke-check.yml`) detects matching paths and posts a bot comment.

**Who:** Brit runs it. Never Stia on own work (author-bias risk). Quinn can substitute if Brit is unavailable.

**PR description requirement:** UI PRs must include a `## 4d walkthrough` section listing the specific screens and interactions to verify. For multi-issue PRs, each issue gets its own checklist item. This gives 4d an auditable script and forces pre-thought on the test path.

Example:
```markdown
## 4d walkthrough
- [ ] Smart Features picker: Skip/Back stacked vertically, Skip styled as button
- [ ] Upload Documents: X button returns to "Set Up" screen (not exit)
- [ ] SF sub-flows (OR/Anthropic/Ollama): spacing between h1 and form
- [ ] Rerank picker: dropdown stays in viewport when near bottom
```

**Procedure:**
1. Build DMG from merged main
2. Nuke - full clean install (pinned nuke command)
3. Launch with stderr capture: `pane 2>&1 | tee ~/Desktop/smoke-vX.Y.Z.txt`
4. Walk each item in the PR's `## 4d walkthrough` checklist
5. Visual check - does it look right? Any flicker, misposition, state issues?
6. Log check - `grep -i 'error\|panic\|CRITICAL' ~/Desktop/smoke-vX.Y.Z.txt`
7. Add `4d: PASS` to PR body (or `4d: FAIL - [details]`)
8. If FAIL - file or reopen the issue. Do NOT announce "ready for Matt."

**Skip mechanism:** `4d-skip: [reason, min 40 chars]` in PR body. CI bot posts a visible ⚠️ comment on the PR when it detects a skip - same pattern as Reed Gate. Skips are audit-trailed, not buried.

**Honesty rule:** No `4d: PASS` in a PR body without an actual human walk having happened. A `4d-skip:` directive is the honest alternative when the walk hasn't happened yet (or is deferred to a later, grouped walk) - it is never a shortcut to claiming `PASS` anyway. Nobody but Matt walks 4d; a skip directive naming a later walk (e.g. `4d-skip: Matt - this PR is covered by Live test #N`) must name who and which walk covers it, not simply omit the section.

**Enforcement:** CI bot comment (not merge-blocking initially). Promote to required status check if comment-only proves insufficient.

**Incident record:** 2026-05-23 - #1225 (textarea flicker), #1239 (dropdown flip direction), #1254 (agent name persistence) all passed Reed audit (PASS), CI (green), and Gate 4b (structural trace). All three failed on Matt's first fresh-install test. Gate 4d would have caught all three.

---

## 6. Reversibility & rollback

### Default to revertable

- **One logical change per PR.** Don't bundle a bug fix with a refactor with a feature.
- **Small, frequent PRs over large, rare ones.** Easier to review, easier to revert.
- **Feature flags for risky changes.** Ship behind a flag, enable when confident.

### Rollback procedure

**Full procedure:** [`docs/rollback-procedure.md`](rollback-procedure.md) - step-by-step for data-severity incidents.

- Every release tagged in git. `git revert` + rebuild is the fast rollback.
- Database changes should be reversible where practical. If not, document the forward-only nature explicitly.
- **Before shipping anything risky, know the rollback plan.** "What do we do if this breaks Matt's real data?"

### Backups before destructive changes

- Any change touching user data, schema, or file layout: confirm backup exists and is restorable before shipping.

---

## 7. Communication & documentation

### Decision log

- ADRs capture architectural decisions (in `docs/adr/`).
- Issue threads capture day-to-day decisions (this approach over that approach, etc.).
- **Never make an important decision in chat without capturing it somewhere durable.** If it matters, it goes in an ADR, issue, or spec.

### Agent handoffs

- When delegating from Brit → Stia/Reed, include: the issue number, relevant spec section path, ADR references, and acceptance criteria.
- When agent returns work, Brit confirms it meets acceptance criteria before passing to next stage.
- Agent work that lacks a clear handoff trail is re-worked, not accepted.

### Human-readable changelog

- Every release has a changelog entry describing what changed from a user's perspective.
- Lives in the repo (`CHANGELOG.md`). **CI-enforced** - PRs with source changes must update CHANGELOG.md or include `changelog-skip: [reason]`.
- Written for Matt six months from now, not for the developer who wrote the code.
- **Format:** [Keep a Changelog](https://keepachangelog.com/) - group entries under: Added, Changed, Deprecated, Removed, Fixed, Security.
- New entries go in the `[Unreleased]` section. When a versioned build ships, move unreleased entries under the version heading.
- **Skip for:** typo fixes, comment-only changes, CI/workflow updates, pure docs changes. Use `changelog-skip: [reason]` in PR description.

---

## 8. Build & release discipline

- **No release without a green test suite.** Ever. Machine-enforced via CI.
- **No DMG delivered to Matt until Reed review returns SHIP.** Stia builds → Reed reviews → SHIP ✅ → THEN announce DMG to Matt.
- **No DMG announced without a timestamped Quinn test report.** Report must exist at `~/Shared/ExampleApp/docs/test-reports/YYYY-MM-DD-quinn-vX.Y.Z.md` before announcing the build. This is mechanical: Brit checks for the file before posting the DMG link. If no report exists, the DMG is not announced - period. *(Added after discovering 3 DMGs shipped without Quinn reports: v0.59.1, v0.60.0, v0.61.0.)*
- **Quinn launches the DMG from Terminal once before producing the report.** Captures the first 30 seconds of stderr. Report must include a "Startup logs reviewed: clean" line or, if any `[CRITICAL]`/`[FATAL]`/`panicked` lines appeared, the lines themselves and the issue numbers tracking them. This is a redundant layer to ADR-0018's CI smoke check - CI catches PR-time emissions; this catches anything that slipped through (e.g., logs that fire only after release-build optimizations, or under conditions the CI fixture doesn't cover). Added after the PR #690 incident (2026-04-30) where the `[CRITICAL]` log fired in every Quinn run across v0.64.0, v0.64.1, v0.65.0 and was not noticed because Quinn's checklist didn't include log review. See `docs/audits/2026-04-30-pr690-migration-incident.md`.
- **Version numbers mean something.** Semantic versioning: major = breaking change, minor = new feature, patch = bug fix.
- **Release notes reference issues and ADRs.** The changelog is the external-facing view of what shipped.
- **DMG delivery checklist** (see Appendix B).
- **Any PR touching `src-gateway/` is not live until the Studio gateway is restarted on that SHA through `scripts/build-gateway-dmg.sh`.** A merged PR that changes the gateway and nothing else is not deployed - `build-gateway-dmg.sh` signs, notarizes, and installs in place at the path the gateway's LaunchAgent points to; nothing else updates the running binary. The live-test report for that build names the gateway SHA alongside the app version, so "what's actually running" is never inferred from the PR merge alone. (See #3121 for the specific defect this rule prevents - `pre-build.sh` rebuilding the gateway unsigned and then restarting it, which can only end in an `OS_REASON_CODESIGNING` kill.)

---

## 9. Red lines (things we never do)

- **Never push directly to main.** All changes through PR. Machine-enforced via branch protection.
- **Never leave an issue closed after a failed live test.** Issues close on merge via `Fixes #X`; a failed Matt verification must reopen it in that same test report.
- **Never skip the spec update for a behavior change.** Machine-enforced via CI.
- **Never restart the gateway without Matt's go-ahead.**
- **Never take a "quick fix" that bypasses a gate.** Speed now = pain later.
- **Never let tests stay red.** Fix or revert - no "we'll get to it."
- **Never file external issues (upstream repos, etc.) without Matt's explicit permission.**
- **One Stia run per branch, in a fresh worktree.** Push after every commit - if a run dies, the next run continues from the already-pushed commits on that branch; that is the safety net, never a reason to re-run the same branch from scratch or to open a second worktree for it.
- **Never push an empty commit to retrigger CI.** If CI needs re-running, re-run the CI job - don't manufacture a commit to do it.
- **Acceptance for an open PR is CI green on the final SHA plus Reed's PASS.** Not CI green alone, not Reed's PASS alone, and never an intermediate SHA superseded by a later push.
- **Never share GitHub credentials or tokens between human and agent accounts.** Each identity (Matt, Brit, Stia, Reed) has its own GitHub account. Agents never authenticate as Matt. Matt never gives agents his credentials. Cross-identity authentication collapses every gate that depends on "Matt approved."

  > **Why This Exists (2026-04-22):** Brit had access to Matt's GitHub account from initial setup. On 2026-04-22, Brit applied `reed-reviewed` labels to PRs #625 and #626 without Reed reviewing the actual diffs - the third such incident that day. Investigation revealed the deeper problem: Brit could perform *any* action as Matt, making every "Matt approved" gate (labels, skip directives, PR approvals) unenforceable. All tokens were revoked, accounts separated, and `reed-reviewed` label enforcement is being mechanized (#627). The credential sharing was an original sin - it made identity separation impossible from day one.

---

## 10. Cadence & review

### Weekly

- Review open issues: prioritize, close stale ones, update blocked ones.
- Brit summarizes what shipped that week and what's in flight.
- Stale issue sweep: anything untouched 30+ days gets triaged.
- Spec-vs-reality audit (Reed) - we dev too fast for monthly.
- Review ADRs: any decisions from the past week worth documenting?
- **Review this playbook:** Is anything missing or outdated? Is any rule being bypassed? If so, fix the rule or fix the behavior - don't let drift accumulate.

### Per major release

- Full spec review.
- ADR backlog sweep.
- Test coverage review - what's thin?

### Monthly

- Removed - everything moved to weekly cadence. We're dev'ing too fast for monthly cycles.

---

## 11. System audits

Run a system audit **before major work** on a subsystem (new feature sprint, architecture rewrite) and **before launch readiness review** for that system. The memory pipeline audit (Apr 21) is the reference pattern.

### What an audit produces

A gap list, GitHub issues for each gap, and optionally an ADR if the audit reveals scope changes that require architectural decisions.

### Five checks

1. **Spec coverage** - Walk every function/module in the subsystem and verify a spec section describes its behavior. Gaps = undocumented behavior that can drift.
2. **Code/spec drift** - Compare what the code actually does against what the spec says it should do. Drift = either the code is wrong or the spec is stale.
3. **Test coverage** - Identify functions with no unit test, integration test, or E2E coverage. Untested code is unverified code.
4. **INVARIANTS coverage** - Check whether the subsystem's hard constraints are captured in `docs/INVARIANTS.md`. Missing invariants = rules that exist only in agent memory.
5. **ADR coverage** - Verify that architectural decisions affecting this subsystem have accepted ADRs. Undocumented decisions get re-litigated.

### Filing pattern

- One **umbrella issue** per audit: "System Audit: [subsystem name]" with the gap list as the body.
- One **child issue** per actionable gap, linked to the umbrella.
- Audit report saved to `docs/audits/YYYY-MM-DD-[subsystem]-audit.md` (in-repo) or `~/Shared/ExampleApp/docs/audits/` (shared drive).
- If the audit reveals scope changes, draft an ADR before filing implementation issues.

### Who runs it

Decided per audit. Memory was Brit-led with Reed support; bridge audit (next) likely needs the same. Matt assigns.

---

## 12. Dependency hygiene

Refs #3106 (4.5 scope). Not required by any existing gate at the time it was added — `check-spec-first.sh`'s `SRC_CHANGED` filter only matches `^(src/|src-tauri/src/)`, and this PR's full file list (workflow YAML, `Cargo.toml`/`Cargo.lock`, `package.json`/`pnpm-lock.yaml`, a new manifest, a new script) never trips it. Documented anyway, because an undocumented ignore-list governance policy is exactly the kind of institutional-knowledge gap that causes the next person to either misuse it or be afraid to touch it.

### The gate

`.github/workflows/dependency-audit.yml` runs `cargo-audit` (RustSec) over `src-tauri/` and `src-gateway/`, and `pnpm audit --audit-level=high` over the frontend. Triggers: `pull_request` to `main` (no path filter — a newly-disclosed advisory against an unchanged lockfile must still be catchable), a weekly `schedule`, and `workflow_dispatch`. GitHub-hosted (`ubuntu-latest`) only, deliberately — a new PR-triggered job on the self-hosted macOS runner would reopen the exact #3106 C1 self-hosted-reachable-from-fork-PR risk this issue thread exists to close.

Vulnerabilities block; `cargo-audit`'s own `unmaintained`/`unsound`/`yanked` warning categories do not (no `--deny` flag is added). A numbered RUSTSEC advisory is a specific, scored exploitability claim; a maintenance-status warning is a different kind of claim that may never be hit by this crate's actual usage. Blocking on warnings would also force every one of them onto the exception manifest immediately — `unmaintained` crates don't get un-unmaintained by a version bump — which buries the signal that matters right next to real vulnerability entries. The weekly scheduled run still reports the full warnings list (non-blocking), so a currently-fine-to-not-block-on warning doesn't quietly age into something that should have been acted on.

`pnpm audit` uses `--audit-level=high`, never `--prod`. This repo's stack makes `--prod` the wrong shortcut: `svelte`/`@sveltejs/kit` are `devDependencies` by SvelteKit convention, but parts of their own runtime (e.g. `devalue`, their shared serialization helper) are bundled into every compiled client build regardless. `--prod` would correctly include genuine `dependencies` entries but would silently and permanently exclude anything shipped only through a `devDependency`'s own runtime — an invisible gap, which is worse than an honest, reviewable ignore-list entry.

### The exception manifest

Neither tool's native suppression mechanism supports a reason or an expiry: `cargo-audit`'s `audit.toml` `ignore` array is a bare list of advisory-ID strings (TOML comments exist but aren't parsed by the tool); `pnpm`'s `pnpm.auditConfig.ignoreCves` is the same shape. `docs/security/dependency-audit-exceptions.yml` is a tool-agnostic manifest instead — one entry per finding, with `id`, `package`, `scope` (`cargo-tauri` | `cargo-gateway` | `pnpm`), `reason`, `added`, and `review_by`.

`scripts/check-dependency-exceptions.sh` is what makes `review_by` actually enforce something:

- **Expiry check**, independent of either tool's own output: fails the job outright if `today > review_by` for any entry, on every PR run and the weekly schedule. A stale entry doesn't quietly keep suppressing its finding — it breaks CI until a human re-justifies it (bumps `review_by`, a diff visible in review and git blame) or removes the entry and fixes the underlying finding.
- **cargo scopes**: builds one `--ignore <ID>` flag per manifest entry in that scope and lets `cargo-audit`'s own exit code gate the job — anything not covered by an `--ignore` flag (i.e., not on the manifest) makes `cargo-audit` fail on its own.
- **pnpm scope**: `pnpm audit`'s `--audit-level` flag has no per-ID exception, so the script filters `pnpm audit --json`'s advisory list against the manifest's `pnpm`-scoped IDs itself and fails if anything above threshold isn't listed.

This is the same mechanism that makes an unlisted finding a real gate failure rather than noise: a brand-new advisory is never in the manifest, so it's never ignored, so the relevant tool's exit code goes non-zero on its own.

---

## Appendix A: What good looks like

A change from start to finish, done right:

1. Matt: "The card list needs to support keyboard navigation."
2. Brit opens issue #450, references `docs/spec/ui.md` §3.2.
3. Reed audits: scope is ChatSidebar component, touches keyboard event handling, no compatibility issues with virtual scrolling.
4. Stia creates branch `feat/450-keyboard-nav`, updates `docs/spec/ui.md` §3.2, implements keyboard navigation, writes Playwright E2E test.
5. Stia opens PR: "feat(#450): keyboard navigation for card list." CI runs - tests pass, spec check passes.
6. Reed reviews PR: code matches spec, tests cover happy path and edge cases. Approves.
7. Brit merges PR (`Fixes #450` in the PR description auto-closes the issue on merge). Quinn runs Tier 1+2 on the merged build. Pass.
8. Matt tests on real workflow. Works - issue stays closed.
9. Changelog entry: "Card list now supports keyboard navigation (↑↓ arrows, Enter to open)."

Total human touch points for Matt: verify on real workflow, reopen the issue if it fails. Everything else is agent-executed within a clearly documented, machine-enforced process.

---

## Appendix B: Matt's Build Checklist

*When Brit announces a DMG is ready, here's what to do:*

1. **Check the announcement.** It should include: version number, what changed (issue references), and DMG path.
2. **Install the DMG.** Drag to Applications, replace existing.
3. **Open Pane.** Does it launch without errors?
4. **Test what changed.** The announcement tells you what was fixed/added - try that specific thing.
5. **Spot-check your workflow.** Open a project, create a note, chat with AI, check that existing data is intact.
6. **Report issues.** If anything's wrong: describe what you did, what you expected, what happened. Screenshot if visual. Post in #bugs. **If the fixing PR used `Fixes #X` and the issue already auto-closed on merge, reopen it now, in this same report.** (#3070: PR bodies moved from `Relates to #X` to `Fixes #X`/`Part of #X` so merged PRs auto-close their issues; this reopen step is what keeps that safe.)
7. **Confirm or reject.** Tell Brit "works" or "broken - [what's wrong]." This is the final gate - reopening (step 6, when needed) plus this confirmation is what replaced the old manual-close discipline; an issue never stays closed on a fix you haven't verified.

*If the build touches database/schema/sync:* Pay extra attention to existing data. Check that old notes, tasks, and projects are all still there and correct.

---

## Appendix C: Bootstrap Architecture (Reference)

OpenClaw bootstrap loads exactly **8 hardcoded files** from the workspace directory - not a directory scan:

1. `AGENTS.md`
2. `SOUL.md`
3. `TOOLS.md`
4. `IDENTITY.md`
5. `USER.md`
6. `HEARTBEAT.md`
7. `BOOTSTRAP.md`
8. `MEMORY.md`

**The `memory/` directory is NOT loaded at bootstrap.** Files in `memory/` are indexed for `memory_search` (semantic retrieval) but never injected into the system prompt. Only the 8 files above consume bootstrap budget.

Caps (configurable in `openclaw.json` under `agents.defaults`):
- `bootstrapMaxChars`: max per file (default 12,000; currently 25,000)
- `bootstrapTotalMaxChars`: max total across all files (default 60,000; currently 120,000)

This means: optimizing bootstrap = trimming these 8 files only. The `memory/` directory size affects search quality and disk, not startup context.

*Discovered 2026-04-19 via OC source code audit (`dist/workspace-hhTlRYqM.js`, `loadWorkspaceBootstrapFiles()`).*

---

## Appendix D: Enforcement summary

What's machine-enforced (survives compaction) vs. discipline (requires agent memory):

| Rule | Enforcement | Mechanism |
|------|-------------|-----------|
| No direct push to main | **Machine** | GitHub branch protection |
| Tests must pass before merge | **Machine** | CI required status checks |
| Spec must be updated with code | **Machine** | CI workflow (`spec-first.yml`) - `spec-skip` blocked on core behavioral paths; must update `docs/spec/` |
| Changelog updated with source changes | **Machine** | CI workflow (`spec-first.yml`) - path-based restrictions on `src/`, `src-tauri/src/` |
| Reed reviews core system changes | **Machine** | CI workflow (`reed-gate.yml`) - 13 core paths (incl. `.github/workflows/`) require `reed-reviewed` label (matt-only). No skip mechanism. |
| `reed-reviewed` label is matt-only | **Machine** | CI workflow (`label-gate.yml`) - auto-removes label if applied by non-Matt |
| Identity separation (no shared credentials) | **Structural + Machine** | Separate GitHub accounts; no cross-auth; label gate enforces |
| PR links an issue | **Structural** | PR template with required fields |
| ADR checkbox on PRs | **Structural** | PR template |
| Issue closes on merge via `Fixes #X`; reopen on failed live test requires Matt/Brit | **Structural + Discipline** | GitHub native linking auto-closes on merge; reopen-on-fail is a playbook rule - agents trained |
| Quinn runs Tier 2 before DMG | **Discipline** | Brit's spawn checklist |
| Reed pre-audit before coding | **Discipline** | Brit's spawn checklist |
| Integration test for state mutations | **Mechanical (CI)** | `integration-test-gate.yml` - flags state-mutating paths without test diff; requires `integration-test-skip` label (matt-only) | PR blocked, can't merge |
| §4 integration test calls production entry point | **Machine** | CI workflow (`section-4-enforcement.yml`) - AST grep on `*_integration_test.rs`, allowlist via `// §4-helper-test:` or `// §4-untestable: #NNN` annotations |
| §4 test-the-test artifact in PR body | **Machine** | Same workflow - checks PR body for `## §4 test-the-test` section with reverted/output/restored fields populated |
| §4-untestable issue stays open | **Machine** | Same workflow - verifies linked issue exists and is open; re-flags test when issue closes |
| Startup logs free of severity-prefix lines | **Machine** | CI workflow (`startup-smoke.yml`) - launch + grep against fresh and upgrade fixtures, allowlist via `// §smoke-allowlist:` annotations |
| Upgrade fixture is current | **Machine** | Same workflow - `verify-upgrade-fixture.sh` asserts fixture's stamped version = `SCHEMA_VERSION - 1` |
| Quinn reviews startup logs before DMG report | **Discipline** | Playbook §8 rule - Quinn checklist requires "Startup logs reviewed: clean" line in report |
| One logical change per PR | **Discipline** | Code review catches violations |

> **Why This Exists - Reed Gate (2026-04-24):** Reed Gate originally allowed `reed-skip: [reason]` + `REED_SKIP_APPROVED_BY=matt` as a bypass on core paths. On PR #653, Brit proposed reed-skip on an `ai/commands.rs` threading refactor - the bypass had equal mechanical weight to Reed's actual review. #657 closes the hole by making `reed-reviewed` the only pass for core-path PRs. Non-core paths don't fire the gate at all.

> **Why This Exists - Integration Test Requirement (2026-04-28):** During the gateway implementation arc, four bugs of the same shape shipped through unit-test-passing PRs and were caught only by manual verification or careful code review. (1) Router middleware applied to wrong routes - handler unit tests passed by calling handlers directly, bypassing middleware. (2) Idempotency store first-call path - store tests passed with proper non-empty values; the handler stored an empty placeholder that subsequent calls returned as a broken record. (3) Idempotency store error path - error rollback wasn't tested; OC errors left poisoned placeholders. (4) Reconciliation conflict-module call - module tests passed; the function calling the module passed identical arguments that short-circuited every call to a no-op. Each passed component-level tests and failed integration-level outcome. The §4 test-shape rule closes this gap: PRs introducing state-mutating functions must include a test that asserts the function's side effect actually occurred end-to-end.

> **Why This Exists - Spec-Skip Path Restriction (2026-04-24):** On 2026-04-21, PRs #604 and #605 shipped behavioral changes to the AI reranker and daily logging with `spec-skip` - skipping spec updates to ship faster. Both touched core AI behavior paths (`src-tauri/src/ai/`). #611 blocks `spec-skip` on core behavioral paths: if a PR touches those files, it must update `docs/spec/`. Non-core source changes can still use `spec-skip`.

> **Why This Exists - Workflow File Gating (2026-04-24):** Reed Gate, spec-first, and changelog gates all live in `.github/workflows/`. Before #612, none of the gates protected their own files - a PR could loosen or remove a gate without triggering any review. Adding `.github/workflows/` to Reed Gate's core paths closes the self-referential gap. The gate now guards itself.

> **Why This Exists - Label Gate (2026-04-24):** On 2026-04-22, Brit applied `reed-reviewed` to PRs #625 and #626 based on Reed's pre-audit of *issues*, not Reed's review of the actual PR *diffs* - three times in one day. Root cause: identity collapse (Brit had access to Matt's GitHub account). Accounts were separated the same day, but the label itself had no enforcement on WHO applies it. #627 adds `label-gate.yml`: if anyone other than `matthewcarano` applies a protected label, it's auto-removed with a comment.

### Identity and attribution

Every action on the repo is traceable to a specific identity:

| Actor | GitHub account | Role | Can apply `reed-reviewed`? |
|-------|---------------|------|---------------------------|
| Matt | `matthewcarano` | Owner, approver, final gate | ✅ Yes (only Matt) |
| Brit | `britfreed` | Coordinator, issue management, docs | ❌ No |
| Stia | `britfreed` (pending own account) | Coder - branches, commits, PRs | ❌ No |
| Reed | `britfreed` (pending own account) | Reviewer - PR comments only | ❌ No |

Agents share `britfreed` for now (git author metadata distinguishes them). Reed and Stia get dedicated accounts post-launch. The critical boundary is: **no agent ever authenticates as Matt.** Matt's identity is the trust root for every approval gate.

> **Why This Exists (2026-04-22):** Same incident as the credential sharing red line above. When agent and owner share credentials, "Matt approved" is indistinguishable from "agent approved." Separation makes the audit trail real.

---

## Appendix E: CI Guard Catalog (G1–G6)

*Added: 2026-05-31 | G6 added: 2026-05-31 | Spec issue: #1320 | G6 issue: #1336 | Audit: `~/Shared/ExampleApp/docs/audits/2026-05-31-hardcoded-models-audit.md` § 7 | G6 audit: `~/Shared/ExampleApp/docs/audits/2026-05-31-1336-protocol-durability-audit.md` §8*

Six mechanical CI guards that prevent hardcoded model strings, sampling parameters, context-window literals, and OC protocol version literals from creeping back into the codebase. G1–G5 land in **report-only mode** (#1320) and are flipped to **fail mode** by downstream fix PRs. G6 lands directly at **fail mode** (#1336) because its violations are removed in the same PR.

Workflow file: `.github/workflows/forbid-hardcoded-constants.yml`  
Guard runner: `scripts/ci-guards/run-guards.sh`  
Unit tests: `scripts/ci-guards/tests/run-tests.sh` (14 tests: 12 for G1–G5 + 2 for G6; run as self-check step before each CI run)

### Mode flags

Each guard has a per-guard env var. Default at land: `report`. Change to `fail` in the downstream PR that cleans up that guard's violations.

| Env var | Default | Flipped to `fail` by |
|---------|---------|----------------------|
| `G1_MODE` | `report` | #1321 (streaming params fix) |
| `G2_MODE` | `report` | #1321 (streaming params fix) |
| `G3_MODE` | `report` | #1322 (remove hardcoded model defaults) |
| `G4_MODE` | `report` | Stays warn — picker seeds are intentional |
| `G5_MODE` | `report` | #1324 (context window: gateway emits contextWindow) |
| `G6_MODE` | `fail` | Landed at fail — #1336 removes all existing violations; no report window needed |

When `mode=report`: guard logs violations, exits 0, build continues.
When `mode=fail`: guard logs violations, exits 1, build fails.

### Output format

Each violation line:
```
file:line - matched literal - GUARD_ID - mode:MODE - audit:AUDIT_REF
```

Example:
```
VIOLATION: src-tauri/src/ai/openrouter.rs:417 - "temperature": 0.7, - G1 - mode:report - audit:docs/dev-hygiene-playbook.md#appendix-e-ci-guard-catalog-g1g5
```

### Guard catalog

---

#### G1 - Forbid hardcoded `temperature` literals in non-test Rust/TS source

**Problem:** Hardcoded `temperature` values (especially `0.7`) in streaming paths break Anthropic models that reject the parameter. Root cause of v0.65.71 failure against `claude-opus-4-7`.

**Pattern (Rust + TypeScript/Svelte):**
```
"temperature"\s*:\s*[0-9]      - JSON body key with numeric literal
temperature:\s*[0-9]           - struct field with numeric literal
```

**Scope:** `src-tauri/src/` (Rust) + `src/` (TS/Svelte frontend)
**Excludes:** `*_tests.rs`, `*tests*.rs`, `*test*.rs`, `target/`, `node_modules/`, `.svelte-kit/`, `dist/`, `build/`, `__tests__/`, `*.test.ts`
**Allowlist:** `// CI-ALLOW: hardcoded-sampling` on the same line (Reed must approve any addition)
**Flip to fail:** PR #1321 (streaming params)

---

#### G2 - Forbid hardcoded `max_tokens` literals in streaming-path JSON bodies

**Problem:** `"max_tokens": 4096` baked into `stream_openrouter` and `stream_anthropic` ignores `ChatRequest.max_tokens` entirely. All OpenClaw streaming responses silently truncated at 4096 tokens regardless of task.

**Pattern (Rust):**
```
"max_tokens"\s*:\s*[0-9]+      - JSON body key with integer literal
```

**Scope:** `src-tauri/src/ai/`
**Excludes:** `*_tests.rs`, `*tests*.rs`, `*test*.rs`, `target/`
**Fix path:** Streaming paths must derive `max_tokens` from `request.max_tokens`. Internal sub-calls (reranker, extraction, synthesis) use named constants (e.g., `const RERANKER_MAX_TOKENS: u32 = 100`).
**Allowlist:** `// CI-ALLOW: hardcoded-sampling` on the same line (Reed must approve)
**Flip to fail:** PR #1321 (streaming params)

---

#### G3 - Forbid model ID literals in production Rust outside `context_budget.rs`

**Problem:** Hardcoded model ID strings (`"anthropic/claude-opus-4-6"`, etc.) in production code create silent coupling to specific model versions. When a model is deprecated or changes API contract, hardcoded references break silently.

**Patterns (Rust):**
```
"anthropic/claude-[a-z0-9._-]+"    - Anthropic via OpenRouter
"claude-[a-z0-9._-]+"              - Anthropic direct
"openai/gpt-[a-z0-9._+.-]+"        - OpenAI
"google/gemini-[a-z0-9._-]+"       - Google
```

**Scope:** `src-tauri/src/`
**Excludes:** `*_tests.rs`, `*tests*.rs`, `*test*.rs`, `*context_budget*`, `target/`
**Goal:** Zero allowlists after #1322 lands (defaults removed, read from `GET /v1/agents`).
**Allowlist:** `// CI-ALLOW: hardcoded-model` on the same line (Reed must approve in audit)
**Flip to fail:** PR #1322 (remove hardcoded model defaults)

---

#### G4 - Forbid model IDs in frontend without `// PICKER-SEED:` annotation (warn)

**Problem:** New model IDs added to frontend pickers without annotation are invisible in audits. Any model string in UI must be intentionally labelled as a picker seed and centrally registered.

**Patterns (TypeScript/Svelte):**
```
anthropic/claude-[a-z0-9._-]+              - Anthropic
openai/gpt-[0-9a-z._-]+                    - OpenAI
google/gemini-[0-9a-z._-]+                  - Google
```

**Scope:** `src/`
**Excludes:** `node_modules/`, `build/`, `.svelte-kit/`, `dist/`
**Annotation (not allowlist):** Lines containing `// PICKER-SEED:` are filtered - they are intentionally labelled. Any model ID without this annotation is a violation.
**Severity:** WARN (remains in report mode permanently - picker seeds are intentional by design)
**Mode:** `G4_MODE=report` always; G4 never blocks the build

---

#### G5 — Forbid context-window integer literals outside `context_budget.rs` (warn)

**Problem:** Window sizes hardcoded outside `context_budget.rs` create two sources of truth. `context_budget.rs` is the single authoritative window table — all other code must call `context_budget::get_context_window()`.

**Patterns (Rust):**
```
\b200_000\b, \b200000\b       — 200K token window (Claude)
\b128_000\b, \b128000\b       — 128K token window (GPT-4)
\b1_000_000\b, \b1000000\b    — 1M token window (future Gemini)
\b100_000\b, \b100000\b       — 100K token window
```

**Scope:** `src-tauri/src/ai/` (excluding `context_budget.rs`)  
**Excludes:** `*_tests.rs`, `*tests*.rs`, `*test*.rs`, `context_budget.rs` (via grep filter), `target/`  
**Allowlist:** `// CI-ALLOW: hardcoded-window` on the same line (Reed must approve)  
**Flip to fail:** PR #1324 (context window: gateway emits `contextWindow`, Pane reads from `GET /v1/models`)

---

#### G6 — Forbid `minProtocol`/`maxProtocol`/`protocol` integer literals in `src-gateway/src/`

*Added: 2026-05-31 | PR #1336 | Audit: `~/Shared/ExampleApp/docs/audits/2026-05-31-1336-protocol-durability-audit.md` §8*

**Problem:** Hardcoded OC protocol version integers in the gateway's `connect` params are the root bug class that caused the v3→v4 outage (#1336). When OC bumps its protocol version, any literal in the JSON body silently locks the gateway to an old version and causes handshake rejection. **This guard covers test modules too** — a test that hardcodes `"protocol": 3` is a blind spot: when the production constant bumps, the test no longer reflects it. Matt directive (#1336): "A test that doesn't [use the constant] is a blind spot."

**Patterns (Rust):**
```
"minProtocol"\s*:\s*[0-9]    — JSON-body minProtocol with integer literal
"maxProtocol"\s*:\s*[0-9]    — JSON-body maxProtocol with integer literal
"protocol"\s*:\s*[0-9]       — JSON-body protocol field with integer literal
```

Note: Rust const definitions (`pub(crate) const OC_PROTOCOL_MIN: u32 = 3`) do NOT match these patterns because they lack the quoted JSON-key form. The regex fires only on JSON-body constructions like `serde_json::json!({ "protocol": 4 })`.

**Scope:** `src-gateway/src/` — **INCLUDING test modules** (`#[cfg(test)]` blocks, `*_test*.rs`, inline test functions). Test code is not excluded.

**Excludes:** `target/` (build artifacts) only. No test exclusions.

**Allowlist:** None. No `CI-ALLOW` tag exists for G6. Protocol version must always be expressed through `OC_PROTOCOL_MIN` / `OC_PROTOCOL_MAX` named constants. Any exception requires a new PR with Reed audit.

**Mode:** `G6_MODE=fail` at land (PR #1336). The PR removes all existing violations before flipping to fail — no report-only window.

**Named constants** (defined in `src-gateway/src/oc_client.rs`):
- `OC_PROTOCOL_MIN: u32 = 3` — lowest OC protocol the gateway speaks. Update only when OC drops v3 backwards compat.
- `OC_PROTOCOL_MAX: u32 = 99` — broad forward-compatible upper bound. Update only for wire-breaking OC protocol changes (not additive bumps).

**Durability:** With `max=99`, the guard-and-constant pair survives OC v5, v6, … without code change. The inclusive range check on OC's server (`maxProtocol >= N && minProtocol <= N`, per `protocol.md §644`) means a wide max is the documented forward-compat pattern. See `docs/spec/ai-integration.md §4A.2.3` for the full contract note.

---

### Allowlist discipline

Allowlist comments (`// CI-ALLOW: hardcoded-sampling`, `// CI-ALLOW: hardcoded-model`, `// CI-ALLOW: hardcoded-window`) suppress a single violation line in CI output. Rules:

1. **Reed must approve every allowlist addition** - the allowlist comment must appear in the PR diff and Reed's review must explicitly acknowledge it.
2. **No allowlists in the initial landing PR (#1320)** - violations report unfiltered on first run.
3. **Allowlists are tech debt** - they exist only until the downstream fix PR removes the violation. Track them in the issue.
4. **Violation of the allowlist discipline is a process failure** - treat it the same as bypassing the Reed gate.

### Expected violations at initial landing (PR #1320)

The first CI run will report the following audit-confirmed violations. These are **expected and tracked in downstream issues** - they do NOT fail the build.

| Guard | Violations | Tracked by |
|-------|-----------|------------|
| G1 | `openrouter.rs:417` (`temperature: 0.7`) + internal sub-calls with `temperature: 0.0` struct fields | #1321 |
| G2 | `openrouter.rs:416` + `anthropic.rs:395` (`"max_tokens": 4096` in streaming paths) | #1321 |
| G3 | `lib.rs:566` + `commands.rs:2951` (production model defaults) + test-inline model strings in `commands.rs` | #1322 |
| G4 | Unannotated model IDs in `ModelPicker.svelte`, `Settings.svelte`, `Onboarding.svelte` | #1322 |
| G5 | Window literals in `commands.rs` inline test functions | #1324 |
| G6 | None — #1336 removes the only violations before flipping to fail mode | #1336 (clean at land) |

*Audit source: `~/Shared/ExampleApp/docs/audits/2026-05-31-hardcoded-models-audit.md` § 7 (G1–G5) + `~/Shared/ExampleApp/docs/audits/2026-05-31-1336-protocol-durability-audit.md` §8 (G6) — All patterns are from those documents verbatim.*


---

## Appendix F: Tauri Version Parity

**Added:** 2026-05-31 | **PR:** #1318 fix branch | **Incident refs:** v0.65.71 build failure (2026-05-31), v0.65.73 build failure (2026-05-31)

Tauri spans both the Rust build system (`tauri`, `tauri-plugin-*` crates in `src-tauri/Cargo.toml`) and the NPM build toolchain (`@tauri-apps/cli`, `@tauri-apps/api`, `@tauri-apps/plugin-*` in `package.json`). A minor-version mismatch between the two sides causes the Tauri CLI to refuse to build. This appendix documents the rules that keep both sides permanently aligned.

### F1 - Tracked lockfiles (non-negotiable)

Both binary crates commit their lockfiles:

- **`src-tauri/Cargo.lock`** - tracked in git, never gitignored. Rust's own guidance: commit `Cargo.lock` for binaries. Pane is a binary; ignoring it allows crates.io to upgrade transitive Tauri crates between builds on the same commit.
- **`src-gateway/Cargo.lock`** - already tracked (confirmed tracked since PR #1330). Same rule applies.

`Cargo.lock` is the truth about what version cargo actually linked. Without it, any published minor-version bump on crates.io silently changes what a fresh checkout builds.

**Enforcement:** If a PR removes either lockfile or re-adds it to `.gitignore`, treat it as a critical regression.

### F2 - Exact-pinned Tauri CLI

`package.json` must specify `@tauri-apps/cli` with an exact version (no `^` or `~`):

```json
"@tauri-apps/cli": "2.10.0"
```

**Why:** pnpm will respect the lockfile in CI, but developer installs (`pnpm install` without `--frozen-lockfile`) honour semver ranges in `package.json`. A contaminated or fresh install with `^2.10.0` will pull `2.11.x` if it's published - and 2.11.x enforces stricter minor-version parity than 2.10.x, causing the exact failure class this appendix prevents.

**When upgrading the CLI:** bump the exact version in `package.json`, update `pnpm-lock.yaml` via `pnpm install`, commit both in the same PR as the corresponding Rust crate bump (see F5).

### F3 - Exact-pinned Tauri Rust crates

`src-tauri/Cargo.toml` must exact-pin the primary Tauri crates using the `=` prefix:

```toml
tauri = { version = "=2.11.2", features = ["devtools"] }
tauri-plugin-dialog = "=2.7.1"
```

**Why:** Even with `Cargo.lock` tracked, the minimum-version range in `Cargo.toml` means `cargo update` (or `cargo generate-lockfile` on a fresh checkout missing the lockfile) can drift forward. The `=` prefix makes drift impossible without an explicit human edit of `Cargo.toml`.

**Other `tauri-plugin-*` crates** (`global-shortcut`, `log`, `fs`, `shell`, `notification`, `updater`, `process`, `deep-link`) may use the broad `"2"` range because they are not involved in the CLI-vs-Rust minor-version parity check. If they ever become a source of drift, exact-pin them here too.

### F4 - NPM package minor-version alignment

`@tauri-apps/api` and `@tauri-apps/plugin-dialog` in `package.json` must match the **minor version** of their corresponding Rust crates:

| NPM package | Rust crate | Rule |
|---|---|---|
| `@tauri-apps/api` | `tauri` | NPM minor = Rust minor (e.g., `~2.11.0` when Rust is `=2.11.x`) |
| `@tauri-apps/plugin-dialog` | `tauri-plugin-dialog` | NPM minor = Rust minor (e.g., `~2.7.0` when Rust is `=2.7.x`) |

Use `~` (tilde) in `package.json` to allow patch updates within the pinned minor:

```json
"@tauri-apps/api": "~2.11.0",
"@tauri-apps/plugin-dialog": "~2.7.0",
```

**Why:** The Tauri 2.11.x CLI enforces minor-version parity between the Rust and NPM sides. A mismatch causes a hard build refusal.

### F5 - Rule for upgrading Tauri versions

Upgrading Tauri must be a single PR that atomically updates **all** of:

1. `src-tauri/Cargo.toml` - new exact-pinned Rust versions (`=X.Y.Z`)
2. `src-tauri/Cargo.lock` - regenerated via `cargo generate-lockfile`
3. `package.json` - new exact CLI version + updated NPM `~` minor ranges
4. `pnpm-lock.yaml` - regenerated via `pnpm update <packages>`

**Before merging a Tauri upgrade PR:** run a full `scripts/build.sh --allow-branch` locally and confirm the DMG is produced without version-mismatch errors. Do not merge on version edits alone; prove the build succeeds (Matt's directive, 2026-05-31).

**Do not update Rust and NPM in separate PRs.** A gap between the two PRs is exactly the state that caused the 2026-05-31 build failures.

### F6 - Incident history

| Date | Version | Root cause | Detection |
|---|---|---|---|
| 2026-05-31 | v0.65.71 | `src-tauri/Cargo.lock` gitignored; crates.io published `tauri 2.11.2`; dead-session `npm install` contaminated node_modules with `@tauri-apps/cli 2.11.2`; 2.11.x CLI enforced parity mismatch | Manual investigation; see `docs/investigations/2026-05-31-1298-build-drift.md` |
| 2026-05-31 | v0.65.73 | Same Cargo.lock drift recurred (gitignore still in place from pre-#1318); #1318 not yet landed | CI build failure; led to this PR |


### Test input state must match runtime binding type

When testing a Svelte handler that reads from a `bind:value` state, the test harness must set the state to the type the DOM binding produces, not the type that's most convenient.

- `bind:value` on `<input type="number">` → state is `Number` (or `''` when empty). Tests must set Number.
- `bind:value` on `<input type="text">` → state is `String`. Tests set String.
- `bind:value` on `<input type="checkbox">` → state is `Boolean`. Tests set Boolean.

Setting `state().input = '100000'` when the DOM produces `100000` (Number) hides any handler bug that depends on the runtime type (e.g., calling `.trim()` on a Number throws silently in async handlers).

This rule is mechanically enforced via typed harness setters (see `Settings.ctx-window-override.test.ts → setNumberInput`).

(Filed in response to #1434, where this gap let a TypeError escape Stage 3's 13-test grid.)

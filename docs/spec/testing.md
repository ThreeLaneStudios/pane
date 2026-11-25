# Testing & Continuous Integration

**Last Updated:** 2026-04-19  
**Source:** Extracted from pane-architecture-spec.md  
**Sections:** §25

> **Cross-references:** See [agents.md](agents.md) §24 for agent entity tests, INVARIANTS.md for architecture rules.

---

## 25. Testing & Continuous Integration
> **Status:** BUILT (v0.17.40, 2026-03-29)
> **GitHub Issue:** #293
> **Cross-references:** §24 (Agent Entities), INVARIANTS.md, AGENTS.md QA Gate Rule

Every push to `main` and every pull request triggers an automated test pipeline via GitHub Actions. No code reaches production without passing all gates. This is not optional — it is the mechanical enforcement layer that prevents architectural regressions, especially across AI agent compaction boundaries where context is lost.

### 25.1 Philosophy

Tests serve two purposes in Pane:

1. **Correctness** — Does the code do what it should?
2. **Architectural integrity** — Does the code obey the constraints we've decided on?

Purpose #2 is unusual but critical. AI agents (Stia, etc.) lose context across compaction. Decisions made in one session get undone in the next because the agent doesn't remember why. Mechanical enforcement — tests that fail when invariants are violated — is the only reliable solution. Comments get ignored. Specs get skimmed. Tests fail loudly.

### 25.2 Test Layers

#### Layer 1: Rust Unit/Integration Tests
> **Status:** BUILT — 51 tests

Backend logic: database operations, agent management, memory pipeline, note CRUD. Run with `cargo test --lib` from `src-tauri/`.

Key test files:
- `src/ai/tests/agent_tests.rs` — 17 tests for §24 (agents as first-class entities, never in projects table)
- `src/ai/tests/mod.rs` — Memory system tests (extraction, synthesis, decay)
- Standard Tauri command tests

> **#3218 note (relocation, no behavior change):** the eleven `*_tests.rs` files that
> lived directly under `src-tauri/src/ai/` (including `agent_tests.rs` and
> `recovery_tests.rs` referenced below) moved into `src-tauri/src/ai/tests/` as a
> pure file relocation — same module content, same assertions, same coverage.
> `src-tauri/src/ai/tests.rs` (the pre-existing memory-system test file) became
> `src-tauri/src/ai/tests/mod.rs` to make room for the directory. No test was
> added, removed, or altered in behavior.

Every new backend feature or bug fix gets a test. Bug fixes are test-first: reproduce the bug in a test, then fix it.

#### Layer 2: Frontend Unit Tests
> **Status:** BUILT — 30 tests

UI service logic: note creation, task management, store behavior. Run with `npm run test` (Vitest).

Key test files:
- `src/lib/services/noteService.test.ts` — Note CRUD, selection, loading
- `src/lib/services/taskService.test.ts` — Task creation, defaults, store updates

Tests mock Tauri invoke commands to isolate frontend logic from the Rust backend. Mock setup lives in `vitest-setup.ts`.

#### Layer 3: Type Safety
> **Status:** BUILT

`svelte-check` with TypeScript strict mode across the entire frontend. Catches broken imports, wrong types, missing props. Not a test suite per se, but a gate that prevents an entire class of runtime errors.

#### Layer 4: Architecture Invariant Checks
> **Status:** BUILT

Grep-based pattern matching that enforces hard rules from `docs/INVARIANTS.md`:

1. **No "Pane System" as project** — Agents use `ai_agents` table (§24). Any code referencing `"Pane System"` as a project is a violation.
2. **No unguarded `verify_system_notes_integrity()`** — Legacy function must only run for pre-§24 databases. Unguarded calls are violations.

Legitimate legacy code paths are marked with `// legacy-ok` comments. The grep excludes these markers, comments, test files, and function definitions. New invariants can be added to both `INVARIANTS.md` and this check as architecture decisions are made.

#### Layer 5: End-to-End Tests and Journeys
> **Status:** PARTIAL — Playwright specs exist and drive the real app; journeys are specified in §25.13

Playwright specs drive the real Pane app through `tauri-plugin-playwright`. They do not run on pull requests. Journeys (§25.13) are the end-to-end layer going forward.

### 25.3 CI Pipeline

**File:** `.github/workflows/ci.yml`
**Runner:** `macos-latest` (Tauri/Pane is macOS-native)
**Triggers:** Every push to `main`, every pull request to `main`

```
Steps (in order):
1. Checkout code
2. Install Rust (stable)
3. Cache Rust dependencies
4. Install Node 22 + pnpm
5. Install frontend dependencies
6. Rust tests (cargo test --lib)
7. Frontend tests (npm run test)
8. Svelte check (npx svelte-check)
9. Architecture integrity checks (grep-based invariant validation)
```

If any step fails, the run gets a red ❌ on GitHub. All steps must pass for a green ✅.

Rust dependency caching (`Swatinem/rust-cache@v2`) keeps CI runs under 5 minutes after the first build.

### 25.4 Pre-Build Script

**File:** `scripts/pre-build.sh`

Checks-only (#3243, PR D2): exactly four checks, each a pure check with no side effects:
- **Architecture invariants**, the same grep-based checks as CI.
- **Version parity**, fails if the current version's DMG already exists in `$HOME/Shared/Pane/builds`.
- **Spec check**, fails if staged `src/`/`src-tauri/src/` changes have no staged `docs/` update.
- **svelte-check**, fails on TypeScript/Svelte type errors.

The former `BUILD_APPROVED_BY`/`ROOT_CAUSE_CONFIRMED` gates, the gateway rebuild plus `launchctl kickstart` restart (#3121), and the auto-commit were deleted outright, not relocated or made conditional. The issue-comment check moved to a release-runbook step (`docs/release-runbook.md`, "Test builds" step 2): the tag push is now the approval point, not an env var.

This script runs locally before `scripts/build.sh` (the only sanctioned path to `tauri build`; see `docs/INVARIANTS.md`). CI runs a subset (the architecture checks) on every push.

### 25.5 INVARIANTS.md

**File:** `docs/INVARIANTS.md`

Human-readable hard constraints that must never be violated. Lives in the repo so it's version-controlled and visible to all agents. Also copied to agent workspaces so it survives compaction.

Current invariants:
- NEVER create "Pane System" as a project
- NEVER write agent data to `projects` table
- NEVER call `verify_system_notes_integrity()` when `ai_agents` table exists
- NEVER modify `ai_memory` table structure without Matt's approval
- NEVER change memory extraction/synthesis/decay as a side effect of other work

New architectural decisions that create hard constraints should be added here. The CI pipeline (§25.3) and pre-build script (§25.4) mechanically enforce them.

### 25.6 Rules for New Code

1. **Every bug fix starts with a test** that reproduces the bug. Fix the test, then ship.
2. **Every new UI flow gets a matching test** — at minimum, a Vitest test for the store/service behavior.
3. **Every new architectural constraint gets added to INVARIANTS.md** and the CI grep checks.
4. **Tests must pass before any build ships.** No exceptions. No "we'll fix the tests later."
5. **Legacy code paths get `// legacy-ok` markers** with a comment explaining why they're exempt. These markers are the only way to exclude code from invariant checks.


### 25.7 Reed Architecture Audit
> **Status:** BUILT (process, not automated)
> **Template:** `docs/reed-architecture-audit.md`
> **Audits saved to:** `docs/audits/`

Before any build touching core systems (agent architecture, memory pipeline, sync, onboarding), spawn Reed (researcher agent) to review:
1. Relevant spec sections
2. INVARIANTS.md
3. The proposed change

Reed outputs PASS / FAIL / WARN with specific spec references. This catches semantic violations that grep patterns can't — e.g., a change that technically doesn't mention "Pane System" but creates equivalent behavior through a different path.

Not automated in CI (requires AI inference). Run manually as part of the build review process.

### 25.8 Database Migration Tests
> **Status:** NOT BUILT — Required before v1.0 release
> **Priority:** High — data corruption on upgrade is catastrophic for paying customers

Test every migration path:
- **Fresh install** — empty DB → latest schema. All tables, indices, defaults correct.
- **Sequential upgrade** — v1.0 → v1.1 → v1.2. Each migration applies cleanly.
- **Skip upgrade** — v1.0 → v1.3 (user skips versions). All intermediate migrations apply.
- **Data preservation** — seed a DB with notes, tasks, memories, agents, chat history. Run migration. Verify all data survives with correct values.
- **Rollback safety** — if a migration fails mid-way, DB is not left in a corrupted state.

Implementation: Rust integration tests with pre-seeded SQLite databases for each version. Run in CI alongside existing tests.

### 25.9 Memory Pipeline End-to-End Tests
> **Status:** NOT BUILT — Required before v1.0 release
> **Priority:** High — memory is Pane's core differentiator

Test the full pipeline from chat message to searchable memory:
1. **Extraction** — Send a chat message containing a fact → verify `ai_memory` row created with correct category, source_message_id dedup
2. **Synthesis** — Trigger synthesis after N messages → verify prose Memory note written to correct project
3. **Flat file write-back** — After synthesis, verify `.pane/system/{agent}/Memory.md` updated on disk
4. **Search** — After extraction, verify `search_memory` returns the fact via FTS
5. **Decay** — Create a memory with old timestamp → run decay → verify soft-deleted based on category rules (§5.6)
6. **Content loss guard** — Verify synthesis rewrite >50% is blocked (v11.28.23 guard)

Current unit tests cover individual pieces. This tests the full chain. A broken handoff between steps (e.g., extraction works but synthesis silently skips) would go undetected without E2E coverage.

### 25.10 Sync Conflict Tests
> **Status:** NOT BUILT — Required before multi-device sync ships as a feature
> **Priority:** Medium — required if sync is a launch feature, deferrable if "coming soon"

Test cr-sqlite CRDT conflict resolution:
1. **Concurrent note edits** — Two devices edit same note offline → sync → verify merge (both changes preserved or last-write-wins as designed)
2. **Agent + user collision** — AI writes to daily log while user edits same note → verify no data loss
3. **Delete + edit conflict** — Device A deletes a note, Device B edits it → sync → verify correct resolution
4. **Tag conflicts** — Both devices add different tags to same note → sync → verify union merge (§24.7 pattern)
5. **Agent data sync** — `ai_agents` table changes on one device → verify propagation
6. **Egress regression** — Verify `pull_from_queue` with device_id filter doesn't re-download full changeset history (v11.28.69 fix)

### 25.11 Performance Benchmarks
> **Status:** NOT BUILT — Not required for v1.0, important at scale
> **Priority:** Low — build when user base exceeds ~100 or performance complaints arise

Baseline measurements to track regressions:
- **DB query times** — note list, task list, memory search (target: <50ms for 10K notes)
- **FTS search latency** — full-text search across notes + memory + chat (target: <100ms)
- **Memory extraction speed** — time from chat message to `ai_memory` row (target: <2s)
- **Sync round-trip** — time from local change to remote device receipt
- **App launch time** — cold start to interactive UI (target: <3s)
- **Semantic search** — embedding generation + similarity query (target: <500ms)

Implementation: Criterion benchmarks for Rust, tracked in CI with regression alerts.

### 25.12 Integration & Destructive Tests
> **Status:** NOT BUILT
> **Priority:** High — Required for v1.0 release
> **Cross-references:** §24.0 (startup sequence), §24.1 (integrity check), §5.6 (daily logs), §5.3 (storage model)
> **GitHub Issue:** #271

These tests verify Pane's disaster recovery and self-healing capabilities. They run on every push alongside unit tests (Linux, no macOS required) and validate that data can be reconstituted from flat files after catastrophic DB loss, and that the integrity check (`verify_agents_integrity()`) correctly heals missing or damaged system files.

**Location:** `src-tauri/src/ai/tests/recovery_tests.rs` (included via `#[cfg(test)] mod recovery_tests;` in `src-tauri/src/ai/tests/mod.rs`, moved from `src-tauri/src/ai/` directly in #3218)

**Test Pattern:** Each test creates a temporary directory simulating `~/Pane/.pane/`, sets up the scenario (populate DB + flat files), then destroys part of it and verifies recovery. Tests use in-memory SQLite databases for speed and isolation.

---

#### Test 1: DB Wipe → Agent Reconstitution
**Function being tested:** `verify_agents_integrity()` (§24.1)

**Setup:**
1. Create temp directory with `.pane/system/Brit Freed/` structure
2. Write `SOUL.md` (with sample personality content) and `MEMORY.md` (with sample memories) to disk
3. Create in-memory SQLite database with full schema (from migrations)
4. Insert agent record into `ai_agents` table:
   - `name = "Brit Freed"`
   - `workspace_path = "Brit Freed"`
   - `is_main = 1`
   - `is_active = 1`

**Destructive Action:**
1. Drop `ai_agents` table: `DROP TABLE ai_agents;`
2. Recreate table with empty schema
3. Run `verify_agents_integrity(conn)`

**Verification (PASS criteria):**
- [ ] Agent record exists in `ai_agents` table with correct name
- [ ] `workspace_path = "Brit Freed"` (matches folder name)
- [ ] `is_main = 1` (main flag restored correctly — agent has `logs/` directory)
- [ ] `is_active = 1` (agent is active by default)
- [ ] `source = 'filesystem'` (indicates reconstitution from disk)

**What this tests:** The ability to rebuild the agent registry from flat files after total DB loss. Critical for disaster recovery.

---

#### Test 2: DB Wipe → Daily Log Reconstitution
**Function being tested:** `verify_agents_integrity()` (§24.1), daily log import logic

**Setup:**
1. Create temp directory with `.pane/system/Brit Freed/logs/` structure
2. Write two daily log files:
   - `logs/2026-03-28.md` (200 words of sample log content)
   - `logs/2026-03-29.md` (150 words of sample log content)
3. Create in-memory database with agent record
4. Populate `notes` table with records for both logs (correct `agent_id`, `note_type='daily'`, `file_path`)

**Destructive Action:**
1. Delete all records from `notes` table: `DELETE FROM notes;`
2. Run `verify_agents_integrity(conn)`

**Verification (PASS criteria):**
- [ ] Two records exist in `notes` table with `note_type = 'daily'`
- [ ] `file_path` matches flat file location: `.pane/system/Brit Freed/logs/2026-03-28.md` and `.../2026-03-29.md`
- [ ] `agent_id` correctly references the Brit Freed agent
- [ ] `content_preview` populated with first 280 chars of file content
- [ ] `created_at` matches file modification date (or defaults to "now" if unavailable)

**What this tests:** Daily logs can be rebuilt from flat files after notes table corruption. Ensures no loss of agent memory across restarts.

---

#### Test 3: Self-Healing → Missing System File
**Function being tested:** `verify_agents_integrity()` (§24.1), file creation logic

**Setup:**
1. Create temp directory with `.pane/system/Brit Freed/` structure
2. Write `SOUL.md` and `MEMORY.md` to disk
3. Create database with agent record
4. Verify both files exist on disk

**Destructive Action:**
1. Delete `SOUL.md` from disk: `fs::remove_file(soul_path)`
2. Run `verify_agents_integrity(conn)`

**Verification (PASS criteria):**
- [ ] `SOUL.md` file recreated at `.pane/system/Brit Freed/SOUL.md`
- [ ] File contains default placeholder content: `# SOUL.md\n\n**Agent:** Brit Freed\n\n*(This file was auto-created by integrity check...)*`
- [ ] File is readable and not corrupted (attempt to read and parse)

**Stretch goal (if DB stores soul content):** If agent record has `soul_content` column populated, verify file is recreated from DB content instead of default placeholder.

**What this tests:** Missing system files are auto-healed on startup. Prevents app crashes from accidental file deletion or sync conflicts that leave gaps.

---

#### Test 4: Config Persistence Across DB Reconnect
**Function being tested:** `ai_settings` table persistence (§24.x AI configuration storage)

**Setup:**
1. Create in-memory database with `ai_settings` table:
   ```sql
   CREATE TABLE ai_settings (
       key TEXT PRIMARY KEY,
       value TEXT NOT NULL
   );
   ```
2. Insert three config values:
   - `('provider', 'openclaw')`
   - `('endpoint', 'http://localhost:8000')`
   - `('api_key', 'test-key-12345')`

**Destructive Action:**
1. Close the database connection: `drop(conn);`
2. Reopen connection to same in-memory database (use named in-memory: `file::memory:?cache=shared`)
3. Query all settings: `SELECT key, value FROM ai_settings;`

**Verification (PASS criteria):**
- [ ] All three key-value pairs returned correctly
- [ ] Values match exactly (no truncation, encoding issues, or data loss)
- [ ] Query succeeds without errors

**What this tests:** AI configuration survives app restarts and doesn't silently revert to defaults. Addresses bug #287 (Test Gateway doesn't persist).

---

#### Test 5: No Bad Folder Structures
**Function being tested:** `verify_agents_integrity()` (§24.1), folder cleanup logic

**Setup:**
1. Create temp directory with `.pane/system/` structure
2. Create valid agent: `Brit Freed/` with SOUL.md + MEMORY.md + logs/
3. Create BAD structures that should NOT exist:
   - `.pane/system/Main Agent/` (legacy naming, §24 violation)
   - `.pane/tasks/` (legacy folder from pre-§24 architecture)
4. Create database with agent record for Brit Freed
5. Insert BAD record into `projects` table: `('pane-system', 'Pane System', is_system=1)` — agents should NEVER be in projects table per §24

**Destructive Action:**
1. Run `verify_agents_integrity(conn)`

**Verification (PASS criteria):**
- [ ] `.pane/system/Main Agent/` folder **does not exist** after cleanup
- [ ] `.pane/tasks/` folder **does not exist** after cleanup (Bug #288 fix)
- [ ] No record in `projects` table with `name = 'Pane System'`
- [ ] No record in `ai_agents` table has `project_id` column (this column shouldn't even exist per §24.2 schema)
- [ ] `Brit Freed/` folder still exists and is intact

**What this tests:** Legacy architecture violations are cleaned up on startup. Prevents regressions where old code paths recreate deprecated structures.

---

#### Coverage Gaps (Explicitly Out of Scope)

These integration tests focus on **unit-testable recovery scenarios**. The following are explicitly NOT covered here and are addressed elsewhere:

1. **Full app launch → Manual QA / Quinn E2E (#302)**
   - Integration tests verify backend logic in isolation
   - Full Tauri app launch with UI requires Playwright E2E tests (Quinn's domain)
   - Rationale: Pane is macOS-native; CI runs Linux. Headed browser tests are expensive.

2. **Gateway sync round-trip → Infrastructure testing, can't mock meaningfully**
   - Sync logic requires real OpenClaw Gateway + network connectivity
   - Integration tests can verify DB egress queue population, but not full round-trip
   - Rationale: Too many external dependencies for CI; manual testing before releases

3. **Network behavior → Manual testing before releases**
   - HTTP connectivity, DNS resolution, network edge cases
   - Rationale: Requires real network infrastructure, not simulatable in CI

4. **Concurrent writes → Edge case, future hardening**
   - Race conditions from simultaneous UI + AI writes
   - Rationale: Requires multi-threaded test harness, low priority until multi-device sync ships

5. **macOS-specific behavior → Only testable on macOS runners (cost prohibitive for CI)**
   - File system case-sensitivity differences (APFS vs ext4)
   - macOS security prompts, keychain integration
   - Rationale: GitHub macOS runners are 10x cost of Linux; use for builds, not tests

6. **Onboarding UI flows → Quinn's Playwright E2E tests (#302)**
   - First-run experience, settings panels, user interactions
   - Rationale: UI testing is E2E domain; integration tests focus on backend/recovery logic

---

#### Implementation Checklist

When implementing `src-tauri/src/ai/tests/recovery_tests.rs`:

- [ ] Use `tempfile::TempDir` for isolated test directories (same pattern as `agent_tests.rs`)
- [ ] Use `Connection::open_in_memory()` for SQLite (fast, no disk I/O)
- [ ] Use named in-memory DB (`file::memory:?cache=shared`) for Test 4 (persistence across connections)
- [ ] Create helper function `setup_recovery_test_env() -> (TempDir, Connection)` to reduce boilerplate
- [ ] Seed DB with full schema from migrations (copy from `agent_tests.rs::setup_agent_db()`)
- [ ] Write sample flat files with realistic content (don't use empty files — test content extraction)
- [ ] Each test should be `#[test]` annotated and run independently (no shared state)
- [ ] Use descriptive test names: `test_db_wipe_agent_reconstitution`, `test_daily_log_reconstitution`, etc.
- [ ] Add to CI pipeline in `.github/workflows/ci.yml` (runs automatically with `cargo test`)

---

### 25.13 Journeys
> **Status:** PLANNED
> **GitHub Issue:** #470
> **Cross-references:** §25.2 Layer 5, `e2e/`, `release-publish.yml`

A journey is a script that clicks through the real Pane app, one how-to at a time (for example, "Create an agent"). Each journey has two outputs from the same script:

1. **A test** — every step either works or fails.
2. **A docs page** — one screenshot and one caption per step, published to paneapp.ai/docs.

Because the docs page is generated from a script that must pass, a page cannot describe a flow that no longer works.

#### Tool

Journeys use the Playwright bridge already in the repo (`tauri-plugin-playwright` with `@srsholmes/tauri-playwright`). It drives the real app and already exposes `screenshot()`. Switching tools would add a new dependency and a second debug-only plugin for no gain. If screenshots fail against the app, the fallback is upgrading the client, not changing tools.

#### Where journeys run

- GitHub-hosted Linux runners, under a virtual display.
- Against the test fixture only (`PANE_TEST_ROOT`, `PANE_TEST_DB`, `PANE_TEST_MODE`): a throwaway Pane folder, never real user data.
- Against a mock Ollama endpoint. No real model calls, no tokens, no provider keys.

Nothing at run time depends on an AI model or a scheduled job.

#### When journeys run

| Trigger | What it does |
| --- | --- |
| Bump PR | Runs every journey as a test and posts a pass or fail list, before Matt's live test. No screenshots are kept. |
| Public release tag (`vX.Y.Z`) | Runs every journey once against the released build, saves the screenshots, and rebuilds the docs pages. |
| Manual (`workflow_dispatch`) | Runs on demand. |

Journeys do not run on ordinary pull requests. A journey broken by an ordinary PR surfaces at the next bump. Dev prereleases do not rebuild the docs.

#### Rules for writing a journey

1. **One journey per how-to.** Name it for what the user is trying to do.
2. **Every step has an action, a caption, and a screenshot.** Captions live in the journey file, next to the step they describe.
3. **Captions are written for the end user**, in plain language. Matt approves the captions of the first journey; later journeys follow that pattern.
4. **Select elements by `data-testid`.** Adding one to a core-behavioral file (for example `Onboarding.svelte` or `Settings.svelte`) needs a spec touch, as any change to those files does.
5. **Paid providers are captured up to the setup screens only.** A journey never needs a real key.
6. **A PR that changes a covered flow updates its journey in the same PR.**

#### New features

A feature's spec names the journey that covers it (the `Journey:` field in the spec template). A user-facing feature is not complete until that journey exists.

#### Docs build

A template turns each journey's steps into a docs page. On a public release tag, a workflow rebuilds the pages and opens a pull request against the `release` branch of the site repo (`paneapp-ai`). Matt merges it, and merging deploys the site. The workflow can open pull requests but cannot merge.

#### When a journey fails

- A step is retried, and a whole journey is retried once.
- At a bump, the failure is reported in the pass or fail list.
- At release, a failing journey affects only its own page. That page stays at its last good version, the rest of the docs still rebuild, and the pull request says which page was skipped.

#### Shipped-build safety

The Playwright plugin is registered only under `#[cfg(debug_assertions)]` (`lib.rs`). `release-publish.yml` scans the release binary for the plugin's symbols and fails the release if any are present, so the test hooks cannot reach a shipped build.

#### Not covered yet

Video walkthroughs, bots that explore the app freely, and a tool that lets an agent drive the app directly.

---

### Test Isolation Requirements

Tests sharing global state (e.g., `OnceLock<Mutex<...>>` singletons) must be serialized with `#[serial]` from the `serial_test` crate. Parallel test execution with shared mutable state causes mutex poisoning and non-deterministic failures.

**Current serialized test groups:**
- Image cache tests (`ai/chat.rs`) — 4 tests sharing `IMAGE_CACHE` static
- Backup/restore tests (`backup.rs`, `test_backup.rs`) — `#[serial(pane_test_root)]` group; all tests use `MockKeychain` (never the real macOS keychain) and set `PANE_TEST_ROOT` to a per-test `TempDir` (never the real `~/Pane`). RAII `TestRootGuard` handles env-var cleanup on panic. (#2903)

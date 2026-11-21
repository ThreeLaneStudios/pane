# Code Review Checklist

**READ THIS BEFORE EVERY CODE REVIEW.** This is mandatory, not optional.

This is Gate 4 in the [Dev Hygiene Playbook](dev-hygiene-playbook.md) §5 verification pipeline. Reed reads this before every post-build review.

This checklist exists because reviews that only check "is the new code correct in isolation" miss bugs that come from how the rest of the system interacts with it. The v0.26.0/v0.26.1 model header bug was caused by reviewing the plumbing but not the data flowing through it.

---

## 1. Trace the Full Data Path (What Does This Function Expect?)

For every changed function, answer:
- [ ] Where does each INPUT originate? (Database? User input? Config file? Another function?)
- [ ] What ACTUAL VALUES can each input have? Not hypothetical — check the real database/config.
- [ ] What transformations happen between the origin and this function?
- [ ] Does the function handle ALL possible input values, not just the ones the developer imagined?

**How:** `grep` for the function name. Read the function signature. Trace each parameter back to its source.

## 2. Check the Database

- [ ] What values are ACTUALLY stored in the relevant tables/settings right now?
- [ ] If the change depends on a DB value, query it: `SELECT * FROM ai_settings WHERE key = 'model'`
- [ ] Does the change work with EXISTING data, or only with freshly-inserted data?
- [ ] If a new column or value format is expected, is there a migration for existing data?

**How:** Read `docs/canonical-schema.md` (pre-launch) or migration files. Check `get_ai_setting` / `set_ai_setting` calls. Understand the schema.

## 3. Test Through the Full Code Path

- [ ] If the feature involves a UI flow, trace from UI event → Svelte → Tauri command → Rust function → network request
- [ ] Don't just test with curl — test with the same code path the user triggers
- [ ] Prefer running the app. Only fall back to manual trace if the app environment is unavailable, and flag this as a review limitation.
- [ ] **Tests exercise function output, not just precondition state.** Tests that assert on DB state, queue insertion, or other inputs without calling the function under test fail this check. Per playbook §1 intent-vs-execution discipline. (Added after PRs #1132, #1210, #1211 — same pattern caught three times in 24 hours.)

## 4. Check All Callers (What Do They Actually Send?)

Section 1 asks "what does this function expect?" This section asks "what do callers actually pass?" The gap between these two is where bugs live.

- [ ] Who calls the changed function?
- [ ] What do they ACTUALLY pass? (Not what the type signature says — what values at runtime?)
- [ ] Are there multiple code paths that reach this function? (e.g., chat vs background tasks vs compaction)
- [ ] Does the change behave correctly for ALL callers, not just the primary one?

**How:** `grep -rn "function_name" src-tauri/src/` and read every match. Check what each caller constructs before the call.

### 4a. Re-Score Carried-Forward WARNs (mandatory for PRs closing an issue with a prior scoping audit)

- [ ] Does this PR's own diff introduce a NEW caller that reads a field/path a prior WARN
      flagged as unread/unused?
- [ ] For every WARN inherited from a prior audit (scoping audit or earlier PR audit) and
      marked "non-blocking": re-trace it against every new code path THIS PR adds, not just
      the path where it was originally observed.
- [ ] If a carried-forward WARN's blast radius grows because of this PR's new wiring,
      upgrade its severity accordingly (WARN → BLOCK) before shipping, or file a same-day
      follow-up issue if merge is time-sensitive.

**Why this exists:** #2419/#2421 (2026-08-04/05). PR #2421's delta-audit correctly identified
and correctly scoped WARN 1 (`NoteSearchResult.id` vs Rust `note_id` mismatch) against the
picker-insertion path that existed at review time — "non-blocking, picker only reads `.title`."
But the same PR shipped a NEW click-navigation path (`handleWikiLinkNavigation` →
`navigateWikiLink` → `api.searchNotes` → `noteService.selectNote` → `get_note`) that reads
`.id`/`.slug` from the identical mismatched result shape. The WARN went live in production
(v0.68.89) as a P0 click-through failure — `get_note missing required key noteId`, phantom
editor, silently discarded keystrokes — one day after a PASS verdict. The review checked the
WARN against the path it was found on, not against the paths the PR itself was adding in the
same diff.

## 5. Edge Cases from Real State

- [ ] What's in the user's current database/config?
- [ ] Does the change work for: fresh install, existing user, user who changed settings, user who never changed settings?
- [ ] What happens if the relevant setting is empty/null/missing?
- [ ] What happens if the setting contains an unexpected format?

## 6. No Silent Failures

- [ ] If an error occurs, does the user see a meaningful message?
- [ ] Are errors logged with enough context to debug?
- [ ] Does the code fail loudly or silently swallow errors?

## 7. Backward Compatibility

- [ ] Does this change break existing data or behavior?
- [ ] If a user doesn't update their settings, does everything still work?
- [ ] If the gateway/provider changes, does this degrade gracefully?

### 7a. Deployment Scope (mandatory for bug fixes and schema changes)

**If this fixes a bug, does it apply to existing deployments on upgrade?**

Identify WHERE the fix lives:
- `init_schema()` changes → **new installs only** (existing users won't benefit)
- Numbered migrations → **existing users on upgrade** (runs once on next app launch)
- Runtime code fixes → **immediate for all users** (no migration needed)

Then answer:
1. WHO currently has this bug? (new users? existing users? both?)
2. Does the fix reach ALL affected users, or only a subset?
3. If the fix is init-only, does a companion migration exist for existing installs?

If #3 is "no" → **FAIL the review.** A fix that only helps new installs when existing users have the same bug is incomplete.

**Why this exists:** PR #547 (FTS5 triggers, 2026-04-20) passed review because the code was architecturally correct — trigger ordering after CRR is sound. But it only fixed fresh installs. Every existing user still had dead FTS triggers. The reviewer checked implementation correctness, not problem coverage. "Architecturally correct" ≠ "problem solved."

## 8. Scope Verification

- [ ] Do ALL changed files belong to the issue referenced in the PR title/description?
- [ ] Are there any files in the diff that aren't mentioned in the PR description or don't relate to the stated fix?
- [ ] If the PR touches files outside its stated scope, is there a documented reason (shared code, necessary refactor)?
- [ ] Does the PR title accurately describe what actually ships?

**How:** Read `git diff --name-only` against the PR description. Every file should trace back to the stated issue. Flag anything that doesn't belong.

> **Why This Exists (2026-04-22):** PR #625 was titled "daily log guard (#617)" but also shipped ChatSidebar.svelte changes from #618 (extraction ID capture). Reed reviewed and passed it without catching the out-of-scope frontend changes. The contamination came from a cherry-pick that carried unrelated work — scope verification would have caught it.

### 8a. The standing no-lock-changes rule — and its one, narrow carve-out

**The rule:** `src-tauri/Cargo.lock` and `src-gateway/Cargo.lock` diffs that are not the PR's own stated subject are out of scope under §8 and get reverted before merge — even when the content is benign (e.g. a stray version-sync line from a local `cargo check`/`cargo test` run picking up unrelated drift on `main`). Precedent: `bb37d688` ("benign content… but the standing rule has no exceptions"), and the same pattern in #3040/#3048/#3055. This rule stays — most PRs have no business touching a lockfile, and an undisciplined lockfile is exactly how dependency drift sneaks onto `main` unreviewed.

**The carve-out (added by #3056):** A **release-bump PR** — one whose stated purpose is raising `package.json`/`src-tauri/Cargo.toml`/`src-tauri/tauri.conf.json` (or, for a gateway-only bump, `src-gateway/Cargo.toml`) to a new version — is NOT a §8 violation when its `Cargo.lock` diff is confined to that same package's own `version = "..."` line. **This is the one PR type where the lock change is required, not incidental:** `scripts/check-version-parity.sh` (app) and `scripts/check-gateway-version-bump.sh` (gateway) both fail CI if the manifest and lock disagree, so a bump PR that reverts its own lock diff under §8 breaks the very gate §8 exists to keep honest.

**Why the carve-out must be this narrow, not "lockfiles are fine in bump PRs":** verify the lock diff before approving — `git diff --numstat -- src-tauri/Cargo.lock` (or the gateway path) must show exactly one changed line, the package's own version field. Any dependency resolution churn beyond that one line is still a §8 violation in a bump PR and must be investigated (did `cargo check` pull in an unrelated upstream update?) before merge, not waved through because "it's the bump PR."

**Root cause this closes:** #3048's bump PR raised the three app manifests; its `Cargo.lock` diff (the matching one-line version bump) was reverted under §8 with no exception on record, because none existed. Nothing in CI noticed the resulting mismatch — #3056 added that gate. An undocumented exception is exactly how the original gap happened; this subsection is the fix.

## 9. Spec Alignment

This was the root of the v0.26.0 bug — the code was "correct" but didn't match what the system actually needed. Same pattern as the §7a failure (PR #547, 2026-04-20): review said "correct" but wrong in context. Both are instances of reviewing the implementation, not the problem space.

- [ ] Does the spec describe what the code does?
- [ ] Does the code match the spec's intent?
- [ ] If they disagree, spec is authoritative until updated (per Dev Hygiene Playbook §1).
- [ ] If the spec needs updating, is that update in this PR?

**How:** Find the relevant file in `docs/spec/`. Read the section. Compare to the code.

## 10. Concurrency & Ordering

- [ ] Does this change assume serial execution?
- [ ] What happens if another task runs between step A and step B?
- [ ] Are there shared resources (DB, queues, files) that could be touched concurrently?
- [ ] If async, what happens if this fires during compaction / sync / startup?

## 11. Observability

- [ ] If this fails in production, what logs/telemetry will tell us why?
- [ ] Are error paths logged with enough context to debug?
- [ ] Does this change add or remove any existing logging? Why?

## 12. External-System Contract Verification

- [ ] For every external call the PR introduces or modifies (third-party API, sibling service, OS interface, MCP server, OpenClaw RPC, BYOK provider): does the spec cite a captured response shape?
- [ ] Does the code match that captured shape exactly (field names, types, nesting)?
- [ ] Is the capture's version current? If the external system has versioned past the captured version, flag for re-capture before merge.
- [ ] Does the capture include: exact response body, source path, external-system version, capture date, and versioning note (stable/breaking-on-major/unstable)?
- [ ] Does the code reference the spec capture (comment or doc link), not just implement against it?

**Why this exists:** ADR-0023. The §826 audit found 8 of 11 OpenClaw RPC methods had field-name or shape mismatches because the spec invented names without capturing OC's actual response shapes. Three independent conventions resulted (spec names, code names, OC's actual names); none matched. This check prevents recurrence by requiring captured-shape-to-code traceability for every external integration.

---

## Review Output Format

Every review must include:

1. **Data path trace** — "Input X comes from [source], has value [actual value], passes through [transformations]"
2. **Caller analysis** — "Function is called from N places: [list with what each passes]"
3. **Real state check** — "Current DB/config value is [X], change handles this: yes/no"
4. **Spec check** — "Relevant spec: [file §section]. Code matches: yes/no"
5. **Verdict** — SHIP / WARN / BLOCK with specific reasons
   - **SHIP** = change is correct and ready to merge
   - **WARN** = change is correct but exposes follow-up work; ship with issue filed
   - **BLOCK** = change has defects or unresolved questions; do not merge

---

## Why This Exists

v0.26.0 review checked that `openrouter.rs` correctly adds the `x-openclaw-model` header. It does. But nobody asked "what value does Pane actually store as the model?" The answer was `openclaw:main` — an agent target, not a provider model. The new code treated it as a provider model and broke everything.

The review was correct about the code it examined. It was wrong about the system because it didn't trace the input.

**Never review a function without understanding its inputs.**

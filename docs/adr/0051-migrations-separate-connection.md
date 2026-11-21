# ADR-0051: Migrations Run on a Separate Short-Lived Connection

**Status:** Proposed
**Date:** 2026-07-05
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None (this ADR changes the connection lifecycle documented in §2B.6; §2B.6 is amended, not superseded, because its other content — the #454 double-init history and the `open_connection` / `init_database` split — is preserved)
**Supplements:** ADR-0017 (PR#690 migration fix — established per-version-pair migration model; this ADR completes the connection-lifecycle question ADR-0017 left as "no code change"), ADR-0018 (CI smoke check for `[CRITICAL]` startup logs — the detection layer that surfaced this bug), ADR-0019 (Pre-launch schema management clarification — the per-version-pair model this ADR preserves)
**Related:** `docs/spec/file-system.md` §2B.1 (pre-migration backup, unchanged), §2B.2 (graceful fallthrough on migration failure, unchanged), §2B.6 (Database Initialization Pattern — amended by this ADR), `src-tauri/src/database.rs::init_database` (changed by this ADR's implementation), `src-tauri/src/migrations.rs::run_migrations` (unchanged), `docs/audits/2026-07-05-trigger-stmt-research.md` (Reed research)

## Context

On v0.67.94, note and task creation fail on any database that upgrades across a schema bump. The exact error surfaces on the first `INSERT INTO notes` after startup:

> `unable to reset cached trigger stmt`

The failure is invisible on fresh installs and appears only on upgrade paths. The bug entered the codebase as an unavoidable consequence of the current connection-lifecycle pattern; it is not a defect in any specific migration.

**Mechanism (Reed research, verified against SQLite documentation).** SQLite Connection objects cache compiled VDBE (virtual database engine) programs for prepared statements, including the programs used by FTS5 triggers such as `notes_fts_insert`. Each cached program is stamped with the `schema_cookie` — SQLite's monotonic counter of schema changes — at the moment it was compiled. When a program is invoked, SQLite validates that the connection's current `schema_cookie` matches the program's stamp; if the cookie has advanced since the program was compiled, SQLite raises "unable to reset cached trigger stmt."

The `schema_cookie` advances on any DDL that changes the database schema — including `CREATE TABLE`, `ALTER TABLE`, `CREATE INDEX`, `CREATE TRIGGER`. Migration #1908 (v68→v69) issued a `CREATE TABLE og_cache` inside `run_migrations()`, bumping the cookie on the connection that had already cached FTS5 trigger programs. The next `INSERT INTO notes` on that same connection invoked the cached trigger → cookie mismatch → error.

**Why fresh installs don't hit this.** On fresh install, `run_migrations()` runs from v0 → current SCHEMA_VERSION on a connection that has never executed anything. No prepared statements are cached. When the first application-time INSERT fires the FTS trigger, the trigger's VDBE program is compiled and cached with the final post-migration `schema_cookie`. No mismatch is possible.

**Live source verified against SHA at 2026-07-05.** From `docs/spec/file-system.md` §2B.6 (BUILT v0.29.3, fixed #454):

```rust
pub fn init_database(crsqlite_path: Option<PathBuf>) -> Result<Connection, String> {
    let conn = open_connection(crsqlite_path)?;
    init_file_system()?;
    match run_migrations(&conn) {
        Ok(_) => {},
        Err(e) => {
            eprintln!("[CRITICAL] Migration failed: {}", e);
            // Continue with existing schema per §2B.2
        }
    }
    seed_defaults(&conn)?;
    restore_ai_settings_from_config(&conn)?;
    Ok(conn)
}
```

`init_database()` opens one connection, runs migrations on it, seeds defaults and restores AI settings on it, and returns that same connection to `DbState(Arc<Mutex<Connection>>)` in `lib.rs`. The returned connection is the app's long-lived connection — held for the app's entire lifetime, cached prepared statements accumulating across every query the app runs. The migration and the app share one connection. That sharing is what allows the cookie mismatch to happen.

**Why per-migration patches leave the landmine.** Reed's research surfaced three options: (1) drop-and-recreate FTS triggers around each migration; (2) rebuild FTS after each migrate; (3) separate migration connection. Options (1) and (2) fix the current #1908 symptom but leave the connection lifecycle unchanged — every future schema bump has to remember to include the fix. This is discipline-based, not mechanical. The next migration author who forgets — for notifications (next arc), or for any post-launch migration — reintroduces the bug for anyone on an upgrade path.

The pattern is the same as the "next schema bump hits the same gap" concern that produced ADR-0017 (PR#690 incident). ADR-0017 explicitly noted that `init_database`'s graceful-fallthrough behavior was "reinforced via spec, no code change" — the connection lifecycle question was deferred at that time. This ADR completes that question.

What if we didn't decide:

- **The current lifecycle ships.** Every future migration that touches an FTS-tracked table (notes, projects, agents, etc.) requires a per-migration fix. Migrations that don't touch FTS still bump the cookie and could break other cached prepared statements — a broader class than just FTS triggers. The landmine widens with each migration.
- **A per-migration patch ships.** #1908's symptom clears; the next unrelated migration produces a new instance of the same class. Detection depends on hitting the specific INSERT that fires a cached trigger on an upgraded DB — which only happens after upgrade, on real user data. Pre-launch dogfood may not surface it; the CI smoke check per ADR-0018 doesn't reproduce upgrade-path scenarios today.

## Decision

**Migrations run on a separate connection that is opened, used, and dropped inside a scoped block before the app's long-lived connection is opened.**

Concretely:

1. **`init_database()` opens two connections in sequence.** A short-lived `migration_conn` for `run_migrations()`, closed before `app_conn` is opened. The returned `app_conn` is the connection that becomes the app's long-lived `DbState`.

2. **`seed_defaults()` and `restore_ai_settings_from_config()` run on `app_conn`, not `migration_conn`.** Both are idempotent post-migration setup operations that write to tables the migration created. Running them on `app_conn` is safe because migrations have completed and been committed to the database file (WAL is per-database, not per-connection). It is preferred because it means `migration_conn`'s only responsibility is migrations.

3. **`migration_conn` is dropped by scope exit, not by explicit close.** Rust's ownership guarantees the connection is closed at the end of its block scope. This is mechanically enforced: `app_conn` cannot be opened before `migration_conn` drops because the block scope contains the `open_connection` call for the app connection outside the block, sequenced after it.

4. **Pre-migration backup (§2B.1) is unaffected.** Backup runs before any connection opens, per its current location in the startup path. This ADR does not change backup semantics.

5. **Graceful fallthrough (§2B.2) is preserved.** If `run_migrations()` fails on `migration_conn`, the `[CRITICAL] Migration failed` log fires as today, the migration connection drops via scope exit, and the app connection opens against the DB in its pre-migration state. The app runs with the old schema. No behavior change from §2B.2's contract.

6. **`open_connection()` is unchanged.** The existing two-function split (`open_connection` for background tasks, `init_database` for startup) is preserved. This ADR only changes what `init_database` does internally.

7. **cr-sqlite extension loads on both connections.** `open_connection()` loads the cr-sqlite extension per-connection; this is normal usage (cr-sqlite is a per-connection extension). Loading it twice during startup is not a concern.

The new `init_database()` body:

```rust
pub fn init_database(crsqlite_path: Option<PathBuf>) -> Result<Connection, String> {
    init_file_system()?;

    // Migrations run on a separate short-lived connection.
    // See ADR-0051: prevents cached prepared statements (e.g., FTS5 trigger VDBE
    // programs) from going stale when a mid-life migration bumps schema_cookie.
    // The app connection opens after this scope closes, sees the final schema,
    // and caches nothing stale.
    {
        let migration_conn = open_connection(crsqlite_path.clone())?;
        match run_migrations(&migration_conn) {
            Ok(_) => {},
            Err(e) => {
                eprintln!("[CRITICAL] Migration failed: {}", e);
                // Continue with existing schema per §2B.2
            }
        }
        // migration_conn dropped here at end of scope
    }

    // App connection — opens after migrations complete.
    let app_conn = open_connection(crsqlite_path)?;

    seed_defaults(&app_conn)?;
    restore_ai_settings_from_config(&app_conn)?;

    Ok(app_conn)
}
```

## Rationale

### Why separate-connection is a class-fix, not a symptom-fix

The bug's mechanism is generic: any prepared statement cached on a connection that survives a schema change can go stale. FTS5 trigger VDBE programs are the specific instance because they are auto-invoked on INSERT — but the class includes anything the connection has compiled and cached. A future migration that adds a view, a new trigger on the notifications table, or an index that a cached query planner had bound to could produce the same failure mode in a different guise.

The class-fix property: the app connection cannot observe a mid-life schema change if it opens after migrations have completed. The `schema_cookie` on the app connection at open time is the final post-migration value. Every prepared statement the app connection ever caches is stamped with that final cookie. Cookie mismatch is impossible by construction, not by patch.

The per-migration patches (drop-recreate FTS triggers; rebuild FTS after migrate) address the current instance without changing the class. Each future migration would require its own patch, and each future migration author would need to remember to include one. This is exactly the "discipline doesn't survive compaction" failure mode Matt has repeatedly flagged. The mechanical fix moves the guarantee into structure.

### Why `migration_conn` scope exit is load-bearing

The correctness proof for the class-fix depends on the app connection opening after the migration connection has dropped. In Rust, block scope guarantees this: `migration_conn` is bound inside `{ ... }`; the `Drop` impl for `Connection` runs at the closing brace; `app_conn` is bound after the brace on a subsequent line, so its `open_connection` call is sequenced after `migration_conn`'s drop.

This is enforced by Rust's ownership, not by developer discipline. A future refactor that moved `app_conn`'s opening inside the block, or held `migration_conn` in a wider scope, would break the class-fix. The spec amendment (§2B.6) states this rule so a future reader understands why the scope structure is not incidental.

### Why `seed_defaults` and `restore_ai_settings_from_config` move to `app_conn`

Under the current code, both run on the single shared connection after `run_migrations`. Moving them to `app_conn` has two properties worth naming:

- **Purity of `migration_conn`.** With this change, `migration_conn`'s only job is running migrations. Anything else — seeding, restoring, ambient setup — happens on `app_conn`. Future readers of `init_database` see a clear separation: migration scope vs application scope.
- **Correctness under WAL.** Migration writes go to the WAL file. When `migration_conn` drops without an explicit checkpoint, the writes remain in WAL and are visible to any subsequent connection opened on the same database file. `app_conn`'s `seed_defaults` and `restore_ai_settings_from_config` therefore see the post-migration schema. No special handling needed; SQLite's WAL mode handles the visibility transparently.

An alternative was to keep seed and restore on `migration_conn` (inside the block), but this would extend `migration_conn`'s lifetime past migration completion and require seed/restore to be safe for a connection whose prepared-statement cache still holds pre-migration state. Simpler to move them.

### Why this is "Proposed," not "Accepted"

Two reasons:

- **Zero users, no urgency on lock-down.** Per Matt's framing: "we have zero users right now — it's only me, on my one test DB." The pressure that usually pushes ADRs to Accepted quickly (production impact, coordination cost of the debate) doesn't apply here. There is time for the ADR to sit in Proposed state through a review cycle.
- **Interaction with post-launch schema changes still to be validated.** ADR-0017's per-version-pair migration model is preserved but hasn't been exercised across a launch boundary yet. The Proposed state invites Reed's review to confirm no post-launch invariant is quietly assumed by the current lifecycle.

Once Reed has reviewed and the follow-up regression test (below) has landed, the ADR flips to Accepted via the standard supplement-not-edit mechanism (playbook §2).

### Cross-reference to ADR-0017's "no code change" note

ADR-0017's References section names `src-tauri/src/database.rs::init_database` as "graceful-fallthrough behavior reinforced via spec, no code change." That statement was correct in April 2026 for the scope of the PR#690 incident (a migration authoring bug, not a connection-lifecycle bug). ADR-0051's scope is narrower and different: the connection lifecycle. §2B.2's graceful-fallthrough contract is unchanged by this ADR (Decision 5); what changes is which connection `run_migrations` runs on. ADR-0017 does not need amendment — the "no code change" note referred to graceful-fallthrough behavior, which is genuinely unchanged.

## Consequences

### Positive

- **The `unable to reset cached trigger stmt` class of bug is eliminated by construction.** Not patched — eliminated. The app connection cannot observe a mid-life schema change because there is no mid-life schema change on the app connection.
- **Future migrations require no per-migration cache-invalidation code.** Migration authors write SQL; the connection lifecycle handles the rest. Mechanical enforcement of a property that would otherwise require discipline on every migration.
- **Notifications arc (next) is unblocked.** The notifications table addition would produce the same bug under the current lifecycle; under this ADR, it will not.
- **§2B.2's graceful fallthrough contract is preserved.** Migration failure still logs `[CRITICAL]` and continues with the pre-migration schema. No user-facing behavior change from §2B.2's guarantees.
- **Backward compatible with fresh installs.** Fresh installs already work today; the new lifecycle produces identical outcomes for fresh installs (both connections open, migrations run on the first, app runs on the second — the first case just performs no migrations against a fresh schema).

### Negative

- **One extra `Connection::open` per startup.** Marginal cost. SQLite opens are cheap (a `Connection::open` on a WAL-mode local database is sub-millisecond). Both connections load the cr-sqlite extension; this is normal usage per cr-sqlite's design.
- **Slightly more code in `init_database`.** The block-scoped migration connection adds two lines and a comment. The spec's example code grows by the same amount.
- **A developer refactoring `init_database` in the future could break the class-fix by moving code across the scope boundary.** Mitigation: §2B.6 amendment explicitly documents why the scope structure is load-bearing, so a reader has scar tissue to check before restructuring. Additional mitigation: the follow-up regression test would fail if the structure were broken.

### Risks

- **A future migration relies on prepared statements cached during the migration.** If a migration prepares a statement inside `run_migrations` and expects it to be reused later, the drop of `migration_conn` invalidates that expectation. `run_migrations` today does not do this — each migration is `execute_batch`-based (raw SQL, no prepared statement reuse) — so this risk is theoretical. If a future migration needs prepared statement reuse across steps, that reuse must live inside the migration scope, not across `migration_conn`'s scope boundary.
- **A future refactor that combines `init_database` with other startup logic could accidentally hold `migration_conn` open.** Mitigated by the spec amendment documenting the load-bearing scope structure. Residual risk if a future developer doesn't read the spec; a regression test that runs migrations against a WAL DB with a pre-cached FTS trigger and asserts no error would catch this at CI time. Filed as follow-up.
- **cr-sqlite behavior across two same-database connections.** cr-sqlite is a per-connection extension; loading it twice for two connections against the same DB is standard usage. The gateway integration tests already do this. No behavior change expected. If cr-sqlite has any per-database state that would object, it would surface in existing tests. Not verified against cr-sqlite maintainer documentation as part of this ADR; flagged for Reed audit.
- **Race with `pre_migration_backup`.** Backup runs before `init_database` per §2B.1 and is not affected by this ADR. No new race introduced.

## Alternatives Considered

**Option 1 — Drop and recreate FTS triggers around each migration.** Each migration includes `DROP TRIGGER notes_fts_insert; DROP TRIGGER notes_fts_delete; DROP TRIGGER notes_fts_update;` at the start and `CREATE TRIGGER ...` (from the canonical schema) at the end. **Rejected** because it fixes the current #1908 symptom but leaves the class open. Migration authors would need to remember the drop-recreate pattern on every future migration, and the pattern would need to expand every time a new FTS-tracked table is added. Discipline-based, not mechanical. Fails the "mechanize over discipline" principle.

**Option 2 — Rebuild FTS after each migrate.** Call `INSERT INTO notes_fts(notes_fts) VALUES('rebuild');` at the end of `run_migrations`. **Rejected** because (a) it fixes only FTS; the class of bugs includes any prepared statement cached on the connection, not just FTS triggers; (b) it's expensive on large FTS indices; (c) same discipline problem as Option 1 — the next migration author has to remember. If a migration adds a new FTS-tracked table, the rebuild statement has to be updated too.

**Option 3 (this ADR) — Separate migration connection.** Class-fix; mechanical; preserves §2B.2 graceful fallthrough; no per-migration code required.

**Option 4 — Version prepared statement cache invalidation inside SQLite/rusqlite.** Modify the SQLite or rusqlite layer to invalidate cached statements when `schema_cookie` changes. **Rejected** because (a) rusqlite is an external dependency; patching upstream would create a fork burden; (b) the fix conflicts with SQLite's design intent (caching is an optimization; users are expected to reuse connections that don't span schema changes); (c) even if we could patch it, the mechanism would be more complex than opening a fresh connection.

**Option 5 — Reset the connection's prepared statement cache after migration.** Call some `conn.clear_cache()` API. **Rejected** because rusqlite exposes no such API. `PRAGMA integrity_check` and similar do not clear the cache. The only reliable way to clear the cache is to drop the connection, which is what this ADR does.

**Option 6 — Keep migrations on the app connection but issue `SELECT 1` or `PRAGMA schema_version` after migrations to force cache refresh.** **Rejected** because these do not clear cached VDBE programs — they force `schema_cookie` to be re-read, but existing cached programs remain stamped with their old cookie. The cookie mismatch occurs on the cached program's next invocation, not on the connection's next query.

**Option 7 — Defer this fix to post-launch.** Ship v1 with per-migration patches (Option 1 or 2) applied to #1908; do the class-fix later. **Rejected** because (a) the notifications arc is next and will hit this bug on the same path; (b) any post-launch schema change hits this bug on real user data; (c) "no urgency" (Matt's framing) means we have time to do the right fix now rather than paying the discipline tax on every future migration.

## References

- Bug: #1521 Phase 8 P0 — Note/task creation fails on upgrade with "unable to reset cached trigger stmt." Confirmed live on v0.67.94.
- Reed research: `docs/audits/2026-07-05-trigger-stmt-research.md` — mechanism, options, recommendation.
- Live source verified: `src-tauri/src/database.rs::init_database` as documented in `docs/spec/file-system.md` §2B.6 (BUILT v0.29.3, fixed #454); consumers of `DbState` verified at `lib.rs` per ADR-0044 addendum's source-read.
- Migration that triggered the observed instance: `src-tauri/src/migrations.rs` v68→v69 (og_cache table, #1908). The migration is correct; the bug is in the connection lifecycle it runs on.
- ADR-0017 — PR#690 migration fix + reconciliation. Left the connection-lifecycle question as "no code change"; this ADR completes it.
- ADR-0018 — CI smoke check for `[CRITICAL]` startup logs. The detection layer that catches migration failures. Would surface a future connection-lifecycle regression if the migration were logged as `[CRITICAL]`; would not surface silent statement-cache staleness.
- ADR-0019 — Pre-launch schema management clarification. The per-version-pair migration model this ADR preserves.
- `docs/spec/file-system.md` §2B.1 — Pre-migration backup (unchanged).
- `docs/spec/file-system.md` §2B.2 — Graceful fallthrough on migration failure (unchanged).
- `docs/spec/file-system.md` §2B.6 — Database Initialization Pattern (amended by this ADR — see spec amendment below).
- Playbook §2 — ADR immutability rule (ADR-0017's body is not edited; this ADR supplements).

## Follow-ups filed (not gated by this ADR)

- **Regression test.** A test that (a) creates a DB at schema version N with FTS triggers, (b) performs an INSERT to cache the trigger VDBE program, (c) runs a migration to N+1 that would bump `schema_cookie`, (d) opens a new connection per this ADR's pattern, (e) INSERTs again and asserts no `unable to reset cached trigger stmt` error. Should live in `src-tauri/tests/` or equivalent. Not gated by this ADR's landing; filed for Stia's implementation PR.
- **Notifications arc coordination.** The notifications table migration (next arc) benefits from this fix once landed. If notifications' spec-lock precedes this ADR's implementation, note the dependency in notifications' spec so both land together.
- **Post-launch: verify class-fix behavior on production-like DB.** Once real user DBs exist, verify the fix behavior on a copy of a real DB with a synthetic migration. This is the same "test migrations on real DB copy" discipline named in §2D Tier 2 audits. Not a separate follow-up — falls out of existing Tier 2 process.
- **Cross-check with cr-sqlite maintainer documentation.** Confirm cr-sqlite has no per-database state that would object to two connections against the same DB during startup. Existing tests suggest not, but explicit maintainer sign-off would harden the ADR. Reed follow-up.

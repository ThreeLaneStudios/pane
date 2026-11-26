//! #470 PR 3: seeds a real `PANE_TEST_DB`/`PANE_TEST_ROOT` fixture so a
//! journey (or, for this PR, the existing `e2e/tests/smoke.spec.ts`) launches
//! past onboarding.
//!
//! Approved mechanism (Matt, per the PR 3 scope doc
//! `2026-11-25-scope-470-pr3-seeded-fixture.md` and its mock-Ollama
//! follow-up, issue #470 comments 7169738403 / 7170104377): seed through the
//! REAL `create_local_agent` command, run before the app launches. No
//! committed database file, no hand-written SQL.
//!
//! Why this is a `#[cfg(test)]` lib unit test and not a `tests/*.rs` Cargo
//! integration test: every `mod` in `lib.rs` is private (`mod agents;`, not
//! `pub mod agents;`), and `lib.rs` has no `pub use` re-exports at all — so
//! `create_local_agent` is simply unreachable from a true integration-test
//! crate (which only sees this crate's public surface). Every existing
//! fixture generator under `tests/` (`build_v73_fixture.rs` et al.) inlines
//! raw SQL for exactly this reason — see that file's own "rather than
//! calling the private pane_lib functions" comment. A `#[cfg(test)]` module
//! declared in `lib.rs` (this file) compiles as part of the crate itself —
//! the same mechanism `agents.rs`'s own
//! `upsert_agent_explicit_runtime_type_tests` module already uses to call
//! `upsert_agent` through its real `#[tauri::command]` signature via
//! `tauri::test::mock_app()` + `DbState`.
//!
//! Onboarding gate traced directly (`notes.rs:3003-3123` ->
//! `ai::runtime_resolution::derive_active_provider_label`,
//! `runtime_resolution.rs:318-349`): it needs exactly one `ai_agents` row
//! with `is_active = 1` and a non-NULL `runtime_type`. It never reads
//! `ai_settings.ollama_endpoint`. `create_local_agent` already writes
//! `IDENTITY.md`/`SOUL.md`/`MEMORY.md` to disk before the DB row (ADR-0039
//! D7), so the filesystem side comes free through the same real command.
//!
//! `ollama_endpoint` is seeded anyway, to an inert fast-refusing address —
//! not because the onboarding gate needs it, but because `lib.rs`'s eager
//! `populate_model_registry_from_provider` background spawn dials whatever
//! `ollama_endpoint` is configured on every launch (detached, fails
//! silently, but a routable-and-silent address would ride out its full 5s
//! timeout on every CI run for no benefit — issue #470 comment 7170104377,
//! §3). `http://127.0.0.1:1` is the exact value `ai/scheduler.rs`'s own
//! existing tests already use for the same reason.
//!
//! #470 PR 4: `ollama_endpoint` now reads `MOCK_OLLAMA_ENDPOINT` first,
//! falling back to the same inert address when it's unset. Additive and
//! backward-compatible — `e2e-linux.yml`'s own smoke-test-only invocation
//! of this test never sets `MOCK_OLLAMA_ENDPOINT`, so it still seeds the
//! inert fast-refusing address exactly as before. The journeys step sets
//! it to a real, locally-bound mock Ollama server (`e2e/journeys/mock-
//! ollama.mjs`) before running this same seed step, so the voluntary
//! create-agent flow's model picker (`fetch_available_models` ->
//! `fetch_ollama_models` -> `GET {endpoint}/api/tags`) gets a real,
//! fixed-response model list instead of an instant `ECONNREFUSED`.
//!
//! #470 PR 474 round 3 (Reed's Ruling 3): this test now loads the crsqlite
//! extension before running migrations, the same way the real binary does.
//! Before this fix, `init_database(None)` stamped the fresh test DB at the
//! current `SCHEMA_VERSION` with CRR never enabled on any table; the real
//! `pane` binary launched next *with* the extension, saw a schema version
//! that already matched, skipped migration entirely, and `verify_crr_tables`
//! (the migration-skip path's own sanity check) then failed for `projects`
//! — a real `[CRITICAL] Migration failed: CRR verification failed` line in
//! `pane-test.log`, caused by this fixture's own shortcut, not a shipping
//! defect (a genuine fresh install always loads the extension from its
//! first launch, so it can never reach this state). Fixed by passing
//! `Some(crsqlite_path)` to the exact same `database::init_database` the
//! real binary calls, instead of `None` -- the loading mechanism itself
//! (`database::load_crsqlite_extension`, reached through `init_database`)
//! is reused completely unchanged; only *locating* the `.so`/`.dylib` on
//! disk needed test-specific handling, and even that is empirically
//! verified against the real binary's own resolution below, not guessed.
//!
//! Resolving the path: `lib.rs`'s `.setup()` calls
//! `app.path().resource_dir()` (a real `AppHandle`, which this
//! `#[cfg(test)]` module never has) then joins `"resources"` +
//! `CRSQLITE_FILENAME`. Checked empirically rather than assumed: the
//! compiled `pane` debug binary itself prints `✓ Loaded cr-sqlite
//! extension from ".../target/debug/resources/crsqlite.dylib"` on a real
//! launch, confirming `app.path().resource_dir()` resolves to
//! `target/debug` for that binary (exe's own parent dir) --
//! `tauri::test::mock_app()`'s `resource_dir()` does NOT reproduce this:
//! probed directly, it returns `target/debug/resources` already (one
//! level too deep for the same `.join("resources")` lib.rs applies,
//! confirmed by a scratch probe test, output discarded). A `cargo test
//! --lib` binary's own `current_exe()` also sits one level deeper than the
//! real `pane` binary's (`target/debug/deps/pane_lib-<hash>`, not
//! `target/debug/pane`), for the unrelated reason that cargo nests test
//! binaries under `deps/`. `crsqlite_resource_path()` below walks up from
//! `current_exe()` far enough to land on whichever ancestor directory
//! actually has a `resources/<FILENAME>` sitting in it on disk (populated
//! by `tauri-build`'s build script during `cargo build`, confirmed to run
//! for a plain `cargo build`/`cargo test`, not only `tauri build`) --
//! landing on the exact same `target/<profile>/resources/<FILENAME>` file
//! the real binary itself loads, regardless of which nesting depth this
//! particular test binary happens to sit at. This is locating a file on
//! disk, not reimplementing or stubbing the extension load itself.
//!
//! Run with (from `src-tauri/`):
//!   cargo test --lib seed_journey_fixture -- --ignored --nocapture
//!
//! `#[ignore]`: never swept into the default `cargo test` run (`ci.yml`) —
//! this test mutates a real on-disk DB file at `PANE_TEST_DB` and is meant
//! to run exactly once, as its own CI step in `e2e-linux.yml`, immediately
//! before Pane launches against that same path.

#[cfg(test)]
mod tests {
    use crate::agents::{create_local_agent, CreateLocalAgentRequest};
    use crate::database::{self, DbState};
    use rusqlite::Connection;
    use std::path::PathBuf;
    use tauri::Manager;

    // `#[cfg(target_os)]` filenames match lib.rs's own
    // `CRSQLITE_FILENAME` constants verbatim -- same three platforms,
    // same names, same `tauri.conf.json` `bundle.resources` entries.
    #[cfg(target_os = "macos")]
    const CRSQLITE_FILENAME: &str = "crsqlite.dylib";
    #[cfg(target_os = "linux")]
    const CRSQLITE_FILENAME: &str = "crsqlite.so";
    #[cfg(target_os = "windows")]
    const CRSQLITE_FILENAME: &str = "crsqlite.dll";

    /// Finds the crsqlite extension file this test binary's own build
    /// produced, by walking up from `current_exe()` -- see this module's
    /// doc comment above for why a fixed join count (what `lib.rs` itself
    /// uses against a real `AppHandle`) doesn't transfer to a `cargo test`
    /// binary, and why this is locating a file, not reimplementing how
    /// it's loaded. Bounded at 6 ancestors so a genuine absence (resources
    /// never built) fails fast with `None` instead of walking to `/`.
    fn crsqlite_resource_path() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        exe.ancestors().skip(1).take(6).find_map(|dir| {
            let candidate = dir.join("resources").join(CRSQLITE_FILENAME);
            candidate.exists().then_some(candidate)
        })
    }

    /// Inert, fast-refusing: nothing is bound on port 1, so a connection
    /// attempt gets `ECONNREFUSED` near-instantly on Linux rather than
    /// riding out a timeout. Same value `ai/scheduler.rs`'s own tests
    /// already use for this purpose (e.g. its `http://127.0.0.1:1` seeds).
    const INERT_OLLAMA_ENDPOINT: &str = "http://127.0.0.1:1";

    #[test]
    #[ignore] // Run manually / as its own CI step -- see module doc comment.
    fn seed_journey_fixture() {
        // This file is a lib unit test, not a `tests/*.rs` integration
        // test, so `get_pane_root()`'s own `#[cfg(test)]` panic guard
        // (database.rs) IS compiled in and active for this binary -- but
        // `get_db_path()` has no equivalent panic guard for
        // PANE_TEST_MODE/PANE_TEST_DB (it just `eprintln!`s and silently
        // falls back to the production DB path), so this test asserts all
        // three explicitly before touching anything, rather than relying on
        // that asymmetry.
        let test_root = std::env::var("PANE_TEST_ROOT").unwrap_or_default();
        assert!(
            !test_root.is_empty(),
            "PANE_TEST_ROOT must be set (non-empty) before running seed_journey_fixture -- \
             refusing to seed a real path. e2e-linux.yml sets this at job level; set it \
             yourself if running locally."
        );
        let test_mode = std::env::var("PANE_TEST_MODE").unwrap_or_default();
        assert_eq!(
            test_mode, "1",
            "PANE_TEST_MODE must be \"1\" before running seed_journey_fixture -- without it, \
             database::get_db_path() silently falls back to the production DB path."
        );
        let test_db = std::env::var("PANE_TEST_DB").unwrap_or_default();
        assert!(
            !test_db.is_empty(),
            "PANE_TEST_DB must be set before running seed_journey_fixture -- without it, \
             database::get_db_path() silently falls back to the production DB path."
        );

        // Step 1: the exact migration path the app itself runs on first
        // launch (`init_database` -> `init_database_with_progress` ->
        // `run_migrations`), at the exact `PANE_TEST_DB` path the app will
        // open next. Round 3: `Some(crsqlite_path)`, not `None` -- CRR
        // tracking DOES matter here even though the onboarding gate itself
        // never reads it, because the REAL `pane` binary launches next
        // *with* this same extension loaded (`lib.rs`'s `.setup()`); a
        // schema stamped without CRR enabled, read back by a connection
        // that has the extension loaded, is exactly the mismatch Reed's
        // Ruling 3 traced to the `[CRITICAL]` log line -- see this file's
        // module doc comment above. Every OTHER `run_migrations` unit test
        // in this crate (including `upsert_agent_explicit_runtime_type_
        // tests::setup()` in agents.rs) stays on the no-extension shape
        // deliberately: none of them launch the real compiled binary
        // against their DB afterward, so none of them can hit this
        // mismatch -- this fixture is the one caller where it matters.
        let crsqlite_path = crsqlite_resource_path();
        assert!(
            crsqlite_path.is_some(),
            "seed_journey_fixture: could not find {CRSQLITE_FILENAME} under any ancestor \
             of {:?} -- `cargo build` (which copies tauri.conf.json's bundle.resources via \
             tauri-build's build script) must run before this test.",
            std::env::current_exe()
        );
        let conn: Connection = database::init_database(crsqlite_path)
            .expect("seed_journey_fixture: init_database failed");

        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));

        // Step 2: the real `create_local_agent` command -- not a raw
        // INSERT. Writes the `ai_agents` row (is_default=1, is_active=1,
        // runtime_type='ollama' -- the onboarding gate's exact requirement,
        // confirmed by the literal INSERT in create_local_agent_impl) AND
        // IDENTITY.md/SOUL.md/MEMORY.md on disk under
        // PANE_TEST_ROOT/.pane/system/{workspace_path}/, both for free, via
        // the same code path the frontend's AgentCreateOverlay calls.
        let db_state = app.state::<DbState>();
        // #470 PR 474 round 2: a natural name, not "Test"/"Fixture"/"Seed" --
        // these screenshots become docs pages, and a test-shaped agent name
        // sitting next to the journey's own freshly-created "Research Agent"
        // undermines the page. "Scout" reads like a name a real user would
        // actually give a default assistant.
        let agent = create_local_agent(
            CreateLocalAgentRequest {
                name: "Scout".to_string(),
                model: Some("llama3.2".to_string()), // CI-ALLOW: hardcoded-model
                description: None,
                runtime_type: "ollama".to_string(),
            },
            db_state,
        )
        .expect("seed_journey_fixture: create_local_agent failed");

        assert!(agent.is_default, "seeded agent must be is_default=1 (onboarding gate's precedence path)");
        assert!(agent.is_active, "seeded agent must be is_active=1 (onboarding gate's exact requirement)");
        assert_eq!(
            agent.runtime_type.as_deref(),
            Some("ollama"),
            "seeded agent must have a non-NULL runtime_type (onboarding gate's exact requirement)"
        );

        // Step 3: ollama_endpoint -- a plain ai_settings write, the same
        // lighter-weight mechanism dozens of existing unit tests already
        // use (e.g. ai/runtime_resolution.rs, ai/scheduler.rs). The
        // onboarding gate never reads this key; it's seeded so the one
        // detached background provider-registry call (lib.rs) and the
        // voluntary create-agent flow's model picker both have a real
        // address to dial.
        //
        // #470 PR 4: additive env override. When the journeys workflow step
        // starts the mock Ollama server first and sets MOCK_OLLAMA_ENDPOINT
        // (e2e/journeys/mock-ollama.mjs's bound port), the seeded setting
        // points at it. When unset (the smoke-test-only path PR 3 already
        // exercised), falls back to the same inert, fast-refusing address
        // as before -- behavior for that path is unchanged.
        let ollama_endpoint = std::env::var("MOCK_OLLAMA_ENDPOINT")
            .unwrap_or_else(|_| INERT_OLLAMA_ENDPOINT.to_string());
        {
            let db_state = app.state::<DbState>();
            let conn_guard = db_state.0.lock().expect("db mutex poisoned");
            database::set_ai_setting(&conn_guard, "ollama_endpoint", &ollama_endpoint)
                .expect("seed_journey_fixture: set_ai_setting(ollama_endpoint) failed");

            // Flush WAL to the main DB file -- the next reader is a
            // separate OS process (the `pane` binary launched after this
            // test exits), not this same connection.
            conn_guard
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                .expect("seed_journey_fixture: wal_checkpoint failed");

            // Proof-of-seed, printed unconditionally (not just on failure)
            // so a CI log always shows exactly what landed, per the task's
            // "if onboarding still shows, report what the gate checked and
            // what the seeded DB actually contained" requirement.
            let (id, name, is_default, is_active, runtime_type): (String, String, i64, i64, Option<String>) = conn_guard
                .query_row(
                    "SELECT id, name, is_default, is_active, runtime_type FROM ai_agents WHERE id = ?1",
                    [&agent.id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
                )
                .expect("seed_journey_fixture: readback of seeded ai_agents row failed");
            eprintln!(
                "[seed_journey_fixture] seeded ai_agents row: id={id} name={name} is_default={is_default} \
                 is_active={is_active} runtime_type={runtime_type:?} (db={test_db})"
            );
        }
    }
}

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
    use tauri::Manager;

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
        // open next. `None` for the crsqlite extension path: CRR tracking
        // is irrelevant to the onboarding gate and to either smoke test --
        // the same no-extension shape every other `run_migrations` unit
        // test in this crate already uses (including
        // `upsert_agent_explicit_runtime_type_tests::setup()` in
        // agents.rs).
        let conn: Connection =
            database::init_database(None).expect("seed_journey_fixture: init_database failed");

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
        let agent = create_local_agent(
            CreateLocalAgentRequest {
                name: "Journey Fixture Agent".to_string(),
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

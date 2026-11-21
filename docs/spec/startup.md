# §35. Startup Sequencing & Migration UX

**Status:** BUILT (#2723 PR B — `feat/2723-window-first-updating-screen`)
**Relates to:** §25 (Backup System, `docs/spec/backup.md`), #2723, #2583, #2724, #353
**Last updated:** 2026-10-24

---

## 0. Why this exists

PR A (#3064, merged) made the pre-migration backup's copy step fast (50 MB:
628s → 0.14s, measured). That fixed the *dominant* cost on a migrating
launch, but not the *visible* one: even at full speed, a chained migration
across several schema versions, a large note library's §486 integrity
check, or just normal disk I/O can still take a real, user-noticeable
moment — and until this PR, that moment happened entirely **before the
window ever painted**, with zero feedback. Reed's audit
(`docs/audits/2026-10-23-scope-slow-launch-updating-screen.md` §3) traced
this to Tauri's own lifecycle, not a bug in any one function:

> `Builder::build()` constructs the app and declarative windows, then
> invokes `.setup()` synchronously, **before `.run()` starts pumping the
> native event loop.** A window object can exist without being able to
> paint/render until that event loop is running.

Pane's window is declared declaratively in `tauri.conf.json` (`app.windows[0]`
— no `WebviewWindowBuilder`, no `"visible": false`). It paints as soon as
the event loop starts. Pre-this-PR, `.setup()` didn't return until
`database::init_database()` — including, on a migrating launch, the
pre-migration backup and the schema migration itself — had already
finished. **There was never a code bug to fix here; there was a sequencing
decision to change.**

## 1. The fix, in one sentence

A cheap, read-only pre-check decides, before any real work starts, whether
this launch can take the exact same synchronous path it always has (no
migration pending — the overwhelming majority of launches), or needs to let
the window paint first and do the real work on a background thread (a
migration is pending). This is not a new architecture — it generalizes
issue #353's already-shipped pattern (commit `0880f573`, "Move daily backup
to background thread after window renders") from the daily backup to the
one case that can actually block the window: the pre-migration path.

## 2. The two paths

### 2.1 Fast path (no migration pending — unchanged)

`database::is_migration_pending(crsqlite_path)` opens a throwaway connection
and compares the stored `schema_version` to `migrations::SCHEMA_VERSION`
(the exact same comparison `migrations::run_migrations`'s first match arm
makes). A fresh install (`None` — no `schema_version` row yet) and an
install already at the current version are both **not pending** — a fresh
install has nothing to back up yet and schema creation on an empty DB is
already fast.

When not pending, `lib.rs`'s `.setup()` calls
`complete_db_dependent_startup(&app, crsqlite_path)` **inline, on the setup
thread**, and propagates a hard failure via `?` exactly as every version of
this code has since before this PR. **No new thread is spawned.** This is
regression-tested (`database::test_is_migration_pending_false_for_fresh_install`,
`test_is_migration_pending_false_when_schema_current`) and proved live by
the `smoke-fresh-db` run (see the PR description for this feature) — the
real compiled binary takes this path on a fresh install and reaches ready
with no deferred-startup log line.

### 2.2 Window-first path (migration pending)

1. `.setup()` records `StartupStatus::MigrationRequired` (see §3) and emits
   `startup://migration-required`.
2. `.setup()` returns **immediately**, without having touched the database
   at all. `.run()` starts pumping the event loop; the window paints.
3. `complete_db_dependent_startup` — the *exact same function* the fast
   path calls inline — runs on a `std::thread::spawn`'d background thread,
   mirroring `#353`'s daily-backup shape precisely.
4. On success: `StartupStatus::Ready`, `startup://ready` emitted.
5. On failure: `StartupStatus::Failed(message)`, `startup://failed` emitted
   (see §5).

`complete_db_dependent_startup` is the single function both paths call —
there is no separate "slow path" implementation of the startup sequence to
drift out of sync with the fast path's. It performs, in order: DB init +
migration (`database::init_database_with_progress`), mounting `DbState`,
then every dependent setup block (§486 note-body integrity, the §33.8 and
workshop orphan sweeps, sync/auto-sync/budget/model-registry state,
credential migration, the §24.1 agent integrity check) and the
already-background-thread work that existed pre-this-PR (FTS rebuild,
consolidation staleness check, sync pull, periodic pull, notification
watcher, due-date check, daily backup, file watcher).

## 3. Ordering guarantee

**Requirement:** `set_schema_version` must complete before any dependent
setup block (§486, the orphan sweeps, every other `DbState` consumer) runs —
on both paths. Moving DB work onto a background thread is exactly the kind
of change that could let a consumer observe a half-migrated database if
this weren't held structurally.

**How it's held, not just asserted:**

- `database::init_database_with_progress(...)?` cannot return a
  `Connection` without `migrations::run_migrations_with_progress` — and
  therefore `set_schema_version` on every reachable match arm — having
  already run to completion on it. This is unconditional; it's the same
  function `init_database` has always delegated to.
- `startup::mount_db_state_then(app, conn, on_db_ready)` is a two-line
  function: `manager.manage(DbState::new(conn))`, then
  `on_db_ready(manager)`. `conn` cannot exist at the call site without the
  bullet above having already happened; `on_db_ready` cannot run before
  `manage()` returns, by ordinary sequential execution within one function
  body. There is exactly one place in the codebase where "mount `DbState`,
  then run the dependent blocks" could possibly be reordered, and this
  function's own body does not allow it.
- Both the fast path (inline call) and the slow path (background thread)
  invoke the same `complete_db_dependent_startup` → `mount_db_state_then`
  chain. There's no second, divergent implementation.

**Tested, not just reasoned about:**
`startup::test_schema_version_committed_before_db_state_is_managed`
(`src-tauri/src/startup.rs`) copies the real
`tests/fixtures/upgrade-from-v91.db` fixture, runs the real `run_migrations`
on it, confirms `schema_version` actually changed, then calls
`mount_db_state_then` against `tauri::test::mock_app()` with a spy
dependent-block closure that itself re-reads `schema_version` off the
`DbState` it receives and asserts it's already current — proving the
closure never sees a pre-migration connection.

## 4. `StartupStatus` + `get_startup_status` (race-safety design)

The frontend needs to distinguish "no migration, mount the real app" from
"migration in progress, show the Updating screen" **without an
event-ordering race**: an event emitted before a `listen()` call attaches
is silently lost, and on the fast path `.setup()` can resolve to `Ready`
before the webview has even started running JS.

The fix is a `State`, read via a request/response command, not an event:

```rust
pub struct StartupStatus(Mutex<StartupPhase>); // NotYetDetermined | MigrationRequired | Ready | Failed(String)

#[tauri::command]
pub fn get_startup_status(status: tauri::State<StartupStatus>) -> StartupStatusPayload
```

`app.manage(StartupStatus::new())` is the **very first line** inside
`.setup()` — before crsqlite path resolution, before the migration
pre-check, before anything else — so `get_startup_status` (no `DbState`
dependency) is safely callable from the first instant the frontend can
reach it, on every launch, regardless of which path this run takes. A
command always reflects the state *at the moment it's asked*; there is no
"missed the emit" failure mode for this initial read.

Once the frontend has confirmed it's in `migration_required`, it switches
to **events** for live updates (`backup://progress`, `migration://progress`,
terminal `startup://ready` / `startup://failed`) — a good fit once already
inside the narrower, longer-lived migrating-launch window — but keeps a
defensive 500ms re-poll of `get_startup_status` running alongside them in
case the terminal event itself is missed (e.g. a very fast migration
completing before `listen()` finishes attaching).

## 5. Progress events

| Event | Source | Payload | Pre-existing? |
|---|---|---|---|
| `backup://progress` | `backup.rs` `emit_progress`, called from the 8 phases inside `create_backup` | `{ phase, message, pct }` | Yes — PR A (#3064). `create_migration_backup` now threads an `Option<&AppHandle>` through to it (mechanical signature addition; `None` for every one of its ~30 pre-existing call sites via `run_migrations` → progress is a no-op there). |
| `migration://progress` | `migrations.rs` `migration_step` | `{ step, total, label, pct }` | New (#2723 PR B). Fires alongside the pre-existing `[MIGRATIONS] step N/M` log line (#2724) — same no-op-when-`None` guarantee as above. |

**Never file size.** Reed measured directly: the destination `.db`'s byte
size stays near 0 until late in the copy (PR A's own kill-test comments
confirm this independently). Progress is derived exclusively from the
phase/step events above — there is no code path in this feature that reads
a file size to report progress.

## 6. Failure path — design decision

**If `complete_db_dependent_startup` returns a hard `Err`** (realistically:
`init_file_system`, the crsqlite extension failing to load, or a
`seed_defaults`/`restore_ai_settings_from_config` failure — not a migration
`Err`, which this code already treats as a soft "continue with existing
schema" degraded mode, unchanged by this PR), the screen shows:

- **"Update Failed"** in red, plus the real error message (plain text, not
  a stack trace).
- A fixed reassurance line: *"Pane takes a backup of your database before
  applying updates — your data is safe. Quit and relaunch Pane to try
  again."*
- **One button: Quit Pane** (`@tauri-apps/plugin-process` `exit(1)`).

**Why Quit, not Retry-in-place:** the realistic failure modes here are
filesystem/extension-level (disk full, a corrupted `crsqlite.dylib`,
permissions) — not retriable by re-running the same logic in the same
process. Retrying in-place would also mean re-entering
`complete_db_dependent_startup` after partial global state has already been
set (`database::set_crsqlite_path`, static caches) that was never designed
to be re-entered mid-session. A fresh process launch is the only state this
codebase can be fully sure starts clean — and it's the exact same recovery
story the pre-existing degraded-migration-continue path already relies on
(it doesn't attempt an in-place retry either). **Never a blank window,
never an endless spinner** — the explicit failure state is the fix for
exactly that failure mode.

## 7. Frontend contract

`src/lib/components/UpdatingScreen.svelte` wraps `+layout.svelte`'s
`children` snippet and is the one gate before the real app mounts or any
`DbState`-dependent command fires:

- `ready` → renders `children()` (the real app) — on the fast path this
  resolves on the very first `get_startup_status()` call, before any
  `listen()` is ever attached; there is no extra visible delay.
- `migration_required` → "Updating Pane… don't quit", a progress bar bound
  to the events in §5, defensive re-poll per §4.
- `failed` → §6's failure screen.
- `not_yet_determined` → a vanishingly brief, content-free screen (should
  never be observed in practice; included so the default state is honest
  rather than silently guessing).

## 8. What Matt must look for on install (cannot be automated)

Reproduced verbatim from Reed's scope audit
(`docs/audits/2026-10-23-scope-slow-launch-updating-screen.md`, "What Matt
must look for on install") — the one thing no agent here can verify:

> On a build where a migration is pending (force this by restoring an
> older-schema backup, or have Stia stage one):
> 1. **A window appears within roughly half a second of launch** — not a
>    beachball, not a blank/no-window gap.
> 2. That window shows the literal text **"Updating Pane… don't quit"** (or
>    whatever final copy ships) **immediately**, not after a delay.
> 3. **Progress visibly advances** — a percentage or step indicator that
>    moves at least twice during the update, not a static spinner.
> 4. **The window never freezes/hangs unresponsively** at any point during
>    the update (drag it, resize it — it should still repaint).
> 5. When the update finishes, **the real app loads automatically** — no
>    manual action needed, no flash of a broken/empty app shell first.
> 6. **Quitting the app from the Updating screen** (Cmd+Q or force-quit)
>    and relaunching should land back on the same Updating screen or
>    complete cleanly — not corrupt, not stuck.
>
> Everything else (progress event payloads, phase percentages, the
> pre-check branch logic, thread spawning) is covered by automated tests —
> only the five items above require Matt's eyes.

## 9. Cross-references

- `docs/spec/backup.md` — backup phase events (`backup://progress`), copy
  pacing, archive-skip-for-Migration. PR A's half of #2723.
- `docs/audits/2026-10-23-scope-slow-launch-updating-screen.md` — Reed's
  scoping audit this PR implements.
- Issue #353 — the daily-backup background-thread precedent this PR
  generalizes.

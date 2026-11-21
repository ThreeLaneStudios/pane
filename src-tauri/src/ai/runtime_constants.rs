//! Named constants for the runtime/scheduler sprint (`docs/spec/runtime.md`
//! §36.10A, #3050). One block, no literals elsewhere — same convention
//! `context_budget.rs` already uses for its own policy numbers.
//!
//! §36.10A's full table has more entries than this file does today
//! (`RUNTIME_ATTACHMENT_TEXT_CAP`, `RUNTIME_NOTIFICATION_EXPIRY_DAYS`,
//! `RUNTIME_HEARTBEAT_DEFAULT_PROMPT`, `RUNTIME_SCHEDULE_PRESETS`) — those
//! are added by the PRs that actually consume them (attachments PR 9,
//! notifications PR 6, UI PR 8), per Reed's scope audit for this PR.
//! `RUNTIME_MAX_SPAWN_DEPTH`, `RUNTIME_MAX_CHAIN_DEPTH`,
//! `RUNTIME_JOB_ARCHIVE_DAYS`, `RUNTIME_OC_PICKUP_WINDOW_DAYS`, and
//! `RUNTIME_OC_PICKUP_SESSIONS_PER_TICK` were in this same "named but not
//! yet defined" state before #3050 PR 5/PR 7 respectively — each PR is the
//! one that consumes its own constants, so they are now real consts below,
//! not dead code with no test to anchor them.

/// §36.3.1: the scheduler tick's interval — the only clock in the scheduler,
/// no per-job timer thread and no OS-level scheduling (same restraint as
/// §19.5's event-driven watcher).
pub const RUNTIME_TICK_SECS: u64 = 60;

/// §36.3.3: seed/fallback for `ai_settings.job_concurrency` — the max number
/// of jobs a tick claims and runs at once. Interactive turns are never
/// counted against this; they always run. Read-time fallback only (no
/// INSERT at migration time — see `scheduler.rs`'s read site), matching the
/// existing `max_tool_iterations` precedent (`chat.rs`).
pub const RUNTIME_DEFAULT_CONCURRENCY: u32 = 2;

/// §36.2.2: seed/fallback for `ai_settings.job_ceiling_tool_calls` — the
/// total-tool-call ceiling for one job run. Read by `run_agent_loop`'s
/// auto-continue branch (§36.2, PR 3) as the fallback when a call site
/// supplies no `continue_policy`, and by `ai_chat_stream`'s own settings
/// read (same `unwrap_or` precedent `max_tool_iterations` already uses).
pub const RUNTIME_DEFAULT_JOB_CEILING: u32 = 200;

/// §36.2/§3.5 of `docs/audits/2026-10-27-scope-3050-pr3-autocontinue.md`
/// (Reed): the allowlist gating auto-continue to API runtimes, never
/// `openclaw` (§36.0.1 — OpenClaw keeps its own runtime; Pane never
/// self-continues a turn for an OC-bound session). Deliberately a SEPARATE
/// constant from `ai::tools::WEB_TOOL_RUNTIME_ALLOWLIST` even though the two
/// lists are identical today — Reed's audit option (b): the two allowlists
/// answer different questions ("can this runtime make a web call" vs. "can
/// this runtime's turn continue itself") and sharing one constant would
/// couple two conceptually independent policies that merely happen to agree
/// right now. If they ever need to diverge (e.g. a future runtime that may
/// browse but may not auto-continue), this constant is the one to edit —
/// `ai::tools`'s own gate is untouched by that change.
pub const API_RUNTIME_ALLOWLIST: [&str; 4] = ["anthropic", "openrouter", "ollama", "venice"];

/// §36.4/§4 of Reed's PR 5 scope audit: the cap on CONCURRENT nesting — "a
/// spawn of a spawn is the floor." Not a stored counter: a turn's own depth
/// is the presence/absence of `agent_jobs.parent_job_id` on the calling
/// job's row, checked at `spawn_job` dispatch time
/// (`ai::tools::execute_single_tool`'s `spawn_job` arm). Deliberately a
/// SEPARATE mechanism from `RUNTIME_MAX_CHAIN_DEPTH` below — depth is about
/// concurrent nesting within one turn-chain (a spawn calling a spawn, same
/// moment); chain is about sequential wake-chaining over time (job
/// finishes, wakes origin, origin spawns the next one, repeat). Conflating
/// the two was the single biggest risk Reed's scope audit named for this
/// PR — see that document's §4 for the full reasoning.
pub const RUNTIME_MAX_SPAWN_DEPTH: u32 = 2;

/// §36.4.5/§5 of Reed's PR 5 scope audit: the cap on SEQUENTIAL
/// wake-chaining — a chain of jobs linked by `wake_origin=1` completions,
/// sharing one `root_job_id`. The 9th job in a chain (`chain_depth` == this
/// constant) has `spawn_job`/`schedule_job` withheld from its offered tool
/// set (`ai::tools::get_pane_tools_for_runtime`'s `chain_capped` param) —
/// offer-time only, same as the depth cap above is dispatch-time only; the
/// two never share a check.
pub const RUNTIME_MAX_CHAIN_DEPTH: u32 = 8;

/// §5.3 of Reed's PR 5 scope audit: proxy for "this spawn session is
/// archived" — no real `chat_sessions.archived_at`/`is_archived` column
/// exists in this codebase (checked directly); `last_active_at` older than
/// this many days is the stated, honest proxy, not a literal archive flag.
/// Checked before wake-row creation (`ai::scheduler::scheduler_tick`'s
/// post-completion hook) — a stale origin session degrades a wake to
/// report-only rather than waking a session nobody is looking at anymore.
///
/// **Annotated beside, #3151 PR2/C24 (v97) — the "no real archived flag
/// exists" sentence above is now literally false, just not for THIS
/// constant's own original consumer.** `agent_jobs.archived_at` (v97) is
/// a real archive column — but it's a separate, newer mechanism, on a
/// different table, reusing this same 30-day number by convention (see
/// `ai::scheduler::sweep_archivable_jobs`), never by sharing this
/// constant's call path. This paragraph is left as the historical record
/// of the gap #3136 found (this constant's first-ever use against
/// `agent_jobs`), not rewritten.
pub const RUNTIME_JOB_ARCHIVE_DAYS: u32 = 30;

/// §36.6.2: the eligibility window for background OC pickup — a
/// `chat_sessions` row with an OC-bound agent is eligible when
/// `last_active_at` (or, for job-origin sessions whose `last_active_at` is
/// frozen at creation, `agent_jobs.updated_at`) falls within this many days
/// of now. Read by `select_oc_pickup_eligible_sessions`'s cutoff query
/// (`ai::scheduler`, #3050 PR 7). Named here and in §36.10A's constants
/// table since PR 2's scope audit; PR 7 is the one that consumes it.
pub const RUNTIME_OC_PICKUP_WINDOW_DAYS: u32 = 7;

/// §36.6.2: the page size for background OC pickup's per-tick rotation —
/// up to this many eligible sessions are pulled in one scheduler tick, one
/// gateway call per session, before the rest wait for `next_oc_pickup_page`'s
/// next rotated page. Read by `run_oc_background_pickup` (`ai::scheduler`,
/// #3050 PR 7). Named here and in §36.10A's constants table since PR 2's
/// scope audit; PR 7 is the one that consumes it.
pub const RUNTIME_OC_PICKUP_SESSIONS_PER_TICK: usize = 20;

/// §36.9.3 (#3050 PR 9): `read_attachment`'s text-extraction character cap.
/// Char-indexed, not byte-indexed (`.chars().take(n)`, #3111's own
/// precedent) — a byte index can panic/split mid-codepoint on multi-byte
/// UTF-8 content. Consumed by `attachments::truncate_attachment_text`.
pub const RUNTIME_ATTACHMENT_TEXT_CAP: usize = 60_000;

/// Matt's Ruling 3 (2026-10-31, #3133/#3129 C28(b) follow-up): the
/// recurring/heartbeat failure ladder's strike threshold — a transport/
/// provider failure increments `agent_jobs.consecutive_failures`; at this
/// many CONSECUTIVE failures the job flips to a terminal `'failed'`
/// instead of staying `'pending'` for its next scheduled firing. A success
/// anywhere in the streak resets the counter to 0. Consumed by
/// `ai::scheduler::settle_recurring_job_outcome` — the one function that
/// reads it, per this file's own "named here, consumed by the PR that
/// needs it" convention.
pub const RUNTIME_MAX_CONSECUTIVE_FAILURES: i64 = 3;

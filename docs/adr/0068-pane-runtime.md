# ADR-0068: Pane Runtime — Scheduler, Jobs, Spawn and Wake

**Status:** Proposed
**Date:** 2026-10-25 (amended 2026-10-26)
**Author:** Matt Carano (decisions), Fable (draft)
**Supersedes:** The March 2026 §17 "agent spawning" outline (never built)
**Related:** #3050, #3059, #3058, #3041, #3076, ADR-0021, ADR-0062, ADR-0063, ADR-0064, spec §36

## Context

An API-runtime agent in Pane (Anthropic, OpenRouter, Venice, Ollama) can only do what fits in one user-started turn. OpenClaw supplies continuation, scheduling and spawning for OC agents; API agents have none of it. #3050 asks Pane to be its own runtime for those agents without changing how OC agents work.

Constraints that shaped the design:

- **OC must keep working exactly as it does.** Pane invokes only the nine whitelisted OC methods (§22C.7.1, ADR-0015) and never runs a loop for an OC-bound session.
- **Session binding is immutable** (ADR-0062). A different agent means a different session.
- **Append-in ≠ dispatch-out** (ADR-0062 amendment): an ingested message never starts a turn.
- **Mechanize over discipline** (playbook §0). Rules that depend on the model behaving are not rules.
- **Pane only needs to be open**, not always on, not focused (Matt, 2026-10-24).
- Two users today; the design must stay simple to upgrade.

## Decision

1. **One Pane-owned scheduler, one clock.** A 60-second tick (plus a launch pass) is the only thing that starts a turn nobody typed. Jobs are rows in a per-device `agent_jobs` table; the tick claims a row (`pending → running`) in one transaction before running it. Missed recurring runs collapse to one catch-up.

2. **A job is a turn on its own session.** Each job owns a `chat_sessions` row bound to the job's agent through the normal `create_session` path. Spawns (`origin = 'spawn'`) are hidden: never active, never listed; the result reaches the originating chat. Schedules (`origin = 'schedule'`) are named sessions listed only in Home's cross-project Scheduled panel (never in a project drawer) — because chat sessions get retired and a schedule's output needs a home that outlives them.

3. **Spawn targets are Pane agents.** `spawn_job(agent, brief)` may name any active `ai_agents` row, across providers; the user owns the privacy consequence when they pick the agent. OC agents are refused as targets (their spawns stay OC's). Depth is capped at 2.

4. **Report-back is an ingest; wake is a scheduled turn.** A finished job writes its result into the originating session through `ingest_external_messages` (which cannot dispatch), attributed to that session's own agent with `job_id` on the row and a Pane-written line naming which agent did the work. No exception to the binding check or the attribution invariant (Matt, 2026-10-26: one agent per session). Separately, the scheduler enqueues a proactive turn on that session so the originating agent can act on the result — the OC coordinator pattern, reproduced without letting ingest start turns. Chains are capped by depth and share the root job's tool-call ceiling.

5. **Proactive turns are gated by withholding, not instruction.** Scheduled, heartbeat, spawn, wake and continuation turns do not receive `delete_note`, `move_note` or `move_folder` (Rule 20's consent set); the dispatch gate refuses them if named. Navigation tools are offered but no-op. Web-taint protection applies unchanged. OC-dispatched turns are OC's responsibility.

6. **Auto-continue is in-loop, bounded by a user ceiling.** When a turn hits `max_tool_iterations` with tool calls still being produced, the loop continues itself up to `job_ceiling_tool_calls` (user setting, default 200), with a Stop button. Applies to interactive and background turns alike. The loop is first extracted into an `R: tauri::Runtime`-generic function (#3059) so this change lands under a real-loop test.

7. **OC gets the trigger, not the loop.** A scheduled job on an OC agent is a single `sessions.send` at the scheduled time; OC thinks; the reply arrives through background pickup. Pane records `sent`, never mirrors OC's own run status.

8. **Background pickup replaces the focus-gated poll.** All OC-bound sessions active within 7 days are pulled on the same tick, open or not. Messages into a closed session raise an `agent_message` notification.

9. **Attachment text is external content.** `read_attachment` output is delimited and tainted exactly like `web_fetch`. Vision support is read from the provider's live model list, never a hardcoded model list.

10. **Policy numbers live in one named block** (`runtime_constants.rs`); the two user-adjustable ones (`job_concurrency`, `job_ceiling_tool_calls`) are `ai_settings` keys seeded from it.

## Consequences

### Positive
- API agents get long jobs, schedules, delegation and proactive check-ins with no OC dependency.
- The OC boundary is a table in §36.0.1, not a judgment call per PR.
- Every "turn nobody typed" goes through one component, so the security surface of proactive action has one gate.
- The coordinator workflow Matt runs on OC (spawn → woken → next spawn → tell Matt) works on any provider.

### Negative
- A new table and four columns: a real migration (v92→v93) with fixture and smoke proof.
- Parallel jobs mean parallel API spend; the concurrency setting is the only brake.
- Job sessions accumulate invisibly until the 30-day archive.
- Pane agents still cannot run shell or git; "Stia in Pane" does Pane work, not PRs.

### Risks
- **Runaway chains or continuations** — mitigated by chain-depth cap, shared root ceiling, no-progress timeout and Stop; cost, not stability, is the exposure.
- **A future PR lets ingest dispatch** "for convenience" — the invariant in §36.11 and the test asserting no stream call from `ingest_external_messages` exist to catch it.
- **Tool-capability gating absent** (#3076): a spawn onto a model that cannot call tools fails loudly rather than silently; acceptable at two users, tracked.
- **SQLite contention** with two jobs plus an interactive turn — the loop already runs off the UI thread; if contention shows, drop `job_concurrency` to 1.

## Alternatives Considered

- **Report-back attributed to the subagent** (a scoped binding exception keyed on the job row). Rejected: honest, but it means a second agent identity inside a session; relaying through the session's agent with the subagent named in the content is simpler and keeps the invariant exception-free (Matt, 2026-10-26).
- **Let report-back dispatch directly** (ingest starts the origin turn). Simpler, but it breaks the append-in ≠ dispatch-out amendment and gives every message source a path to start turns. Rejected; the scheduler-wakes design keeps one starter.
- **All job sessions visible.** Rejected for spawns (clutter — Matt, 2026-10-25); accepted for schedules once the retired-chat problem surfaced (Matt, 2026-10-26).
- **Per-agent heartbeat setting.** Rejected: a heartbeat is a preset recurring job, not an agent property (Matt, 2026-10-26).
- **Pane manages OC cron.** OC's cron can only be disabled globally; Pane sending on a schedule achieves the same without touching OC config. Rejected.
- **Always-on daemon / launchd.** Rejected: Pane only needs to be open.
- **Email/Telegram as first channel.** Deferred; chat lives in Pane.
- **Do nothing.** API agents stay single-turn; the Venice privacy story (API-only) stays weaker than OC's. Rejected.

## References

- Related issues: #3050, #3059, #3058, #3055, #3041, #3076, #2988
- Spec section: §36 (`docs/spec/runtime.md`), §30.11, §19, §4.5, §4.6
- Related ADRs: ADR-0015, ADR-0021, ADR-0023, ADR-0062, ADR-0063, ADR-0064, ADR-0065

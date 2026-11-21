# ADR-0021: Event-Driven Notification Triggers and Coordinator-Only AI Nudges

**Status:** Accepted
**Accepted:** 2026-07-07
**Date:** 2026-05-02
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None

## Context

The notification system has been partially built (DB, OS native delivery, encrypted Supabase poll, JSONL fallback, badge) but the trigger architecture for new notification types — task due dates, AI nudges, sync events — was never specified and never built. The §11 audit (2026-05-01) confirmed: schema and read/write tools for due dates exist, no scheduler watches for due/overdue tasks; AI nudges are described in §0 vision language but have zero code; sync events are tracked for the sidebar status indicator but never wired to the notification system.

Three architectural questions are entangled inside the trigger design and need to be settled before implementation:

1. **What kicks the trigger?** A cron-like polling thread, an event-driven check on user activity, or OS-level scheduling.
2. **What stops a trigger from firing twice for the same condition?** Memoization in process state, idempotency keys in the DB, or "we'll just be careful" (discipline).
3. **Which agents can emit user-facing nudges?** All agents, with prompt-based restraint; all agents, with mechanical rate limits; or only specific agents, with schema-level enforcement.

All three questions have answers in adjacent subsystems by precedent that needs to be honored explicitly here, otherwise the implementation will reproduce known failure modes.

The consolidator (§5.5.3, designed in the 2026-04-23 memory architecture session) chose event-driven over cron with this reasoning, recorded verbatim in the design notes: *"No cron. Pane doesn't have cron infrastructure and building it is fragile (app closed = cron doesn't run). Triggered at app startup + on-threshold. Event-driven pattern matches §5.6.5."* This is the same decision the notification system needs to make about due-date checks, and it's the same decision for the same reasons. The OS may suspend or kill the app process; cron-style triggers in a desktop app's process space silently fail when the user closes the app and re-opens four hours later expecting a 9 AM reminder.

The discipline-vs-mechanism question has been answered three times by ADR-0008 (general principle), ADR-0016 (§4 enforcement), and ADR-0018 (CI smoke check). The pattern: every "developers will remember to X" rule eventually fails, gets caught in a postmortem, and gets mechanized. Notification-specific examples of this shape:

- "We'll remember to dismiss old notifications" — fails as the table grows. → mechanical: `expires_at` per-type TTL.
- "We'll dedupe duplicate due-date notifications by checking before emitting" — fails when two trigger paths race. → mechanical: idempotency key with DB-level dedup.
- "Sub-agents will be careful not to spam notifications" — fails the first time a sub-agent's prompt is buggy or prompt-injected. → mechanical: schema-level coordinator-only check.

The third question — whether scheduler firings need their own mutex against each other — is a refinement that the event-driven choice eliminates. Two `check_due_dates()` calls racing both produce the same idempotency key on the same row; the unique-index conflict on `idem_key` resolves the race deterministically without a lock. Cron would have made this a real race because two timer ticks could overlap.

Pane's actual operating model is single-coordinator: the user converses with one coordinator agent (Brit today), who delegates to sub-agents (Stia, Reed, Lane, etc.) and decides what's worth surfacing back to the user. Modeling that operating model at the schema layer — `is_coordinator` flag on `ai_agents`, tool dispatch gated on the flag — makes the right thing automatic. Sub-agents *cannot* emit notifications even if their prompts ask them to or a prompt-injection attempts it. The coordinator decides what reaches the user.

### What if we didn't decide

Three predictable failure modes:

- **Cron variant is built** — adds a thread-spawn at app start, ticks every 60s checking for due dates. Works fine when app is open. User who keeps Pane closed during the workday gets no reminders during the workday and a flood at 6 PM when they reopen. A bug report files this as "reminders are broken." The fix at that point is to retrofit event-driven triggers on top of cron and turn cron off, doubling the work.
- **No idempotency, "be careful" instead** — the due-date watcher fires once on app focus and once 5 minutes after start. If the user focuses the window during those 5 minutes (very common), both firings hit the same task and emit two `task_due` rows. The second emit is silently wrong. A bug report files this as "I got notified twice." The fix is to retrofit idempotency keys and backfill dedup logic.
- **All agents can emit nudges, with prompt-level restraint** — works for the agents Matt's team writes (cautious, well-tuned). Does not work the first time a third-party agent or a prompt-injection causes a sub-agent to call `emit_notification`. The notification panel becomes a chat firehose for a session and the surface loses its restraint property. The fix at that point is to add the coordinator gate retroactively, which is harder than adding it now because existing emitting code paths need to be audited.

## Decision

Notification triggers are event-driven, idempotent at the DB layer, and gated to coordinator agents for AI-originated nudges.

### Triggers are event-driven, no cron

Every trigger is invoked from a real event:

- **Due-date watcher** (§19.5.1) runs on app focus (debounced ≤1/60s), 5 minutes after app start, and after any `update_note` Tauri command that touches `due_date` or `status` (scoped fast path). The startup tick handles "user opens Pane after 8 hours of being closed." The focus tick handles "user comes back to the window during a long working session." The post-update tick handles "user just changed a due date and wants to see the consequence." No timer thread runs.
- **Sync events** (§19.5.2) emit from inside the sync layer at the point the failure is detected. No polling.
- **Coordinator AI nudges** (§19.5.3) emit from a Tauri tool the coordinator agent calls.
- **System events** (§19.5.4) emit from the existing checks (integrity, update, etc.) at the point of detection.

Startup-tick + on-event triggers cover the same ground cron would, with the property that *the user being absent does not produce silent failure* — when the user returns, the trigger fires.

### Idempotency keys with DB-level dedup

Every notification carries an optional `idem_key` column. Triggers that can fire multiple times for the same condition compute a deterministic key:

- Due date: `due:{task_id}:{threshold}` (e.g., `due:abc-123:1d`, `due:abc-123:overdue`).
- Sync failure: `sync_failed:{operation}:{first_failure_timestamp_minute}`.
- Coordinator AI nudges: caller-supplied or auto-generated (`nudge:{coordinator_name}:{hash(title)}`).

Before insert, the emitter does `SELECT 1 FROM notifications WHERE idem_key = ?`. If a row exists, the emit is a no-op. The lookup is indexed (`idx_notifications_idem`).

This is dedup at the application layer enforced by an indexed query, not a DB UNIQUE constraint, because the `notifications` table will not be a CRR table (§19.8 / ADR-0022) but the existing CRR tables in the schema can't carry UNIQUE constraints either (§22B precedent). The indexed query is functionally equivalent with one extra lookup.

### Coordinator-only AI nudges

The `emit_notification` AI tool is gated at the schema layer to agents with `is_coordinator = 1` in `ai_agents`. The check happens at the tool dispatcher: a non-coordinator agent calling the tool gets a tool error, returned to the AI per the §4.4 tool-error pattern (logged to console, returned to AI for graceful recovery, never shown raw in chat).

`is_coordinator` defaults to `0`. The seeded coordinator at install time is Brit; the per-pair migration arm flips Brit's row to `is_coordinator = 1` for existing installs and the agent-creation seed sets it for new installs.

Multi-coordinator support (e.g., per-project coordinators) is a flag-flip on additional `ai_agents` rows when that operating model is decided — not an architectural change.

> *On the question of whether to also add a per-source rate limit as a second layer:* not at launch. The coordinator gate restricts who can emit; the user trusts the coordinator they configured. If Brit emits too many notifications, that is a prompt-tuning issue with one specific agent, not a class of failure that warrants mechanical rate-limiting. If the operating model later expands to multi-coordinator or to less-trusted coordinators, a rate limit can be added then via a clarifying supersession ADR.

### Type registry as a Rust enum

`NotificationType` is a Rust enum at the application boundary; the DB column stays `TEXT` for forward-compat. Adding a type requires editing the enum + the channel-router match arm. Compile-time checks catch missing arms.

### Automatic expiry as the table-growth backstop

Each notification gets a type-specific default `expires_at`. The same event that runs `check_due_dates()` (app focus, 5min post-start) also runs a single SQL update to auto-dismiss expired rows:

```sql
UPDATE notifications SET dismissed_at = datetime('now')
WHERE dismissed_at IS NULL AND expires_at < datetime('now');
```

This is the mechanical backstop that prevents the panel from becoming useless over time without requiring user discipline.

## Consequences

### Positive

- **Reuses the consolidator pattern** (§5.5.3, ADR-0010 design notes) instead of inventing a new scheduler architecture for an adjacent problem. Two subsystems with the same answer to the same question is cheaper than two subsystems with two different answers.
- **App-closed periods don't produce silent failure.** A user who opens Pane once a day at 6 PM gets all relevant notifications at 6 PM, including the "task was due at 9 AM" one. Cron would have fired silently into a closed app.
- **Race-free by construction.** Two trigger events firing the same `check_due_dates()` produce the same idempotency keys. The first emit succeeds; the second's lookup finds the row and no-ops. No mutex needed.
- **The notification surface stays scarce.** Sub-agents cannot emit; the user sees only what the coordinator decides to surface. Models the actual operating relationship (user ↔ coordinator ↔ specialists) in the schema, so behavior matches intent without prompt discipline.
- **Type registry catches drift.** Adding a notification type and forgetting to update the channel router is a compile error, not a runtime "silently dropped notification" bug.
- **Automatic expiry prevents panel rot.** The notification surface stays focused on currently-relevant items without manual pruning.
- **Multi-coordinator is a flag-flip.** If the operating model expands (per-project coordinators, multi-coordinator workspaces), the schema already supports it — set `is_coordinator = 1` on the additional rows. No retroactive code changes.

### Negative

- **No precise timing.** A task due at 9 AM with a 1-day-before threshold reminder fires the next time the user opens Pane after 9 AM the day before, not at 9 AM the day before. Mitigation: the threshold semantics are "at-or-after," not "at exactly." Documented in §19.5.1.
- **Idempotency key design is per-trigger.** Each new trigger needs its key designed once. Mitigation: the launch-scope triggers' keys are spec'd in this ADR; future ones inherit the pattern.
- **Coordinator-only means sub-agents can't surface urgent results without going through the coordinator.** Mitigation: the actual urgency cases at launch (single coordinator: Brit) all naturally route through the coordinator anyway; the coordinator is the agent the user converses with. Sub-agents that complete background work return to the coordinator, who decides whether to nudge.
- **Notification-emit path adds a SELECT before INSERT for dedup.** Two queries instead of one per emit. Mitigation: indexed; cost is sub-millisecond at notification-table scale.

### Risks

- **Idempotency key collisions across trigger types.** A cleverly named idem_key could collide across types if the prefix isn't disciplined. Mitigation: keys carry the type prefix (`due:`, `sync_failed:`, `nudge:`); plus a unit test that asserts no two trigger code paths emit the same key prefix.
- **A new trigger type is added without an idempotency key.** The dedup path is bypassed. Same spec-vs-implementation drift risk as PR #690. Mitigation: the `emit_notification` Rust function should *require* an idem_key parameter via the type system — `Option<String>` is fine, but callers have to consciously pass `None`. A lint or AST grep in the §4 enforcement layer (ADR-0016) can flag `emit_notification(...)` calls without an explicit idem_key argument.
- **Coordinator agent compromise.** If Brit's prompt gets injected, all the protection at the sub-agent layer is moot — the attacker is now the coordinator. Mitigation: the coordinator gate doesn't pretend to defend against compromise of the coordinator itself; it defends against the much larger surface of *every* sub-agent being a potential emit point. Coordinator hardening is a separate concern (prompt-injection defense in chat context, etc.) that isn't notification-specific.
- **"Coordinator" becomes overloaded.** The flag is named for the notification-emit role, but it may pick up other meanings over time (which agent runs daily logs? which agent owns memory router?). Mitigation: at launch the flag is single-purpose. If other roles need agent-level flags, they get their own columns; we don't pile semantics onto `is_coordinator`.
- **Expiry race with read.** User reads a notification, the expiry sweep runs, the read state is preserved on the dismissed row but the panel removes it before the user finishes reading. Mitigation: dismissed rows stay in DB (soft-delete); panel updates are reactive on event, not on a scheduled refresh. The user sees the row at the moment of click.

## Alternatives Considered

**Cron-like timer thread.** A `tokio::spawn` loop that ticks every 60s and runs `check_due_dates()` regardless of app focus. **Rejected** because the consolidator already chose event-driven for the same reasons (app-closed = silent failure, OS-suspend = silent failure on macOS in particular). Building two scheduler architectures in the same app — one for memory consolidation, one for notifications — has no upside and creates two surfaces to maintain.

**OS-level scheduling (launchd / systemd timers).** Pane registers system timers that fire even when the app is closed. **Rejected** because it requires platform-specific code paths, daemon registration on first run, permission prompts, and an entire architecture for "the daemon can't access the app's DB unless we expose an IPC surface, but the daemon also can't show a notification with the app's name unless..." Long thread of complications for a feature that event-driven covers adequately.

**Per-source rate limits as the restraint mechanism.** Allow every agent to emit; cap per-source counts at the DB layer. **Rejected.** Rate limits solve a problem that doesn't exist at launch (Pane has one coordinator emitting; there is no "too many sources to limit per-source"). Coordinator-only is simpler, models the actual operating relationship, and is the schema-level enforcement the discipline-vs-mechanism principle asks for. If multi-coordinator deployments later hit per-coordinator rate-limit needs, file a clarifying supersession ADR with the actual incident driving the change.

**Discipline-only restraint, no schema check.** Trust agent prompts to be careful. Trust the coordinator's prompt; trust sub-agent prompts to never call the tool. **Rejected** explicitly per the ADR-0008 / ADR-0016 / ADR-0018 line: every discipline-only rule that *can* fail eventually does, and the cost of mechanizing later is greater than mechanizing now. The coordinator-only gate is the smallest possible mechanism that covers the largest category of failure (any sub-agent being able to emit).

**Idempotency via a UNIQUE constraint instead of a SELECT-before-INSERT.** **Rejected** because the `notifications` table will need to coexist with patterns where the same key needs to be re-issued in legitimate cases (e.g., a task whose due date is removed and re-added produces the same idem_key by design — we want the second one to fire). A SELECT-before-INSERT lets the application layer make that decision; a UNIQUE constraint rules it out at the DB layer.

**Type-by-type emitter functions instead of a unified `emit_notification` enum-driven path.** **Rejected** because the channel router, panel renderer, and persistence layer all care about type. Making type a parameter of one function instead of a function-name-prefix means the router stays in one place. ADR-0011's read-only check used the same shape (one check, switching on `note_type` enum) and it has held up.

## References

- Companion ADR: ADR-0022 (Notification State is Per-Device). Filed alongside this ADR.
- Spec section: `docs/spec/notifications.md` (this is the section the ADR locks the architecture for).
- Pattern precedent: §5.5.3 consolidator triggers (Memory Architecture Decisions, 2026-04-23). The "no cron" reasoning ports forward unchanged.
- Pattern precedent: ADR-0010 (Three-Writer Memory Pipeline). Same event-driven shape.
- Pattern precedent: ADR-0008 (Mechanical Dev Hygiene Over Discipline-Based Rules). The restraint-via-mechanism principle.
- Pattern precedent: ADR-0016, ADR-0018. Repeated proof that discipline floors leak.
- Pattern precedent: ADR-0011 (System Notes Read-Only). Same shape of "schema-level enforcement of an operating constraint."
- Audit: `docs/audits/2026-05-01-notifications-audit.md` (Brit). Source of the orphaned-trigger findings.
- Issue: notifications backend audit (closed; replaced #146).
- Issue: notifications UI surface (open; this ADR + `docs/spec/notifications.md` unblock it).
- Issue: #822 (broken left-menu Notifications link returns to home; bug independent of this ADR).

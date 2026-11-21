# Pane Notifications & Reminders — §19

> **Status:** PARTIAL — core delivery BUILT (DB → Tauri event → OS notification, encrypted Supabase poll, JSONL watcher fallback, badge). Notification list UI, due-date scheduler, sync-event wiring, AI nudges, email channel NOT BUILT. Spec'd 2026-03-17 (outline), full body authored 2026-05-02 per ADR-0021 and ADR-0022.

> **Section ownership:** This file is the canonical spec for §19. README.md §19 carries a one-line summary and a pointer here. All cross-references in code, ADRs, and other spec files use `§19` (or `§19.N`) and resolve to this file.

## §19.0 Purpose

The notification system is how Pane tells the user something they need to know without pulling them into chat. It surfaces the small set of things that are time-sensitive enough to interrupt — a task is due, sync failed, a coordinator agent has something worth surfacing — and presents them in one durable, dismissible place. Everything else stays in chat or in the workspace itself.

The system is bounded by a restraint principle: **notifications are scarce by design.** Every channel, type, and trigger is filtered through "is this worth interrupting the user." When in doubt, the system writes to chat or to a daily log entry, not to a notification. A noisy notification surface is functionally equivalent to no notification surface, because users learn to dismiss without reading. This is treated as a design constraint, not a guideline.

Restraint is enforced by mechanism, not discipline. The two mechanisms are (a) the coordinator-only check on AI nudges (§19.5.3), which restricts who can emit user-facing interruptions to a single agent role; and (b) automatic per-type expiry (§19.4), which keeps the panel from accumulating stale items.

## §19.1 Architecture overview

```
                    ┌──────────────────────────────────┐
                    │  TRIGGERS (event-driven only)    │
                    │  • Due-date watcher (§19.5.1)    │
                    │  • Sync events (§19.5.2)         │
                    │  • Coordinator AI nudge (§19.5.3)│
                    │  • System events (§19.5.4)       │
                    │  • External: JSONL, Supabase poll │
                    └────────────────┬─────────────────┘
                                     │
                                     ▼
                    ┌──────────────────────────────────┐
                    │  emit_notification(type, ...)    │
                    │  • idempotency dedup (§19.5)     │
                    │  • INSERT notifications row      │
                    │  • emit "pane-notification"      │
                    │  • OS native (window unfocused)  │
                    └────────────────┬─────────────────┘
                                     │
                                     ▼
                    ┌──────────────────────────────────┐
                    │  CHANNELS (§19.3)                │
                    │  • In-app: badge + panel + toast │
                    │  • OS native (built)             │
                    │  • Push: encrypted Supabase      │
                    │  • Email: post-launch            │
                    └──────────────────────────────────┘
```

The center of the system is the `notifications` table and the `emit_notification(...)` Rust function (today: `process_notification` in `src-tauri/src/notifications.rs`). All triggers — internal and external — funnel through that function. Channels read from the same DB row. There is exactly one writer to a notification row's lifecycle (create, mark-read, dismiss); there is no mechanism that mutates rows out-of-band.

## §19.2 Notification types

Every notification carries a `type` (one of the values below) and a `source` (free-form, identifies the producer; for AI nudges, this is always the coordinator agent's name — see §19.5.3). The type is the structural enum; the source is the audit trail.

| Type | Trigger | Default channels | Launch scope |
|---|---|---|---|
| `task_due` | Due-date watcher (§19.5.1) crosses a configured threshold | in-app + OS | **Launch** |
| `task_overdue` | Due-date watcher detects past-due undone task | in-app + OS | **Launch** |
| `sync_failed` | Sync layer returns terminal failure (max retries exceeded) | in-app + OS | **Launch** |
| `ai_nudge` | Coordinator agent calls `emit_notification` tool (§19.5.3) | in-app only by default; OS opt-in per nudge | Launch (in-app); OS opt-in launch+1 |
| `agent_done` | Spawned background agent (§17) completes; coordinator decides whether to surface as nudge or stay in chat | in-app | Launch+1 (depends on §17 spawning surface) |
| `agent_message` | §36.7 (#3050 PR 6, **BUILT**): a job session's report-back (§36.8) or an OC background-pickup message (§36.6) lands in a session that is not open | in-app + OS | **Built** |
| `job_failed` | §36.7 (#3050 PR 6, **BUILT**, Ruling 1 — Matt 2026-10-29): a job records a failure. For `once`/`spawn`, the single failure IS the terminal exit — one row. For `recurring`/`heartbeat`, EVERY firing that records a failure (not only the 3rd-strike terminal flip) updates one row per failure **streak**, keyed on `job_id` + the streak's first `last_failed_at` — first failure inserts, each further failure in the same streak updates the same row in place, a success ends the streak (next failure starts a new row) | in-app + OS | **Built** |
| `system_event` | App-level: update available, license expiring, integrity check failure, low disk | in-app + OS (severity-gated) | Launch (subset: update, integrity) |
| `info` | Catch-all for legacy/test paths (current `default_type`) | in-app | **Built** (legacy, do not extend) |

> Status: the `agent_done` row above is SUPERSEDED by `agent_message` (§36.7)

**Corrected (Ruling 5, Matt 2026-10-29 — #3050 PR 6; fifth spec/code divergence this sprint, same discipline as §36.11/Ruling 1): the paragraph below, as it previously read, asserted a `NotificationType` Rust enum that has never existed.** `grep -rn "NotificationType" src-tauri/src/` returns zero hits. `notifications.rs`'s `insert_notification`/`process_notification` take `notif_type: &str` — a plain string, no `CHECK` constraint on the `type`/`target_kind` columns. Every type in the table above, including the ones marked "Built" long before this correction (`task_due`, `ai_nudge`, `note`, `subtask`, …), has always been a plain string, not an enum variant. **Fixed to describe the actual pattern**, matching `refresh_event.rs`'s precedent (`ENTITY_NOTE`/`ENTITY_SHEET` named `&str` constants): adding a type means adding a named `&str` constant (`notifications.rs`'s `NOTIF_TYPE_*` constants, e.g. `NOTIF_TYPE_AGENT_MESSAGE`/`NOTIF_TYPE_JOB_FAILED`) and the corresponding match arms in the panel renderer and channel router. **No enum is being built — this is a documentation fix, not a deferred feature.** `docs/INVARIANTS.md` §19 carried the identical incorrect claim and received the same correction in this PR.

> **📌 Why This Exists — type as enum, not string (2026-05-02, ADR-0021):** The current schema stores `type` as `TEXT` with default `'info'` and accepts any string. This is fine for the small set built today, but it means every consumer that branches on type — channel routing, panel rendering, future filters — has to either accept arbitrary strings (no typo protection) or duplicate a list of valid values. Lifting the type to a Rust enum at the boundary, with a single conversion at DB read, gives the type system the shape it already has in the spec. The DB column stays `TEXT` for forward-compat with future types added without schema bump; the application enum gates which values land in it.

## §19.3 Channels

Four channels exist or are planned. Each notification can travel through one or more, determined by type defaults plus user preferences (§19.3.5).

### §19.3.1 In-app

Three surfaces, same data:

- **Badge** (built — `Sidebar.svelte`): unread count next to the bell icon. Counts unread, undismissed notifications.
- **Panel** (NOT BUILT, launch-blocking): clickable bell opens a floating panel that overlays from the sidebar and dismisses on outside-click. Lists notifications newest-first. Each row: type icon, title, body preview, timestamp, dismiss action. Click a row → navigate to the linked target (task, note, settings page) and mark read. Empty state: "No notifications. You'll see things here when something needs your attention." A "See all" link inside the panel opens a future page-mode view (post-launch affordance).
- **Toast** (NOT BUILT, post-launch): brief in-app overlay when a notification arrives while the window is focused. Auto-dismisses. Click → opens the panel scrolled to that item.

The `pane-notification` Tauri event is already emitted on every new notification (`process_notification` in `notifications.rs`). The panel and toast subscribe to that event for live updates without re-querying.

### §19.3.2 OS native (built)

`tauri-plugin-notification` shows the system notification. Already wired: only fires when the window is unfocused (`is_focused()` check in `process_notification`). This is correct behavior — focused window already sees the toast (when built) and badge update.

### §19.3.3 Push (encrypted, Supabase) (built)

`start_supabase_notification_poller` polls `<SUPABASE_URL>/rest/v1/notifications` every 30s, decrypts with the `pane-notifications-v1` sub-key derived from the master sync key, and feeds into `process_notification`. After processing, marks the Supabase row `read=true` to prevent re-delivery.

This is **inbound only** today — Pane *receives* push notifications from itself on other devices via the encrypted relay. The `push_notification` Tauri command (built) is the *outbound* path: a caller hits this command on their Pane, which encrypts and inserts into Supabase, which the user's other Pane(s) poll.

> **Polling, not realtime.** The earlier design used Supabase Realtime websockets; current implementation uses 30s REST polling. This is intentional and locked: realtime had reconnect storms after sleep/wake on macOS. Polling is bounded, predictable, and the 30s latency is acceptable for the kinds of notifications that round-trip through Supabase.

### §19.3.4 Email (NOT BUILT)

Reserved channel for post-launch. The Mar 17 changelog entry that mentions "3 channels — in-app, push, email" is aspirational; no email code exists. Email is the only channel that crosses out of the encrypted-by-default boundary, so it requires:

- Per-user opt-in (default off).
- Per-type opt-in (e.g., a user might enable email only for `sync_failed`).
- Sender domain owned by Anthropic (Pane Cloud only — Tier 2/3 BYOK and self-hosted users are not in scope for email at launch+1; they can wire their own outbound transport via a webhook channel if needed).
- Plaintext content trade-off: email content cannot be E2E-encrypted to user's master key without breaking the recipient experience. Email subjects/bodies must be designed around this — they should announce *that* something happened without revealing details a sender wouldn't put in plaintext.

### §19.3.5 Channel preferences

Per-type preference table (§19.9) with defaults from the §19.2 table. Settings UI: a grid (rows = type, columns = channel, cells = on/off, with grayed cells where the channel isn't applicable). Power users can route per-type; default users never see this — defaults work.

## §19.4 Persistence model

Each notification is a row in `notifications` with the lifecycle:

```
created  →  read       →  dismissed
            (optional)     (terminal)
```

- **Read:** user has acknowledged the notification. Set when the user clicks into the panel, clicks a notification, or hits "mark all read." Counts no longer decrement the badge.
- **Dismissed:** user has actively removed the notification from the list. Can happen with or without prior read. Dismissed rows don't appear in the panel.
- **Expired:** a row that has hit `expires_at` (optional column). Expiry is type-specific: `sync_failed` after 24h, `task_due` when the linked task hits `done` or its `due_date` is removed, `task_overdue` similarly, `ai_nudge` after 7 days, `agent_done` after 24h. System events don't expire automatically.

Expired rows are auto-dismissed by the same scheduler that runs the due-date watcher (one tick per app-focus, see §19.5.1) — no separate background loop. **Expiry is the mechanical backstop that prevents the notifications table from accumulating unboundedly without requiring user discipline.**

> **📌 Why This Exists — automatic expiry (2026-05-02, ADR-0021):** A discipline-only "remember to dismiss old notifications" rule fails the same way every discipline-only rule has failed in this codebase (ADR-0008, ADR-0016, ADR-0018). The notification table grows, the panel becomes noisy, the user starts ignoring it, the system loses its purpose. Type-specific automatic expiry pushes cleanup into the schema layer where it can't be forgotten. Notifications the user *wants* to keep visible are just notifications they haven't dismissed yet; expiry's terminal action is auto-dismiss, not delete, so audit trail is preserved.

Dismissed and expired rows are **kept** in the table (soft-delete via `dismissed_at IS NOT NULL`), not physically deleted, for two reasons: (a) sync round-trips can race with dismiss (§19.8 covers this); (b) notifications form a debugging audit trail. The `get_notifications` command filters on `dismissed_at IS NULL` for the panel; a future "show dismissed" toggle is trivial.

## §19.5 Trigger architecture

All triggers are event-driven. **No cron, no always-on scheduler thread.** The pattern is: an event causes a check; the check enqueues notifications if conditions are met; idempotency keys prevent duplicates across multiple firings.

> **📌 Why This Exists — event-driven, no cron (2026-05-02, ADR-0021):** This follows the consolidator precedent (§5.5.3, ADR-0010 design notes 2026-04-23): *"No cron. Pane doesn't have cron infrastructure and building it is fragile (app closed = cron doesn't run)."* A user reminder for a 9 AM task that fires at 11 AM (when the user opens the app) is correct; a user reminder that's silently lost because the OS killed the app's background process at 8:55 AM is the failure mode cron introduces. Event-driven triggers are bounded, debuggable, and tier-agnostic.

### §19.5.1 Due-date watcher

The watcher is a function `check_due_dates()` that:

1. Queries `SELECT id, title, due_date FROM notes WHERE due_date IS NOT NULL AND status != 'done' AND deleted_at IS NULL`.
2. For each row, classifies relative to `now()`: *overdue*, *due-today*, *due-tomorrow*, *due-in-N-days* where N comes from per-task or global preferences (default: 1 day).
3. For each classification → `(task_id, threshold)` pair, computes an idempotency key (e.g., `due:{task_id}:overdue`, `due:{task_id}:1d`).
4. If no notification with that idempotency key exists in the `notifications` table → emit one. If one exists (active or dismissed) → skip. Dismissed rows act as tombstones: a user-dismissed notification for the current due window must not re-fire on focus/reload. The companion clear (step 5b) ensures legitimate re-fire by deleting prior rows when a task's status or due_date changes.
5. For tasks now `done` or with cleared `due_date`, marks corresponding open notifications as auto-dismissed (the linked task is no longer relevant).
5b. **Companion clear (§19.5.1a):** When `update_note` detects a status change from `done` to `todo`/`in_progress`, or a `due_date` change, it DELETEs prior notification rows for that `target_id`. This allows step 4's dedup check to pass on the next `check_due_dates_for_task` call, emitting a fresh notification for the new state. The DELETE (not UPDATE) is load-bearing: any surviving row with the same `idem_key` would block re-emit.

`check_due_dates()` runs:

- At app focus (debounced — at most once per 60s).
- 5 minutes after app start.
- After any `update_note` Tauri command that touches `due_date` or `status` (scoped to the affected task only — fast path, no full scan).

No timer thread. No background loop. The function takes ~10ms on a small task list and remains fast at thousands of tasks because the index `idx_notes_due_date` already exists.

### §19.5.1b Subtask due-date scan (extends §19.5.1)

> **Status:** TARGET — NOT BUILT
> **ADRs:** ADR-0057 (subtasks). Consistent with ADR-0021 (event-driven, idem-key dedup) and ADR-0022 (per-device state) — neither is contradicted, so no notification-architecture ADR is filed. ADR-0057 covers the data decision only.
> **What this adds:** subtasks (§31.3, core-data-model §1.9) are the only new emitter. They reuse the existing types, channels, dedup, and routing — extending §19.5.1, not replacing it.

**§19.2 Types — clarifying note (no new type):**

Subtask due dates fire the **existing** `task_due` and `task_overdue` types. No new type is added.

> **📌 Why This Exists — reuse the type, distinguish by target (2026-07-18, ADR-0057):** The type enum drives the channel router and panel renderer (§19.2, ADR-0021). A subtask-due notification wants the same channels and the same row rendering as a task-due notification — only its click target and its drawer label differ. That is a `target_kind` distinction, not a type distinction. Adding a `subtask_due` type would fork the router/renderer for no behavioral gain. The type stays `task_due`/`task_overdue`; `target_kind='subtask'` carries the difference.

`target_kind` gains a value: **`subtask`** (alongside `note | project | agent | url`). `target_kind` is unconstrained nullable `TEXT` in the live schema (`create_system_tables`, migrations.rs) — adding this value is a routing-enum + convention change with **no migration**.

**Scan extension:**

`check_due_dates()` gains a second scan over `subtasks`, run in the same event ticks as the notes scan (app focus, ~5 min after start, and — scoped — after any subtask write that touches `due_date` or `done`):

1. Query due, undone subtasks under live parents:

  ```sql
  SELECT s.id, s.text, s.due_date, s.note_id
  FROM subtasks s
  JOIN notes n ON n.id = s.note_id
  WHERE s.due_date IS NOT NULL
  AND s.done = 0
  AND n.deleted_at IS NULL;
  ```

  (Fast on the `idx_subtasks_due_date` partial index. Parent `status` is irrelevant — a subtask tracks its own `done`; only a trashed parent silences it.)

2. Classify each `due_date` relative to `now()`: overdue / due-today / due-tomorrow / due-in-N-days, same thresholds as notes.

3. For each `(subtask_id, threshold)`, compute idem_key **`subtask_due:{subtask_id}:{threshold}`** — e.g. `subtask_due:abc-123:overdue`, `subtask_due:abc-123:today`. The `subtask_due:` prefix is distinct from the note prefix `due:`, so a subtask and a note can never collide on idem_key.

4. SELECT-before-INSERT on idem_key (indexed `idx_notifications_idem`, non-unique): if a row with that idem_key exists (active or dismissed) → skip; else emit with `type` = `task_due`/`task_overdue`, `target_kind='subtask'`, `target_id = subtask_id`. Dismissed rows act as tombstones exactly as in §19.5.1.

Emit is **local-emit per device** (same row in §19.8.2's table as `task_due`: the watcher runs locally on each device; the Supabase relay is not used for subtask-due).

### §19.5.1c Subtask companion-clear and auto-dismiss (extends §19.5.1a)

Mirrors the note lifecycle (§19.5.1 step 5 + §19.5.1a) for subtask writes and parent-note deletion:

- **Subtask `done` 0→1, or `due_date` cleared** → auto-dismiss (`dismissed_at`) open notifications for that subtask. Terminal: the item is no longer relevant.
- **Subtask `due_date` changed, or `done` 1→0** → companion `DELETE` of prior notification rows `WHERE target_kind='subtask' AND target_id = {subtask_id}`, so step-4 dedup passes on the next scan and a fresh notification emits for the new state. `DELETE` (not `UPDATE`) is load-bearing — a surviving row with the same idem_key would block re-emit.
- **Subtask deleted** → companion `DELETE` of its notification rows.
- **Parent note trashed** (`notes.deleted_at` set) → in the same write tick, auto-dismiss open notifications for that note's subtasks. The §19.5.1b scan already excludes them (join on `n.deleted_at IS NULL`), so none re-emit while trashed.
- **Parent note restored** → companion `DELETE` of those subtasks' notification rows so the scan re-emits fresh.
- **Parent note hard-deleted** → `DELETE` the note's subtasks' notification rows (alongside the cascade hard-delete of the subtasks themselves, core-data-model §1.9.4).

### §19.5.1d Routing and drawer label (extends the §19 note-routing arm)

- **Routing arm for `target_kind='subtask'`:** resolve the parent via `SELECT note_id FROM subtasks WHERE id = {target_id}`, open that note, and highlight/scroll to the subtask line item in the checklist component. If the subtask no longer exists (deleted between emit and click), fall back to a no-op with the row already companion-cleared.
- **Drawer label:** the panel renderer distinguishes a `target_kind='subtask'` row from a task row — it is marked as a subtask and shows the parent note's title for context. This is the drawer-side of the same differentiation Due Today applies (§31.2.1). Label selection keys off `target_kind`, not off the message text.

**Cross-references:**

- Component spec: §31.3 (Advanced Checklist), §31.2 (Due Today).
- Data model: core-data-model §1.9 (`subtasks` table, deletion/cascade).
- Prerequisite/coordination: the AI-set-due-date-doesn't-light-the-bell bug (2026-07-15) is in this subsystem. A subtask due date set by the AI must trigger the scoped §19.5.1b scan the same way a user-set one does — it rides that fix. Not fixed here; sequenced in session notes.

### §19.5.2 Sync events

The sync layer (`sync_client.rs`, the gateway client) is wired to call `emit_notification(...)` directly when:

- A push fails after `MAX_RETRIES` exceeded → `sync_failed` notification.

This is a wiring change, not new infrastructure. The sync layer already tracks failure state (`src/lib/sync.ts` has `notifySyncStatus(...)` for the sidebar status indicator); this section spec'd the missing call into the notification system.

> **Conflict notifications are not in scope.** All sync conflicts today auto-resolve via LWW + content-identity per §22B.3.5 and INVARIANT #14. No `sync_conflict` notification type exists. If a future change introduces non-LWW conflict cases (e.g., structured collaborative edits), the type can be added then via a clarifying supersession ADR. Speculative reservation creates the same kind of three-eras drift ADR-0020 was filed to fix.

### §19.5.3 Coordinator AI nudge tool

A new AI tool `emit_notification` exposed in §4. Schema:

```typescript
{
  type: "ai_nudge",
  title: string,        // <= 80 chars
  body: string,         // <= 400 chars
  source: string,       // coordinator agent name (always)
  target?: {            // optional click destination
    kind: "note" | "project" | "agent" | "url",
    id: string
  },
  channels?: ("in_app" | "os")[],  // defaults to in-app only
  expires_at?: string   // ISO 8601, defaults to now + 7d
}
```

**Coordinator-only enforcement.** The tool is gated at the schema layer: `emit_notification` is only resolvable for an AI agent whose `ai_agents` row has `is_coordinator = 1`. Sub-agents that Brit (or whichever agent holds the coordinator role) spawns surface their results back through the coordinator, not directly to the user. The check happens in the tool dispatcher — a non-coordinator agent calling `emit_notification` gets a tool error, which the AI handles per the existing tool-error pattern (§4.4, errors returned to the AI for graceful recovery, never shown raw in chat).

Default `is_coordinator` is `0` for all agents. Coordinator status is set explicitly when an agent is configured as the team coordinator. Today this is Brit; in single-coordinator deployments it stays a single agent.

> **📌 Why This Exists — coordinator-only AI nudges (2026-05-02, ADR-0021):** The user-facing notification surface is an attention budget. Allowing every agent to emit notifications turns the panel into a chat firehose of agent activity, which destroys the surface's value (cf. the restraint principle in §19.0). Pane's actual operating model is single-coordinator: the user converses with one coordinator agent, who delegates to specialists and decides what's worth surfacing back. Modeling that explicitly at the schema layer makes the right thing automatic — sub-agents *cannot* emit notifications even if their prompts ask them to or a prompt-injection attempts it. Multi-coordinator support (e.g., per-project coordinators) is a flag-flip on additional `ai_agents` rows, not an architectural change.

### §19.5.4 System events

System-level emitters call `emit_notification(...)` for:

- Update available (auto-update infra is post-launch — placeholder type)
- Integrity check failure (existing `verify_agents_integrity` path)
- License expiring (Tier 1 hosted, post-launch)
- Low disk space (post-launch)

Built today: integrity-failure path can write a system event via the same JSONL watcher fallback. Restructure into a direct `emit_notification(...)` call as part of the implementation work.

## §19.6 UI surface

### §19.6.1 Bell + panel

The bell icon in `Sidebar.svelte` currently navigates to `view: "home"` and calls `mark_all_notifications_read` (which zeroes the badge optimistically). This is the bug filed as #822. **Replace with:**

- Bell click → toggle a floating notification panel that overlays from the sidebar and dismisses on outside-click.
- The panel reads from `get_notifications(unread_only: false)`, displays newest-first, virtualizes for >100 items.
- "Mark all read" is a button inside the panel, not a side effect of clicking the bell.
- A pin/badge stays on the bell while the panel is open if there are still unread items.
- A "See all" link inside the panel reserves space for a future page-mode view (post-launch).

### §19.6.2 Notification row

Each row in the panel:

```
┌──────────────────────────────────────────────┐
│ [icon]  Title                          12:34 │
│         Body preview, two lines max.         │
│         #source-tag                  [×]     │
└──────────────────────────────────────────────┘
```

- **Click row** → navigate to `target` (task, note, agent, project) AND mark the row read. When `target_kind` is `note` and the caller does not supply `projectId`, the navigation layer auto-resolves the note's project via `get_note` before setting the canvas state — ensuring the note opens in its correct project context regardless of the user's current view.
- **× button** → dismiss (terminal). Click is a single action, no confirmation.
- **Long press / right click** → context menu: "Mark unread", "Snooze 1h" (post-launch), "Mute this source" (post-launch).
- **Empty state** → short message + an empty-list illustration; NOT an exclamation point about "no notifications" being wrong.

### §19.6.3 Toast (post-launch)

A brief overlay appears in the lower-right when a notification fires while the window is focused. Auto-dismiss after 5s. Clicking the toast opens the panel scrolled to that item. Toasts respect type-level "show toast" preference; default on for `task_due`, `task_overdue`, `sync_failed`; default off for `ai_nudge` and `info`.

## §19.7 Topology behavior

Notification delivery has two flavors per topology: **local emit** (something happens on this device, notify the user *here*) and **cross-device emit** (something happens on device A, the user is currently on device B, notify there).

| Topology | Local emit | Cross-device emit |
|---|---|---|
| Tier 1 (hosted) | Direct: server-side trigger → push to user's active device(s) via encrypted Supabase. | Same path; server is the source. |
| Tier 2 (BYOK, single-machine) | Direct: trigger → `emit_notification` → DB → UI. | N/A (single device). Cross-device only happens if the user has multiple Tier 2 installs sharing sync. Then: Supabase relay (already built). |
| Tier 3 same-machine | Direct: triggers fire on Pane process. OC-side emitters (e.g., agent done notifications) write to `~/Shared/ExampleApp/notifications.jsonl` which Pane's watcher picks up. | N/A. |
| Tier 3 split-machine | Local (Pane side): direct. Local (OC side): **GAP — no path today.** | Same gap. The gateway does not relay user-facing notifications. |

The split-machine OC→Pane gap is real and is documented as a known shortfall in §19.10. Three options to close it (in order of architectural cleanliness):

1. **Gateway endpoint `POST /v1/notifications`** — OC-side coordinator agent calls the gateway, gateway forwards to Pane via the existing SSE stream or via the Supabase relay. This adds a 12th method to the §22C.7.1 whitelist and is an INVARIANT change.
2. **OC writes to a synced JSONL on a path the gateway also writes to** — fragile, recreates `~/Shared/ExampleApp/` semantics across machines, conflicts with the gateway's transport-only role.
3. **OC publishes via the user's Supabase relay using credentials shared at pairing time** — clean from a topology perspective but introduces a new credential-flow.

Recommendation: option 1, post-launch. At launch, OC-side coordinator agents on split-machine surface notifications via the chat stream (which already works) instead of the notification panel. This is acceptable because the launch user base for split-machine Tier 3 is single-digit (Matt + Brit's setup), and the pattern is fixable without architectural debt — just one new whitelisted method.

## §19.8 Read/dismiss sync

Two questions the spec must answer:

- **If a notification is created on device A, and the user reads it on device A, does device B see the read state?**
- **If the user dismisses on B, does the notification still appear on A's panel until A independently dismisses it?**

### §19.8.1 Decision: per-device read state, sync via content-identity

The `notifications` table is **NOT a CRR table.** Each device has its own notifications and its own read/dismiss state. Notifications that originate from cross-device sync (the Supabase push path) are inserted on each device independently when the polling pass picks them up. The Supabase row's primary key is the cross-device identity; on Supabase, `read=true` is set per-row after the *first* Pane consumes it (current behavior).

Device-level read/dismiss state stays on the device. If the user reads a `task_due` notification on phone, the desktop's `task_due` notification stays unread until the desktop user reads it independently.

Full rationale and alternatives in ADR-0022.

### §19.8.2 Cross-device emit identity

Notifications that *originate* on one device and need to *reach* another (e.g., `task_due` for a task whose due-date triggered while user is on phone) deduplicate via an idempotency key:

- Local triggers compute the idempotency key from `(type, target_id, threshold)` — e.g., `due:abc-123:1d`.
- The encrypted Supabase row carries the idempotency key in the encrypted payload.
- On poll, the receiver checks the local table for a row with the same idempotency key. If one exists, skip insert (already triggered locally — no duplicate). Otherwise insert.

This means: if device A and device B both run the due-date watcher locally for the same task, both create their own `due:abc-123:1d` row. They don't collide because they're in different DBs. The Supabase relay isn't used for `task_due` at all — it's a local-emitter type. Supabase relay is reserved for emitters that originate off-device (Tier 1 server triggers, future external integrations).

This also means: **type-by-type, the spec specifies whether a notification is local-emit or cross-device-emit:**

| Type | Emit origin |
|---|---|
| `task_due`, `task_overdue` | Local-emit each device (watcher runs locally) |
| `sync_failed` | Local-emit (sync runs on each device) |
| `ai_nudge` | Local-emit on the device that hosts the coordinator agent (Tier 1/2: always local. Tier 3: OC-side coordinator via §19.7's split-machine path) |
| `agent_done` | Same as `ai_nudge` |
| `system_event` | Local-emit |
| Push from Pane Cloud (Tier 1 future) | Cross-device emit via Supabase |

INVARIANT (per ADR-0022): **Read and dismiss state is per-device for all notification types.** No row-level CRR sync on the `notifications` table.

## §19.9 Schema additions

The current `notifications` table is fine for what's been built; the additions below are needed for the spec'd behaviors. All idempotent (`ALTER TABLE ... ADD COLUMN IF NOT EXISTS`-shape; per ADR-0019 these go in a per-version-pair migration arm).

```sql
-- Existing columns retained as-is:
--   id, type, title, body, source, read, created_at

ALTER TABLE notifications ADD COLUMN dismissed_at TEXT;             -- set when user dismisses or system auto-dismisses
ALTER TABLE notifications ADD COLUMN expires_at   TEXT;             -- type-specific TTL; nullable
ALTER TABLE notifications ADD COLUMN target_kind  TEXT;             -- 'note' | 'project' | 'agent' | 'url' | NULL
ALTER TABLE notifications ADD COLUMN target_id    TEXT;             -- the navigation target identifier
ALTER TABLE notifications ADD COLUMN idem_key     TEXT;             -- idempotency key for trigger dedup

CREATE INDEX IF NOT EXISTS idx_notifications_active
    ON notifications(created_at DESC)
    WHERE dismissed_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_notifications_idem
    ON notifications(idem_key)
    WHERE idem_key IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_notifications_expires
    ON notifications(expires_at)
    WHERE expires_at IS NOT NULL AND dismissed_at IS NULL;
```

Coordinator flag on the agents table:

```sql
ALTER TABLE ai_agents ADD COLUMN is_coordinator INTEGER NOT NULL DEFAULT 0;

-- Existing-install seeding: Brit is the seeded coordinator agent.
-- Idempotent: sets the flag on the existing row if present, no-op if absent.
UPDATE ai_agents SET is_coordinator = 1 WHERE name = 'Brit';
```

New table for per-type per-device user channel preferences:

```sql
CREATE TABLE IF NOT EXISTS notification_preferences (
    type     TEXT NOT NULL PRIMARY KEY,        -- one of the §19.2 types
    in_app   INTEGER NOT NULL DEFAULT 1,
    os       INTEGER NOT NULL DEFAULT 1,
    push     INTEGER NOT NULL DEFAULT 0,
    email    INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
```

Defaults match §19.2's "Default channels" column; user toggles override.

## §19.10 Non-goals and known shortfalls

Out of scope at any tier (now or post-launch unless re-spec'd):

- **In-app notification snoozing past a few preset durations.** Calendar-grade scheduling lives in tasks (`due_date`), not in notifications.
- **Notification grouping/threading** ("3 tasks due today" rolled into one row). Per-row is the launch design; rollups are a UX iteration, not a structural one.
- **Cross-account notifications.** Pane is single-user per workspace today.
- **Custom notification sounds.** OS-default only.
- **Sub-agent direct emission.** Only coordinator agents can emit `ai_nudge`. If a sub-agent has a result worth surfacing, it returns to the coordinator who decides whether to nudge.
- **Sync-conflict notifications.** All conflicts auto-resolve via LWW today; no notification type exists for conflicts. File a clarifying supersession ADR if non-LWW conflict cases are introduced.

Known gaps explicitly accepted at launch:

- **Tier 3 split-machine OC→Pane user notifications.** Workaround: chat stream (§19.7). Closure plan: post-launch gateway method (§19.7 option 1).
- **Email channel.** Workaround: push + OS native cover urgent cases. Closure plan: post-launch with Pane Cloud only.
- **Toast surface.** Workaround: badge + OS native cover the awareness case. Closure plan: launch+1 with §19.6.3 implementation.

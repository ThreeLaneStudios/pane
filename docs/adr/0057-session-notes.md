# Session Notes — Note Components: Due Today + Advanced Checklist

**Date:** 2026-07-18
**Produces:** §31 (note-components.md), core-data-model §1.9 (subtasks), §19.5.1b/c/d (notification extension), ADR-0057.
**Launch tracker:** #2004 (both components are launch-blocking features).

---

## Decisions made in-session

1. **One vs two components → two, sharing one due-date data path.** Due Today (§31.2) is an interactive dated snapshot of due items (amended 2026-07-18 — originally read-only; now supports check-off, done-visible, date-anchored); Advanced Checklist (§31.3) is the authoring surface producing dated subtasks. Not merged. Due Today also surfaces plain task due dates independent of any checklist.

2. **Subtask data model → dedicated `subtasks` table (reversal).** Initially proposed child-notes (`notes.parent_id`) as the "reuse everything" path. **Reversed after source-reading the task-query surface:** child notes would enter `get_tasks`, the kanban loader, All Tasks, counts, FTS, and Due Today, each needing a `parent_id IS NULL` guard — the exact shared-query blast radius to avoid. Dedicated table excludes subtasks by construction. Locked by Matt; recorded in ADR-0057.

3. **Embed mechanism → existing TipTap node + NodeView pattern (not net-new note infrastructure).** Confirmed against live `Attachment.ts`: `Node.create` + `addNodeView()` mounting a Svelte host + `insert…` command + `data-*` on a wrapper div + raw-HTML-block serialization. Both components are new nodes on this pattern. This was the buildability gate; it is answered.

4. **Notification integration → reuse `task_due`/`task_overdue`, distinguish by `target_kind='subtask'`.** No new type (keeps the ADR-0021 router/renderer single). idem_key `subtask_due:{id}:{threshold}` (distinct prefix from note `due:`). No migration — `target_kind` is unconstrained nullable TEXT in live schema.

5. **Due Today scope → all projects, local midnight, tasks + subtasks, differentiated rows.** Per Matt. Task = note with `due_date` today, `status!='done'`, not trashed. Subtask = `subtasks` row `due_date` today, `done=0`, parent not trashed. Drawer + Due Today both differentiate subtask rows and show parent title.

6. **Deletion path → specified explicitly** (Matt's requirement). See core-data-model §1.9.4 and §19.5.1c. Item delete = hard-delete + companion-clear; parent trash = ride parent, auto-dismiss, re-surface on restore; parent hard-delete = cascade hard-delete + delete notifications.

7. **ADR-0003 reconciliation → head-on in ADR-0057** (Matt's requirement). Subtasks are neither notes nor tasks; no third status, no type column, no `notes.parent_id` claim; unified model preserved *because* subtasks are separate.

### Flagged decisions (need a Matt yes/no if you disagree — defaults chosen)

- **One checklist per note.** Subtasks key on `note_id`; a second checklist node renders the same list. Launch model. Add `subtasks.component_id` later if per-component lists are wanted.
- **Checklist items are DB-source-of-truth, not serialized to markdown.** Items live in `subtasks`; the node is an inert mount point in external markdown viewers. Chosen for watcher-queryability and single-writer; flag if you want a markdown mirror for portability.
- **§ number.** §31 confirmed next-free after §30 Sessions — verified against the live spec TOC before landing.

---

## Backend-field table

| Field / value | Table / column | Existing or new | Reader / writer (build-side) |
| --------------------------------------------------------- | -------------------------- | ------------------------------------------------------------ | ------------------------------------------------------------ |
| `subtasks.id` | new table | new | writer: subtask create; reader: all subtask reads |
| `subtasks.note_id` | new | new | writer: create; readers: checklist NodeView, §19.5.1b scan, Due Today |
| `subtasks.text` | new | new | checklist edit / render |
| `subtasks.due_date` | new | new | §19.5.1b scan, Due Today |
| `subtasks.done` | new | new | §19.5.1b scan, §19.5.1c reconcile |
| `subtasks.completed_at` | new | new | set on done 0→1 |
| `subtasks.sort_order` | new | new | reorder / render order |
| `subtasks.created_at` / `updated_at` | new | new | standard |
| `notifications.target_kind = 'subtask'` | existing column, new value | new **value** (col unconstrained nullable TEXT — no migration) | writer: subtask emit; readers: routing arm §19.5.1d, drawer label |
| `notifications.target_id` = subtask id | existing column | reuse | routing arm |
| `notifications.idem_key` = `subtask_due:{id}:{threshold}` | existing column | new key convention | SELECT-before-INSERT dedup, companion-clear match |
| `notifications.type` = `task_due`/`task_overdue` | existing | reuse (unchanged) | channel router, panel renderer |
| `CRR_TABLES` | migrations.rs | new member (`subtasks`; 21→22) | `enable_crr`, `verify_crr_tables`, `test_fresh_install` count |
| `SCHEMA_VERSION` | migrations.rs | 72 → 73 | fresh install only (no v72→v73 arm, pre-launch) |

---

## Constraint-collision table

**`subtasks`** — only `PRIMARY KEY(id)`.

| Constraint | Collision behavior |
| ----------------- | ------------------------------------------------------------ |
| `PRIMARY KEY(id)` | Fresh UUIDv4 per insert; no natural-key collision. A collision is a UUID-reuse bug → error/rollback, never expected. |

No secondary UNIQUE index. `note_id` intentionally non-unique (a note has many subtasks) — not a collision surface.

**`notifications`** (subtask rows) — only `PRIMARY KEY(id)`; `idx_notifications_idem` is **non-unique**.

| Constraint | Collision behavior |
| ------------------------------------- | ------------------------------------------------------------ |
| `PRIMARY KEY(id)` | Fresh UUIDv4 per emit; no collision. |
| `idx_notifications_idem` (non-unique) | Not a DB constraint. Dedup is app-layer SELECT-before-INSERT on idem_key: on match (active or dismissed) → **skip** insert. One notification per (subtask, threshold), not per tick. Matches ADR-0021. |

---

## Dependencies / coordination

- **Bell-notification bug (2026-07-15, AI-set due date doesn't light the bell) — prerequisite, not fixed here.** A subtask due date set by the AI must trigger the scoped §19.5.1b scan exactly as a user-set one does. B's due-notification path depends on that fix landing. Sequence: the bell fix should land (or land in parallel with) the subtask watcher work; flag to Reed at scope time. This spec does not fix it.
- **#2069 (SubtaskPanel removal) — confirmed frontend-only, dead code.** Nothing to revive; B is fresh. No backend survivor to reconcile.

---

## Source-reads this session relied on (verified 2026-07-18)

- `migrations.rs` (canonical schema v72): `create_system_tables` — notifications is PK-only; `target_kind`/`target_id`/`idem_key` are nullable TEXT; `idx_notifications_idem` non-unique. `create_notes_table` — `parent_id` live, `idx_notes_due_date` live. `CRR_TABLES` = 21. `SCHEMA_VERSION` = "72". `test_crr_tables_not_null_columns_have_defaults` constrains the new table's DEFAULTs.
- `Attachment.ts`: `Node.create` + `addNodeView()` factory + `insertAttachment` command + per-embed `data-*` + raw-HTML-block serialization — the embed pattern both components follow.
- #2069 diff: `SubtaskPanel.svelte` deleted (dead code); no backend change.
- notifications.md §19, core-data-model §1.1–1.8, ADR-0003, ADR-0021/0022 (project knowledge).

---

## Return path

Matt reviews → Brit branches spec PR → Reed audits → Matt merges → code PR cites `spec-covered-by:`. Reed-scope and Stia-build stay separate messages with Matt's go-ahead between. The deletion-path hooks (§19.5.1c / §1.9.4) are the priority audit target — GUI/render/drag surfaces of both components are Matt's live-DMG gate, not source-clean/CI.

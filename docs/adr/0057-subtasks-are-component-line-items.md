# ADR-0057: Subtasks Are Component Line Items, Not Tasks

**Status:** Proposed
**Date:** 2026-07-18
**Author:** Matt Carano

## Context

The Advanced Checklist note component (§31.3) introduces subtasks: line items owned by a note, each carrying a done state, an optional per-item due date, and an ordering. Dated subtasks fire through the existing notification system (§19).

This forces a data-model decision. Pane's founding data invariant is ADR-0003: **notes and tasks are the same entity** — a task is a note with a workflow `status` (amended 2026-07-05: `status` is a first-class closed-set value, `cabinet` is not a NULL sentinel). The `notes` table already carries `parent_id TEXT — hierarchical notes (subtasks)`. The obvious move is therefore to model subtasks as child notes via `notes.parent_id`.

We reject that. This ADR records why, and — critically — reconciles the decision with ADR-0003 so the unified model is not re-litigated later.

A prior subtask implementation (SubtaskPanel, v7.2.0) was removed in #2069 (2026-07-15). That removal was **frontend-only** — the deleted `SubtaskPanel.svelte` was dead code, never mounted; no backend table, column, or command was touched. There is no surviving subtask schema to revive. This is a fresh design.

## Decision

**Subtasks live in a dedicated `subtasks` table (core-data-model §1.9), not in `notes`.** A subtask is a **component line item owned by a note**. It is not a note and not a task.

Concretely:

1. Subtasks are stored in their own CRR table keyed by `note_id`. They never occupy a `notes` row.
2. Subtasks are **excluded from every task surface by construction** — `get_tasks`, `get_project_notes_for_kanban`, All Tasks, task counts, notes FTS, and the Due Today task query all read `notes`, and subtasks are not in `notes`. No query gains a guard; there is nothing to guard.
3. Subtasks reuse the notification pipeline as an emitter (§19.5.1b), distinguished by `target_kind='subtask'`, not by a new notification type.

### Reconciliation with ADR-0003 (read this before proposing "just use child notes")

ADR-0003 says a task is a note with a workflow status, and that notes and tasks are one entity. **This decision does not fork that model — it stays entirely outside it.**

- A subtask is **neither** a note **nor** a task. It is a line item inside a note's checklist component, the way a table cell is not a note. ADR-0003 governs the note/task entity; subtasks are a different kind of thing (component content owned by a note), so ADR-0003's "one entity" claim is untouched — there is still exactly one note/task entity, and subtasks are not a second one.
- Subtasks introduce **no third status**, **no type column on notes**, and **no `parent_id` semantics on notes**. The `status` discriminator remains the sole note/task distinction. `notes.parent_id` remains available for whatever hierarchical-note use it was intended for; subtasks do not claim it.
- The unified model is **preserved precisely because** subtasks are architecturally separate. Putting subtasks in `notes` is what would strain ADR-0003 — it would make some notes "not really tasks" and force every task query to learn that distinction. Keeping them out keeps the notes table meaning exactly what ADR-0003 says it means.

If a future session is tempted to "simplify" by folding subtasks into `notes.parent_id`: that is the rejected alternative below. It re-opens the shared-query blast radius this ADR exists to close.

## Consequences

### Positive

- **Zero task-surface leakage risk.** Subtasks cannot appear on the kanban, in counts, or in task lists, because they are not notes. No `parent_id IS NULL` guard is needed anywhere, so none can be forgotten. (Mechanization over discipline — ADR-0008 lineage.)
- **Clean watcher extension.** The due-date watcher gains one indexed scan over `subtasks` (§19.5.1b); it does not have to learn to tell child notes from real tasks.
- **ADR-0003 stays intact.** The note/task entity keeps a single, unqualified meaning.
- **Notification reuse.** No new type; `target_kind='subtask'` is a value on an already-unconstrained column — no migration.

### Negative

- **A new table and a new CRR member.** `CRR_TABLES` 21 → 22; `SCHEMA_VERSION` 72 → 73; new writers/readers to build.
- **Subtasks are not in the portable markdown.** Item state lives in the DB, not the `.md` (§31.3.3). External markdown viewers see an inert block. Accepted: this is app data, like any task list.
- **A second "list of dated things" alongside notes.** Two emitters for the watcher instead of one. Bounded and explicit.

### Risks

- **Cascade correctness on note deletion.** Subtasks have no independent soft-delete, so their visibility and their notifications are governed by the parent note's lifecycle. If the trash/restore/hard-delete hooks (§19.5.1c, core-data-model §1.9.4) are incomplete, subtasks or their notifications can orphan. The deletion path is specified explicitly to prevent this; it is the primary thing the code audit must verify.
- **One-checklist-per-note constraint.** Subtasks key on `note_id`; multiple checklist nodes in a note render the same list (§31.3.2). If per-component lists are later required, add `subtasks.component_id` — a forward, non-breaking change.

## Alternatives Considered

**Child notes via `notes.parent_id`** — model each subtask as a note whose `parent_id` points at the parent. *Rejected.* Child notes are notes, so they enter every notes-backed query. Keeping them off task surfaces requires a `parent_id IS NULL` conjunct on `get_tasks`, the kanban loader, All Tasks, counts, FTS, and Due Today — and one missed conjunct leaks subtasks onto the board or into counts. It also strains ADR-0003 by creating notes that aren't tasks-in-the-ordinary-sense. The dedicated table removes the entire failure class structurally. (This was the initially-proposed shape in the spec session and was reversed after source-reading the task-query surface.)

**A new `subtask_due` notification type** — give subtasks their own type. *Rejected.* The type enum drives the channel router and panel renderer (ADR-0021); subtasks want identical channels and rendering, differing only in click target and label. That is a `target_kind` distinction. A new type forks the router/renderer for no behavioral gain.

**Serialize subtasks into the note's markdown** (e.g. GFM task list). *Rejected for launch.* Per-item due dates, done flags, and ordering do not round-trip cleanly, and the watcher would have to parse note bodies instead of running an indexed query. Dual-writing DB + markdown invites drift. DB is the single source of truth (§31.3.3).

## References

- Amends/relates: ADR-0003 (Notes and Tasks Are the Same Data Model) — reconciled above; not superseded, not edited.
- Pattern precedent: ADR-0008 (Mechanical Dev Hygiene Over Discipline) — structural exclusion over a remembered guard.
- Notification architecture: ADR-0021 (trigger architecture, idem-key dedup), ADR-0022 (per-device state) — reused unchanged.
- Spec: §31 (Note Components), §19.5.1b/c/d (subtask notification integration), core-data-model §1.9 (`subtasks` table).
- Embed pattern precedent: #401 Attachment node (`Node.create` + `addNodeView()` + `insert…` command).
- Removed prior implementation: #2069 (SubtaskPanel deletion — frontend-only, dead code; nothing revived).
- Schema source: migrations.rs `create_system_tables` (notifications: PK-only, `target_kind` unconstrained nullable TEXT, `idx_notifications_idem` non-unique) and `create_notes_table` (`parent_id` live), verified 2026-07-18.

# ADR-0003: Notes and Tasks Are the Same Data Model

**Status:** Accepted  
**Date:** 2026-02-10  
**Author:** Matt Carano

**Amended:** 2026-07-05 (Matt Carano) — Cabinet is now an explicit `status` value, not the NULL sentinel. See inline **Amended** markers in Decision, Consequences, and Alternatives.

## Context

Most productivity tools separate notes and tasks into different data models — Notion has pages and databases, Todoist has tasks, Obsidian has notes. Users end up context-switching between apps or maintaining parallel systems.

Matt's insight: a task is just a note with a status. A note is just a task without one. The distinction is artificial.

## Decision

Notes and tasks share a single `notes` table. A "task" is a note with a non-null `status` field. Everything else (title, body, project, folder, tags, timestamps) is identical.

> **Amended (2026-07-05):** The single-table decision stands. The task/non-task distinction is no longer null-vs-non-null: every note now carries an explicit `status` from the closed set `cabinet | todo | in_progress | review | done`, defaulting to `'cabinet'`. A **task** is a note whose status is a *workflow* value — `status` set and not `'cabinet'`. `'cabinet'` is a real named value, not a NULL sentinel. No type column is introduced; `status` remains the single discriminator.

## Consequences

### Positive
- **One data model** — No JOIN gymnastics, no sync conflicts between parallel tables.
- **Fluid transitions** — A note becomes a task by setting a status. A completed task becomes a reference note by clearing it.
  > **Amended (2026-07-05):** "Clearing" a task now means **setting `status = 'cabinet'`**, not NULL. `status` is never set to NULL under the explicit-value model.
- **Simpler AI context** — The agent sees one unified list, not two separate systems to reason about.
- **Fewer UI components** — Card rendering, search, navigation all work on one type.

### Negative
- **Status field overloaded** — `status` being null vs non-null is the only distinction. Easy to get wrong in queries.
  > **Amended (2026-07-05):** Resolved. `'cabinet'` is now explicit and `status` is `NOT NULL DEFAULT 'cabinet'`, so the null-vs-non-null overload is gone. The canonical task predicate is mandated as `status IS NOT NULL AND status != 'cabinet'` (never `NOT IN (...)`) — see core-data-model.md §1.1. The null-sentinel special-casing that leaked fresh notes onto the kanban is removed.
- **List views need filtering** — "Show me all tasks" is a WHERE clause, not a separate endpoint.
- **Unconventional** — Every other tool separates these. New users may be confused initially.

### Risks
- As the product grows, notes and tasks might diverge in required fields (due dates, assignees, dependencies). The unified model may need optional columns that feel bolted-on.

## Alternatives Considered

**Separate tables** — Traditional approach. Rejected because it creates sync complexity, forces the AI to reason about two systems, and makes the "note ↔ task" transition a data migration instead of a field update.

**Type column** — `type = 'note' | 'task'` instead of null/non-null status. Considered but adds an extra column for no functional benefit. Status being null already encodes the distinction.

> **Amended (2026-07-05):** The type column remains **rejected** — `status` is still the single discriminator. But the original rationale ("status being null already encodes the distinction") no longer applies: null no longer encodes cabinet. The distinction is now carried by the explicit `'cabinet'` value, which achieves the type column's clarity benefit without a second column.

## References

- Core data model: architecture spec §2
- Card design (locked): MEMORY.md
- Status display: 3px left border color, status pills

# ADR-0052: Chat Session Identity — `chat_sessions` Table, `thread_id` Becomes Load-Bearing

*Line citations predate the #3097 module split.*

**Status:** Accepted
**Date:** 2026-07-08
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** ai-integration.md §4.8.5 item 3 (thread_id removal plan — never shipped; superseded, not deleted); the `chat_thread_{projectId}` ai_settings reset-pointer pattern
**Supplements:** ADR-0022 (per-device chat-adjacent state precedent), ADR-0051 (migration connection lifecycle — v72 arm runs on it)
**Related:** sessions.md §30 (the spec this ADR anchors), ADR-0053 (compaction continuity), docs/audits/2026-07-08-session-model-research.md

## Context

Pane has no real thread model. A "thread" is the `project_id` value bucketing rows in `chat_messages_local`; `get_or_create_thread` is an identity function; the `thread_id` column exists but is written by nothing (INSERT at `save_chat_message` omits it — live-verified 2026-07-08; column presence confirmed on a live DB the same day). "Reset Chat" swaps an `ai_settings` pointer (`chat_thread_{projectId}` → `pane-{uuid}`), orphaning all prior messages: retained but unreachable, violating the UI's own "hidden (not deleted)" promise. Downstream, compaction keys on `context.projectId` and breaks entirely after a reset; the extraction fallback query filters on the always-NULL column and silently returns empty; and the OC gateway session hint keys on the project UUID, so server-side session state survives "New Chat."

The product requirement: first-class sessions — create/list/switch/hide/unhide — per surface (Project, Home), provider-agnostic across all four topologies.

## Decision

1. **New `chat_sessions` table** (schema at §30.1.1): id, surface (`project_id`: project UUID | `'workspace'` | migration-only `'__recovered__'`), auto title, origin, timestamps, `hidden_at`, `is_active`. **Not CRR** — per-device, like `chat_messages_local`.
2. **`chat_messages_local.thread_id` becomes the written FK** to `chat_sessions.id`; loads re-key to it; new `(thread_id, created_at)` index; `project_id` reverts to pure surface meaning.
3. **Active-session invariant is DB-enforced:** partial unique index `(project_id) WHERE is_active = 1`; switch = demote-then-promote in one transaction (§24.7A pattern). No ai_settings pointer replaces the old one.
4. **Migration id-equality rule:** each legacy bucket becomes a session whose `id` **equals the bucket value**. This makes the message backfill a column copy, leaves `compaction_summaries` / `extraction_queue` / `chat_archive` untouched (their thread_id values already match), and keeps the OC gateway hint byte-identical for never-reset projects. Unattributable orphan buckets (≥2 resets deep; pointer overwritten) become hidden `'__recovered__'` sessions listed under Home.
5. **`chat_thread_{projectId}` is superseded:** read once during migration for attribution/active-selection, then all keys deleted.

## Why a new table instead of resurrecting the column alone

Writing `thread_id` without a sessions table gives identity but no metadata home (title, hidden, active, surface) — the active pointer would land back in `ai_settings` (the pattern being retired) and list/switch would be DISTINCT scans over a message table with no place for empty sessions. Separately, the spec formally declared the bare-column path dead (§4.8.5 item 3); a table with the column as FK supersedes that cleanly, where a column-only revival would contradict it. Cost is one small non-CRR table and one migration arm.

## Alternatives considered

- **Extend the `chat_thread_{projectId}` pointer to N slots** (e.g. JSON array per project). Rejected: reimplements a table in a settings value, no constraint enforcement, keeps the orphaning mechanics.
- **Fresh UUIDs for migrated sessions + mapping table.** Rejected: forces real migrations on `compaction_summaries`, `extraction_queue`, `chat_archive`, and breaks OC gateway hint continuity — all avoided by id-equality at zero cost.
- **Drop `thread_id`, key sessions purely via a join table.** Rejected: an extra join on the hottest read path for no benefit; the column already exists.

## Consequences

**Positive:** one-way-door reset eliminated; six live defects closed by construction (§30.9); extraction fallback becomes functional; hide honors existing UI copy; mechanized single-active invariant.
**Negative:** every message save/load call site changes signature (build cost, enumerated at Reed-scope); three surface-keyed delete commands must be superseded in the same arc or they become multi-session-destructive footguns (§30.8.3 — hard gate on the build); `'__recovered__'` is a permanent third surface value in the domain, migration-only but forever in the enum.

## References

Live-source verifications 2026-07-08: `migrations.rs` (schema, v71, CRR list), `commands.rs` (`get_or_create_thread`, `reset_chat_thread`, `save_chat_message`, `load_chat_messages`, dispatch sites), `api.ts` (wrappers, pointer lookup), `ChatPanel.svelte:482–491`, `extraction_worker.rs:343–401`, live DB `.schema chat_messages_local`.

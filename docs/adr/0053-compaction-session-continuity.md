# ADR-0053: Compaction Keys on Session Id

**Status:** Accepted
**Date:** 2026-07-08
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None (changes an implementation keying never governed by an ADR; #354's filter behavior is preserved, re-keyed)
**Supplements:** ADR-0052 (session identity — the id this ADR keys on), ADR-0043 (percentage trigger — untouched)
**Related:** sessions.md §30.5; sessions.md §30.8.3 + memory-pipeline.md §5.14.2 (archive retention — a distinct mechanism; see boundary note in Context)

## Context

Every compaction read/write keys on `compaction_thread_id = context.projectId` (both `ai_chat_stream` sites and `maybe_compact_thread`'s read + INSERT — live-verified 2026-07-08). Because post-reset messages live under a `pane-{uuid}` bucket while the key stays the project UUID, compaction after a reset counts and filters the **orphaned** bucket: the trigger estimate freezes, summaries never attach to the live conversation, and the growing session escapes compaction entirely. The defect is invisible today only because resets are rare.

Boundary note: compaction (this ADR — summaries in `compaction_summaries`, context-window management) is distinct from archival (§5.14.2 — `delete_old_chat_messages` moving >30-day rows to `chat_archive`). Archival is age-keyed and session-orthogonal; it needs no re-keying under ADR-0052 because archive rows already carry `thread_id` (sessions.md §30.1.2, §30.8.3). Nothing in this ADR touches the archive path.

## Decision

1. **All compaction reads and writes key on the active session id** — trigger count, `compacted_before` read, message-id filter, summary INSERT, and `get_compaction_cutoff`.
2. **A new session starts with empty compaction state, by design.** This is the product feature (fresh start after many cycles); project grounding is re-supplied by per-call context injection, and the old session's summaries remain attached to it, intact on switch-back.
3. **Zero legacy migration**, by ADR-0052's id-equality rule: existing `compaction_summaries.thread_id` values (project UUIDs / `'workspace'`) equal their migrated session's id verbatim. Schema, index `(thread_id, created_at DESC)`, and rows are untouched.

## Alternatives considered

- **Per-surface summaries shared across sessions.** Rejected: a new session would inherit a summary of a conversation it doesn't contain — the compaction filter would then drop messages against a cutoff from another session's timeline (exactly the #354 class of bug, reintroduced).
- **Copy latest summary into each new session.** Rejected: same inheritance problem, plus write amplification; context injection already carries the durable project knowledge.

## Consequences

**Positive:** compaction becomes correct across resets/switches for the first time; no data migration; #354's minimum-6-messages guarantee and ADR-0043's percentage trigger carry over unchanged, re-keyed.
**Negative:** long-lived knowledge that existed *only* in an old session's compaction summary (never extracted to `ai_memory`) does not follow to a new session. Accepted: memory extraction + context injection are the sanctioned carriers of durable knowledge; compaction summaries are session-local working state.

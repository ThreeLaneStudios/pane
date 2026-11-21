# ADR-0054: Session Notes — Chat Sessions Spec Session (2026-07-08)

*Line citations predate the #3097 module split.*

**Status:** Notes (non-normative)
**Session:** sessions.md §30, ADR-0052, ADR-0053

## In-session decisions

1. **Identity: new `chat_sessions` table**, `thread_id` becomes its written FK (over column-only revival; over pointer extension). Justified against live schema.
2. **Reset pointer superseded**, not extended: `chat_thread_{projectId}` read-once-and-deleted at migration; active pointer moves into the table as a DB-enforced partial unique index.
3. **Compaction keys on session id**; legacy zero-migration via the id-equality rule (ADR-0053).
4. **Hide = list-visibility only**; extraction/FTS/memory untouched; exclusion semantics deferred to post-launch Delete.
5. **Metadata minimum:** id, project_id(surface), title(auto by default, user-renameable — *amended 2026-07-08, design pass; was post-launch*), origin, created_at, last_active_at, hidden_at, is_active. No provider column (locked).
6. **Canonical message format** stated as requirement + acceptance criterion (mid-session switch across any ordered topology pair with tool history completes without API error); internals build-scope.
7. **Home = same model**, surface `'workspace'`; guards superseded.
8. **Placement:** new file `docs/spec/sessions.md`, §30 (highest existing top-level § is 29). *Brit: one-line TOC/§ collision check at PR time — §19's history earned it.*
9. **Unattributable orphans** (≥2 resets deep): surface sentinel `'__recovered__'`, hidden, grouped under Home. Migration-only value; runtime creation forbidden.
10. **Migration id-equality rule** (session id = legacy bucket value) — the load-bearing simplification; collapses four would-be migrations to zero.

**Design-pass amendments (2026-07-08, ratified by Matt):** A1 — titles user-renameable at launch (new `rename_session` command; auto-title fills only empty titles; empty rename reverts to auto; scope move touched §30.1.1/§30.2.5/§30.10 + intro lifecycle line). A2 — surfaces born-active (§30.2.6: eager session creation on project create — hook location implementer-confirm — plus `get_active_session` lazy safety net; invariant: `get_active_session(surface)` never returns nothing). No ADR — no architectural change.

## Push-backs / corrections surfaced

- **P1 — spec self-contradiction found and superseded:** ai-integration.md §4.8 header says schema cleanup "pending v0.20.1" while §4.8.5 claims "BUILT in v0.20.1"; item 3 half-shipped (query side yes, column drop no). Live schema + live DB settled it. Supersession marker in the companion amendment.
- **P2 — two Reed §3 corrections (live source):** (a) OpenRouter receives **no** session hint — `Some(&thread_id)` is OpenClaw-only at all three dispatch sites (Reed's table said OR passes it); (b) the OC hint is `pane-{context.projectId}` — project-keyed, so today's reset never resets the gateway-side session. Both folded into §30.7.2. §29 note: source-static evidence; execution-log confirmation assigned to build live-test.
- **P3 — surface-keyed hard-delete commands discovered** (`clear_chat_thread`, `clear_chat_messages`, `delete_chat_messages` + `deleteChatThread` wrapper): multi-session-destructive under the new model. Superseded in §30.8.3; caller grep resolved 2026-07-08 (see Open Verifications #1).
- **P4 — `chat_archive` carries `thread_id NOT NULL` already** — inherits session semantics for free under id-equality; archiver write-path confirm assigned to the §30.8.3 implementer.
- **P5 — stale comments flagged in migrations.rs:** `run_migrations` doc-comment still says "deletes + recreates if mismatch" (ADR-0017 ordered it struck); CRR comment says "20 tables" over a 21-entry array.
- **P6 — caller-grep result initially miscategorized; corrected before folding in.** The delete trio's names ARE live backend definitions (verified: `commands.rs:5164`, `:9692`; invoke at `api.ts:1077`) — the grep's empty caller result means **dead code**, not wrong names. `delete_old_chat_messages` is the §5.14.2 **archival** path, not compaction — cross-referenced to §30.1.2/§30.8.3, not ADR-0053's keying decision (0053 gets a boundary note only). `resetChatThread` (ChatSidebar.svelte:319) is the pointer path, not a delete — pinned in §30.8.1. `clearChatMessages` (chatStore.ts:13) "destructive" was unverified — reclassified as classify-at-apply (in-memory store clear vs DB invoke). Two genuine misses in the original grep pattern acknowledged: `delete_old_chat_messages` (not a substring of `delete_chat_messages`) and camelCase `clearChatMessages`.

## Backend-key table

| Name | Existing/new | Reader/writer | Disposition |
|---|---|---|---|
| `chat_thread_{projectId}` (ai_settings) | existing | W: `commands.rs::reset_chat_thread` (invoked from `ChatSidebar.svelte:319`, implementer-confirm); R: `api.ts::getOrCreateThread` | **Superseded** — read in v72 step 3, deleted step 6 |
| `chat_sessions` table + columns | **new** | commands.rs session commands (new) | §30.1.1 |
| `chat_messages_local.thread_id` | existing column, never written | W: `save_chat_message` (new param); R: `load_chat_messages` (re-keyed), `extraction_worker.rs:388` fallback | Becomes load-bearing |
| `compaction_trigger_pct`, `context_window_override` | existing | unchanged | untouched |
| New ai_settings keys | **none** | — | active pointer is `is_active` column |

## Constraint-collision table

| Table / constraint | Columns | Collision behavior |
|---|---|---|
| `chat_sessions` PK | `id` | Runtime create: UUIDv4, plain INSERT — collision = hard error (surfaces a bug). Migration: `INSERT OR IGNORE` (idempotent re-run = skip); bucket values distinct by construction (DISTINCT scan) |
| `chat_sessions` partial UNIQUE | `(project_id) WHERE is_active=1` | Demote-then-promote in one txn (create/switch/hide paths); out-of-order = constraint error, never silent |
| `chat_messages_local` PK | `id` | Existing `INSERT OR REPLACE` (overwrite-in-place) — unchanged. Backfill UPDATE touches only non-unique `thread_id`: cannot collide |
| `compaction_summaries` PK | `id` (randomblob) | Plain INSERT, unchanged; no other unique constraints; zero migration |
| `extraction_queue` UNIQUE | `(message_id)` | Existing `INSERT OR IGNORE` = skip; untouched by migration |
| `chat_archive` | PK `id`; no unique indexes beyond it | Untouched (id-equality) |

## Open verifications (Reed build-scope inputs)

1. **Delete-command caller grep — RESOLVED 2026-07-08** (live `src/` concept-grep, Matt): the delete trio (`clear_chat_thread`, `clear_chat_messages`, `delete_chat_messages`) and the `deleteChatThread` wrapper have **no callers outside api.ts** → dead code, removed per §30.8.3. Residual: classify `clearChatMessages` (`src/lib/stores/chatStore.ts:13`) — in-memory store clear (out of scope) vs DB invoke (sole live caller, re-point to `hide_session`).
2. **`reset_chat_thread` invoke site — RESOLVED:** `ChatSidebar.svelte:319` (`resetChatThread`). Location per live grep; implementer-confirm at apply. Pinned in §30.8.1.
3. **Archiver write path** for `chat_archive.thread_id` — owned by the §30.8.3 implementer-confirm (archive-then-delete ordering + thread_id population from the message record).
4. **OC gateway execution-log check** of the received session key (§30.7.2, §29 rule) — live-test item.
5. **Apply-time verbatim check** of the §4.8.5 BEFORE block (region read via project-knowledge chunks, contiguous with overlap, but the live-file read is the load-bearing one).
6. **Locate backend definitions** of `delete_chat_messages` and `delete_old_chat_messages` — neither is in the commands.rs read this session (or confirm dead invokes).

*Grep-derived locations in this document (`ChatSidebar.svelte:319`, `chatStore.ts:13`, `chat-memory.ts:23`) were reported from a live-`src/` search but not verbatim-verified by the spec session (frontend source not syncable); all carry implementer-confirm-at-apply status, consistent with the feature's standing source-read gate.*

## Process note

The kickoff document arrived empty (attachment-path bug) and this session initially failed to notice, reconstructing the frame from Reed's research + template + onboarding. Matt's live corrections held it on course; the kickoff, once landed, reconciled 7-for-7 with the decisions already made. Flagged for the playbook's verify-what's-in-front-of-you rule: an attached document's *presence* is not its *content*.

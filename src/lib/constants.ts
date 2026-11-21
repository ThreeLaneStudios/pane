// src/lib/constants.ts — shared frontend constants (#2288 PR 2)

/**
 * Well-known sentinel `project_id` for workshop-scoped notes (agents.md
 * §24.10.1e). Mirrors `WORKSHOP_SCOPE_ID` in `src-tauri/src/notes.rs`.
 *
 * Onboarding/create-agent's Memory→Soul review step needs somewhere to
 * attach editable notes before any real `projects` row exists — this
 * reserved literal lets those notes be created, listed, and edited without
 * a backing `projects` row. Distinct from the pre-existing `'workspace'`
 * sentinel (which means "no project, Home chat" for
 * `save_chat_message`/`load_chat_messages` — a different concept).
 *
 * Workshop-scoped notes are DB-only (no on-disk file) and are purged via
 * `purge_note` at `saveAndComplete`.
 */
export const WORKSHOP_SCOPE_ID = '__workshop__';

/**
 * Well-known `project_id` scope used to create a real, disposable
 * `chat_sessions` row for the Memory→Soul workshop refinement chat (#2618).
 *
 * Distinct from `WORKSHOP_SCOPE_ID` above: that sentinel is for DB-only
 * *notes* (no backing `projects` row) and was never meant to double as a
 * `chat_sessions.id`. Post-ADR-0062, `ai_chat_stream`'s per-session agent
 * binding is session-first: when no `sessionId` is passed explicitly, its
 * fallback path in `commands.rs` resolves (or synthesizes) a session id from
 * `project_id`, and `agents.rs::resolve_and_selfheal_session_agent` requires
 * that id to already exist as a `chat_sessions` row. `WorkshopReview.svelte`
 * was passing `WORKSHOP_SCOPE_ID` as `project_id` with no `sessionId`, so the
 * fallback treated the literal string `"__workshop__"` as a session id that
 * never existed in `chat_sessions` — "Session __workshop__ does not exist".
 *
 * The fix: both callers (`Onboarding.svelte`, `AgentCreateOverlay.svelte`)
 * now call `api.createSession(WORKSHOP_SESSION_SCOPE_ID, agentId)` to create
 * a real row explicitly bound to the just-created/just-resolved agent, and
 * pass its `id` down as `WorkshopReview`'s `sessionId` prop, which threads
 * straight through to `ai_chat_stream` — bypassing the fallback entirely.
 *
 * Scoped separately from `'workspace'` (the hidden default project_id used
 * by `getActiveSession`) so this never collides with or demotes the user's
 * real Home-chat active session.
 *
 * Rows created under this scope are intentionally never cleaned up — this
 * mirrors normal "New Session" behavior elsewhere in the app (inactive
 * `chat_sessions` rows persist after a new one is created) and is harmless
 * because nothing lists/surfaces sessions scoped to this project_id in the
 * UI.
 */
export const WORKSHOP_SESSION_SCOPE_ID = '__workshop_session__';

/**
 * §36.17.4 (#3050 PR 10) / Ruling 2 (Matt, 2026-10-29): "the Settings 'Main'
 * badge and the sheet's chip both read DEFAULT CAPTAIN, from one shared
 * string constant so they can't drift. 'Main' goes away." One constant,
 * every consumer — `Settings.svelte`'s Agents-tab badge (was literal
 * `"Main"`) and `NewSessionSheet.svelte`'s captain-list `.dflt` chip (was
 * the design sheet's literal "Default") both import this instead of
 * hardcoding their own string.
 */
export const DEFAULT_CAPTAIN_LABEL = 'DEFAULT CAPTAIN';

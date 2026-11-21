/**
 * workshopSession.ts — shared workshop-chat session resolution (#2618)
 *
 * Both `Onboarding.svelte` and `AgentCreateOverlay.svelte` mount
 * `WorkshopReview.svelte` for the Memory→Soul review step, and both need to
 * hand it a real `chat_sessions` row id before mounting — see
 * `WORKSHOP_SESSION_SCOPE_ID`'s doc comment in `$lib/constants` for the full
 * root-cause explanation (post-ADR-0062, `ai_chat_stream`'s dispatch path is
 * session-first; a synthetic/non-existent session id throws "Session {id}
 * does not exist").
 *
 * Extracted into a shared module (mirrors `seedGeneration.ts`'s "single
 * source of truth" pattern) rather than duplicating the same three-line
 * `api.createSession(...)` wrapper in both components, so both callers stay
 * identical and the logic is unit-testable without mounting either
 * ~1000+-line Svelte component.
 */
import { createSession } from '$lib/api';
import { WORKSHOP_SESSION_SCOPE_ID } from '$lib/constants';

/**
 * Creates a real, disposable `chat_sessions` row explicitly bound to
 * `agentId`, scoped to `WORKSHOP_SESSION_SCOPE_ID` (distinct from the
 * notes-only `WORKSHOP_SCOPE_ID` sentinel). Returns the new session's id for
 * the caller to pass down as `WorkshopReview`'s `sessionId` prop.
 *
 * Explicit `agentId` binding (via `api.createSession(projectId, agentId)`)
 * rather than `api.getActiveSession('workspace')` — the latter binds to
 * whichever agent is currently `is_default`, which is correct by
 * construction (new agents are promoted to `is_default` as part of their
 * creation transaction) but adds an implicit timing dependency this function
 * avoids by being explicit (Reed's investigation, option 2).
 *
 * Throws (does not swallow) on failure — callers must surface the error per
 * SOUL.md's "no silent failures" rule; both call sites already run inside a
 * try/catch that sets a visible error state.
 */
export async function createWorkshopSession(agentId: string): Promise<string> {
  if (!agentId) {
    throw new Error('createWorkshopSession requires a non-empty agentId');
  }
  const session = await createSession(WORKSHOP_SESSION_SCOPE_ID, agentId);
  return session.id;
}

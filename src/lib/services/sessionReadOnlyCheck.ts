/**
 * sessionReadOnlyCheck.ts — shared read-only/lock computation for a session's
 * bound agent (D22, #2649 family).
 *
 * Extracted from SessionDropdownHeader.svelte:191-234 (the `boundAgent` /
 * `bindingUnavailableReason` derivation) so surfaces that don't mount
 * SessionDropdownHeader — namely Home (`ChatSidebar.svelte`, `$activeProject
 * === null`) — can still compute the same lock state and render
 * `ReadOnlySessionNotice` + disable the composer.
 *
 * ADR-0063 D6 rule this mirrors exactly: read-only fires ONLY when the bound
 * agent is inactive, OR its runtime is genuinely disconnected. Both
 * `SessionDropdownHeader` and `ChatSidebar` MUST call this same function so
 * the two surfaces can never disagree on whether a session is locked.
 *
 * Do not duplicate this logic inline anywhere else — call this function.
 */

import { getAgents, listRuntimeConnections } from '$lib/api';
import type { RuntimeConnectionInfo } from '$lib/api';
import type { Agent, Session } from '$lib/types';

export interface SessionReadOnlyState {
  reason: 'agent' | 'provider';
  agent: Agent;
}

/**
 * Resolves the given session's bound agent from an already-fetched agent
 * list. Mirrors `SessionDropdownHeader.svelte`'s `boundAgent` derivation
 * (`activeSession.agent_id` → `allAgents.find(...)`) exactly — pass the full,
 * UNFILTERED agent list (i.e. `getAgents()`'s raw result, which includes
 * inactive agents) or this will silently fail to find deactivated bindings.
 */
export function resolveBoundAgent(session: Session | null, allAgents: Agent[]): Agent | null {
  if (!session?.agent_id) return null;
  return allAgents.find(a => a.id === session.agent_id) ?? null;
}

/**
 * Mirrors `SessionDropdownHeader.svelte`'s `bindingUnavailableReason`
 * derivation exactly (ADR-0063 D6):
 *   - No bound agent → not read-only (null).
 *   - Bound agent inactive (`is_active === false`) → 'agent'.
 *   - Bound agent active but its own runtime is genuinely disconnected → 'provider'.
 *   - Unpinned legacy agent (no runtime_type/runtime_ref) → not read-only yet (D4.3).
 */
export function computeBindingUnavailableReason(
  boundAgent: Agent | null,
  runtimeConnections: RuntimeConnectionInfo[]
): 'agent' | 'provider' | null {
  if (!boundAgent) return null;
  if (!boundAgent.is_active) return 'agent';
  const runtimeType = boundAgent.runtime_ref != null ? 'openclaw' : boundAgent.runtime_type;
  if (!runtimeType) return null; // unpinned legacy agent — no runtime to check yet (D4.3)
  const connection = runtimeConnections.find(c => c.runtime_type === runtimeType);
  if (connection && !connection.connected) return 'provider';
  return null;
}

/**
 * Full end-to-end check: fetches agents + runtime connections, resolves the
 * session's bound agent, and returns the read-only state (or `null` if the
 * session is not locked). This is the single entry point callers should use
 * — `SessionDropdownHeader` and `ChatSidebar` (Home) both call this so the
 * lock computation can never drift between surfaces.
 */
export async function checkSessionReadOnly(session: Session | null): Promise<SessionReadOnlyState | null> {
  if (!session?.agent_id) return null;
  try {
    const [allAgents, runtimeConnections] = await Promise.all([getAgents(), listRuntimeConnections()]);
    const boundAgent = resolveBoundAgent(session, allAgents);
    const reason = computeBindingUnavailableReason(boundAgent, runtimeConnections);
    if (reason && boundAgent) {
      return { reason, agent: boundAgent };
    }
    return null;
  } catch (e) {
    console.error('[sessionReadOnlyCheck] failed to compute read-only state:', e);
    return null;
  }
}

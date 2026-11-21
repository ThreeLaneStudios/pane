/**
 * agentDisplay.ts — small display-only helpers shared by the agent picker
 * surfaces (session header, New Session picker, New Project dialog).
 *
 * #2563 B2 fix (2026-08-18): both exports below used to derive from the
 * single global `ai_settings.provider` scalar (via a `activeProvider`
 * parameter sourced from `aiLoadConfig()`). PR #2574 retired that scalar
 * from every backend reader in favor of each agent's own `runtime_type`
 * column (ADR-0063 D1/D4.3) — these two frontend functions were the last
 * readers left pointed at the dead value. Both are now purely per-agent:
 * `agentProviderLabel` reads only the agent's own identity fields, and
 * `filterConnectedAgents` (replacing the retired `filterInClassAgents`)
 * checks live per-runtime connectivity via `ai_list_runtime_connections`
 * (the same mechanism `DefaultAgentPickerModal` already uses) instead of
 * a client-side XOR against the global scalar.
 */

import type { Agent, Session } from '$lib/types';
import type { RuntimeConnectionInfo } from '$lib/api';
import { WORKSHOP_SESSION_SCOPE_ID } from '$lib/constants';

/** Human-readable provider names, matching Settings.svelte's inline mapping. */
const PROVIDER_LABELS: Record<string, string> = {
  openclaw: 'OpenClaw',
  openrouter: 'OpenRouter',
  anthropic: 'Anthropic',
  ollama: 'Ollama',
  venice: 'Venice',
};

/**
 * Provider display label for an agent, derived entirely from the agent's own
 * identity — no global state involved.
 *
 * - OC-imported agents (`runtime_ref` non-null, was `oc_agent_id` pre-
 *   ADR-0063) are always "OpenClaw" — an agent's OC/local class is
 *   permanent (§24.7C), never a function of any currently-active setting.
 * - Locally-created, pinned agents (`runtime_type` non-null, D4.3) show
 *   their own `runtime_type`'s display name.
 * - Unpinned legacy agents (`runtime_type === null`, the v79-migration
 *   legacy-unpinned state, D4.3) have no runtime to resolve yet — shown as
 *   "Local" until the user pins one (via LegacyProviderPrompt at selection
 *   time).
 */
export function agentProviderLabel(agent: Pick<Agent, 'runtime_ref' | 'runtime_type'>): string {
  if (agent.runtime_ref != null) return 'OpenClaw';
  if (agent.runtime_type != null) return PROVIDER_LABELS[agent.runtime_type] ?? agent.runtime_type;
  return 'Local';
}

/**
 * The agent's own effective runtime_type key (not a display label) — e.g.
 * 'openclaw' | 'openrouter' | 'anthropic' | 'venice' | 'ollama' | 'none' for an
 * unpinned legacy agent. Used by surfaces (like ReadOnlySessionNotice) that
 * need the raw key to look up their own label/state, not a pre-formatted
 * string. Mirrors `agentProviderLabel`'s identity resolution rules exactly.
 */
export function agentRuntimeType(agent: Pick<Agent, 'runtime_ref' | 'runtime_type'>): string {
  if (agent.runtime_ref != null) return 'openclaw';
  return agent.runtime_type ?? 'none';
}

/**
 * Connectivity-based agent filter for the explicit picker surfaces (New
 * Session, New Project). Replaces the retired `filterInClassAgents`, which
 * XOR'd an agent's `runtime_ref` null-ness against the single global
 * `ai_settings.provider` scalar — a check that was blind to actual runtime
 * connectivity and excluded whole classes of agents whenever the global
 * scalar drifted from an individual agent's identity (#2563 B2).
 *
 * An agent is included when it is active AND its own `runtime_type` maps to
 * a currently-connected runtime (`connections`, from
 * `listRuntimeConnections()` / `ai_list_runtime_connections` — the one
 * function in this codebase that actually calls
 * `build_provider_for_runtime_type` per runtime and tests live
 * reachability). Unpinned legacy agents (`runtime_type === null`) are
 * excluded — they cannot be resolved to a runtime (D4.3). C3 (#2587):
 * this population is deactivated at rest by `migrate_v83`, so it's normally
 * excluded by the `is_active` check above before this predicate even runs;
 * the `runtime_type != null` check is kept as defense in depth.
 *
 * #2588 C4 (Matt's ruling: subagent sessions are always-no): agents with
 * `role === 'sub'` AND a non-null `runtime_ref` (real OC-imported
 * subagents — locally-created agents can share role='sub' but have
 * runtime_ref = null and are NOT excluded here) are excluded
 * unconditionally, before the connectivity check. Same predicate as
 * `DefaultAgentPickerModal`'s `eligibleAgents` filter (#2567/#2577 B5) and
 * the backend `set_default_agent_impl` guard — this extends that same
 * subagent exclusion to every remaining picker that shares this utility
 * (New Session header, Sessions drawer, New Project), so subagents never
 * appear as selectable options in any session picker.
 */
export function filterConnectedAgents<T extends Pick<Agent, 'runtime_type' | 'is_active' | 'role' | 'runtime_ref'>>(
  agents: T[],
  connections: Pick<RuntimeConnectionInfo, 'runtime_type' | 'connected'>[]
): T[] {
  const connectedTypes = new Set<string>(connections.filter(c => c.connected).map(c => c.runtime_type));
  return agents.filter(
    a =>
      !(a.role === 'sub' && a.runtime_ref != null) &&
      a.is_active &&
      a.runtime_type != null &&
      connectedTypes.has(a.runtime_type)
  );
}

/**
 * #2618: workshop sessions (project_id === WORKSHOP_SESSION_SCOPE_ID,
 * "__workshop_session__") are disposable rows created solely so
 * WorkshopReview.svelte's `ai_chat_stream` call has a real `chat_sessions`
 * id to bind to (see `WORKSHOP_SESSION_SCOPE_ID`'s doc comment in
 * `$lib/constants` for the full root-cause). They must never appear in any
 * session picker or session list — the 'workspace' surface's session
 * picker/drawer and `getActiveSession('workspace')` should only ever show
 * real Home-chat sessions.
 *
 * Structurally, this is already guaranteed: every session-listing backend
 * query (`list_sessions`, `get_active_session`, `demote_and_promote`) is
 * scoped by an exact `project_id` equality match, so a session created
 * under `WORKSHOP_SESSION_SCOPE_ID` can never be returned by a query scoped
 * to `'workspace'`. This filter is defense-in-depth, kept alongside
 * `filterConnectedAgents`'s C4 subagent-exclusion (#2588) above — both
 * functions exist to guarantee a category of row that should never be
 * user-selectable stays excluded even if a future caller widens a query's
 * scope. See `sessionStore.ts`'s `loadSessions()`, which applies this to
 * every session list SessionDropdownHeader/SessionsDrawer render.
 */
export function filterOutWorkshopSessions<T extends Pick<Session, 'project_id'>>(sessions: T[]): T[] {
  return sessions.filter(s => s.project_id !== WORKSHOP_SESSION_SCOPE_ID);
}

/**
 * #3074 gate — whether an agent's model is safe to expose as user-editable.
 *
 * Editable only for `runtime_type` in {'anthropic', 'openrouter', 'venice', 'ollama'}.
 * Everything else (including every runtime_type string not in that list, should one
 * ever be added without updating this gate) is NOT editable — this is an explicit
 * allow-list, not a deny-list, so a new runtime lands excluded-by-default until someone
 * deliberately adds it here.
 *
 * Two runtime_type values are deliberately excluded, both load-bearing, not oversights:
 *
 * - `'openclaw'` — after every successful OC-gateway stream,
 *   `write_back_agent_model_if_changed` (ADR-0063 D9; `src-tauri/src/ai/runtime_resolution.rs:356`,
 *   called from `src-tauri/src/ai/commands.rs:16663`) overwrites `ai_agents.model` with
 *   whatever the gateway's own agent registry actually resolved, write-on-change only.
 *   The OC gateway never reads `ai_agents.model` as a dispatch input (D9's whole point —
 *   it's a post-dispatch *display cache*, not a dispatch-time input) — so a picker here
 *   would let a user "successfully" edit the model, only to have it silently revert on
 *   the very next completed turn. That isn't unsupported, it's actively wrong to show.
 * - `null` (the legacy-unpinned migration state, ADR-0063 D4.3) — these rows still reach
 *   the Agents list (grouped under the 'unknown' key), but there is no runtime to fetch
 *   a model list from yet; "pick a model" before "pick a runtime" is backwards.
 *
 * Both exclusions are enumerated explicitly (not inferred from "not openclaw") so this
 * function reads correctly even if someone deletes the comment — the allow-list IS the
 * documentation of what's safe, not a redundant restatement of it.
 */
export function isAgentModelEditable(agent: Pick<Agent, 'runtime_type'>): boolean {
  return (
    agent.runtime_type === 'anthropic' ||
    agent.runtime_type === 'openrouter' ||
    agent.runtime_type === 'venice' ||
    agent.runtime_type === 'ollama'
  );
}

/** Two-letter initials for an agent's avatar chip, e.g. "Brit Freed" -> "BF". */
export function agentInitials(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) return '?';
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
}

/**
 * §36.17.4 (#3050 PR 10): groups agents by provider label, default-first
 * then name within each group — extracted verbatim from
 * `AgentPickerRows.svelte`'s own `groups` `$derived.by` (byte-identical
 * logic, now a named export both that component and the new
 * `NewSessionSheet.svelte` import, rather than two copies of the same
 * grouping algorithm). Preserves first-seen group order, matching the
 * original's `Map` iteration behavior.
 */
export function groupAgentsByProvider<T extends Pick<Agent, 'runtime_ref' | 'runtime_type' | 'is_default' | 'name'>>(
  agents: T[]
): [string, T[]][] {
  const byLabel = new Map<string, T[]>();
  for (const a of agents) {
    const label = agentProviderLabel(a);
    if (!byLabel.has(label)) byLabel.set(label, []);
    byLabel.get(label)!.push(a);
  }
  for (const list of byLabel.values()) {
    list.sort((a, b) => (b.is_default ? 1 : 0) - (a.is_default ? 1 : 0) || a.name.localeCompare(b.name));
  }
  return Array.from(byLabel.entries());
}

/**
 * §36.17.4 (#3050 PR 10): the one parse site for `Session.crew_agent_ids`'
 * raw JSON text (see that field's doc comment in `types.ts` for why it's
 * raw text, not pre-parsed, on the wire type). `null`/`undefined`/invalid
 * JSON/non-array JSON all return `[]` — fail closed, same posture as the
 * backend's own `session_has_crew` (`ai/tools.rs`): "unparseable means no
 * crew," never a thrown error reaching a render path.
 */
export function parseCrewAgentIds(raw: string | null | undefined): string[] {
  if (!raw) return [];
  try {
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter((x): x is string => typeof x === 'string') : [];
  } catch {
    return [];
  }
}

/**
 * §36.17.4 (#3050 PR 10): shared chip-capping for crew clusters — the
 * drawer row's `.cl` cluster and the session header's `.crewbtn` cluster
 * (`SessionRow.svelte`, `SessionDropdownHeader.svelte`) both need "show up
 * to `max`, then a '+N' overflow chip, with a tooltip listing every crew
 * member's name" — one function, not two copies of the same slicing/
 * formatting logic (the design's own Notes item 7: "Rows cap at three crew
 * chips plus '+N'. The full list is in the tooltip.").
 */
export function capCrewChips<T extends Pick<Agent, 'id' | 'name'>>(
  crew: T[],
  max = 3
): { shown: T[]; overflowCount: number; tooltipText: string } {
  const shown = crew.slice(0, max);
  const overflowCount = Math.max(0, crew.length - max);
  const tooltipText = crew.length > 0 ? `Crew: ${crew.map(c => c.name).join(', ')}` : '';
  return { shown, overflowCount, tooltipText };
}

/**
 * sessionStore.ts — Session store + actions for #395 chat sessions (§30.2).
 *
 * Holds the session list per surface. Active session always pinned first,
 * then sorted by last_active_at DESC.
 *
 * Actions wire to api.ts wrappers added in PR2.
 *
 * §395 PR6 — spec-covered-by: #1979
 */

import { writable, get } from 'svelte/store';
import type { Session } from '$lib/types';
import {
  createSession as apiCreateSession,
  createSessionWithCrew as apiCreateSessionWithCrew,
  listSessions as apiListSessions,
  switchSession as apiSwitchSession,
  hideSession as apiHideSession,
  unhideSession as apiUnhideSession,
  renameSession as apiRenameSession,
  getActiveSession as apiGetActiveSession,
  updateSessionCrew as apiUpdateSessionCrew,
} from '$lib/api';
import { filterOutWorkshopSessions } from '$lib/utils/agentDisplay';

// ─── Types ────────────────────────────────────────────────────────────────────

export interface SessionState {
  /** All sessions for the current surface (active-first, then last_active_at DESC). */
  sessions: Session[];
  /** Currently active session, or null if not yet loaded. */
  activeSession: Session | null;
  /** Whether the session list is loading. */
  loading: boolean;
  /** Last error encountered (surface for error display). */
  error: string | null;
}

// ─── Ordering rule ────────────────────────────────────────────────────────────

/**
 * Sort sessions: active session always pinned first, rest sorted by
 * last_active_at DESC.
 */
export function sortSessions(sessions: Session[]): Session[] {
  return [...sessions].sort((a, b) => {
    if (a.is_active && !b.is_active) return -1;
    if (!a.is_active && b.is_active) return 1;
    // Both equal — fall back to last_active_at DESC
    return b.last_active_at.localeCompare(a.last_active_at);
  });
}

// ─── Fresh-session hint copy ─────────────────────────────────────────────────

/**
 * Shown in ChatPanel empty state when isFreshSession=true (session exists, no messages).
 * Project context: scoped to project notes & tasks.
 */
export const FRESH_SESSION_HINT = "Fresh session — a clean thread, still scoped to this project's notes & tasks. Ask anything to begin.";

/**
 * Home context variant: no project scope, so copy says "your notes" instead.
 */
export const FRESH_SESSION_HINT_HOME = 'Fresh session — a clean thread, still scoped to your notes. Ask anything to begin.';

// ─── Store ────────────────────────────────────────────────────────────────────

function createSessionStore() {
  const initial: SessionState = {
    sessions: [],
    activeSession: null,
    loading: false,
    error: null,
  };

  const { subscribe, set, update } = writable<SessionState>(initial);

  // ─── Actions ───────────────────────────────────────────────────────────────

  /**
   * Load all sessions for a surface. Replaces the store's session list.
   * Pass includeHidden=true to include hidden sessions (e.g. for the
   * "show hidden" toggle).
   */
  async function loadSessions(projectId: string, includeHidden = false): Promise<void> {
    update(s => ({ ...s, loading: true, error: null }));
    try {
      const rawSessions = await apiListSessions(projectId, includeHidden);
      // #2618: defense-in-depth exclusion of disposable workshop-chat
      // sessions, alongside filterConnectedAgents's C4 subagent exclusion —
      // see filterOutWorkshopSessions's doc comment (agentDisplay.ts).
      const sessions = filterOutWorkshopSessions(rawSessions);
      const sorted = sortSessions(sessions);
      const activeSession = sorted.find(s => s.is_active) ?? null;
      update(s => ({ ...s, sessions: sorted, activeSession, loading: false }));
    } catch (err: any) {
      const msg = `[sessionStore] loadSessions failed for ${projectId}: ${err?.message ?? err}`;
      console.error(msg);
      update(s => ({ ...s, loading: false, error: msg }));
    }
  }

  /**
   * Create a new session for a surface and make it active.
   * Reloads the list after creation.
   *
   * ADR-0062 D1/D2: `agentId` is the permanent binding chosen in the New
   * Session picker (default preselected) — required, not optional.
   */
  /**
   * §36.17.4 (#3050 PR 10): `crewAgentIds` is additive and optional —
   * `undefined` (every existing caller: `ChatPanel.svelte`'s default-agent
   * auto-create) keeps calling the original `apiCreateSession`, byte-
   * identical to before this PR. Only the New Session sheet's trigger
   * (`SessionDropdownHeader.svelte`) passes an explicit array (possibly
   * empty), routing through `apiCreateSessionWithCrew` instead.
   */
  async function createSession(projectId: string, agentId: string, crewAgentIds?: string[]): Promise<Session> {
    update(s => ({ ...s, loading: true, error: null }));
    try {
      const session = crewAgentIds !== undefined
        ? await apiCreateSessionWithCrew(projectId, agentId, crewAgentIds)
        : await apiCreateSession(projectId, agentId);
      // Reload to get the full sorted list
      await loadSessions(projectId, true); // always fetch with include_hidden=true; showHidden controls display only
      return session;
    } catch (err: any) {
      const msg = `[sessionStore] createSession failed for ${projectId}: ${err?.message ?? err}`;
      console.error(msg);
      update(s => ({ ...s, loading: false, error: msg }));
      throw err;
    }
  }

  /**
   * §36.17.4 (#3050 PR 10): the edit-crew popover's "Save crew" write path.
   * Reloads the surface's session list afterward (same pattern every other
   * action here already uses) so the drawer/header's chip clusters reflect
   * the new crew immediately.
   */
  async function updateSessionCrew(sessionId: string, crewAgentIds: string[], projectId: string): Promise<Session> {
    update(s => ({ ...s, loading: true, error: null }));
    try {
      const session = await apiUpdateSessionCrew(sessionId, crewAgentIds);
      await loadSessions(projectId, true);
      return session;
    } catch (err: any) {
      const msg = `[sessionStore] updateSessionCrew failed for ${sessionId}: ${err?.message ?? err}`;
      console.error(msg);
      update(s => ({ ...s, loading: false, error: msg }));
      throw err;
    }
  }

  /**
   * Switch to a different session. Promotes the target session; demotes the
   * previously active one. Reloads the list to reflect new ordering.
   */
  async function switchSession(sessionId: string, projectId: string): Promise<Session> {
    update(s => ({ ...s, loading: true, error: null }));
    try {
      const session = await apiSwitchSession(sessionId);
      await loadSessions(projectId, true); // always fetch with include_hidden=true; showHidden controls display only
      return session;
    } catch (err: any) {
      const msg = `[sessionStore] switchSession failed for ${sessionId}: ${err?.message ?? err}`;
      console.error(msg);
      update(s => ({ ...s, loading: false, error: msg }));
      throw err;
    }
  }

  /**
   * Hide a session. If it was active, the backend promotes the next visible
   * session (or creates one). Reloads the list.
   */
  async function hideSession(sessionId: string, projectId: string): Promise<void> {
    update(s => ({ ...s, loading: true, error: null }));
    try {
      await apiHideSession(sessionId);
      await loadSessions(projectId, true); // always fetch with include_hidden=true; showHidden controls display only, never fetching
    } catch (err: any) {
      const msg = `[sessionStore] hideSession failed for ${sessionId}: ${err?.message ?? err}`;
      console.error(msg);
      update(s => ({ ...s, loading: false, error: msg }));
      throw err;
    }
  }

  /**
   * Unhide a session. Does not change active status. Reloads the list.
   */
  async function unhideSession(sessionId: string, projectId: string): Promise<void> {
    update(s => ({ ...s, loading: true, error: null }));
    try {
      await apiUnhideSession(sessionId);
      await loadSessions(projectId, true); // reload with hidden visible so the unhidden row stays in list
    } catch (err: any) {
      const msg = `[sessionStore] unhideSession failed for ${sessionId}: ${err?.message ?? err}`;
      console.error(msg);
      update(s => ({ ...s, loading: false, error: msg }));
      throw err;
    }
  }

  /**
   * Rename a session. Empty/whitespace title resets to '' (backend handles
   * auto-title revert). Updates the store optimistically, then reloads.
   */
  async function renameSession(sessionId: string, title: string, projectId: string): Promise<void> {
    // Optimistic update so the UI feels instant
    update(s => ({
      ...s,
      sessions: s.sessions.map(session =>
        session.id === sessionId ? { ...session, title } : session
      ),
      activeSession:
        s.activeSession?.id === sessionId
          ? { ...s.activeSession, title }
          : s.activeSession,
    }));
    try {
      await apiRenameSession(sessionId, title);
      // Reload to get backend-trimmed value
      await loadSessions(projectId, true); // always fetch with include_hidden=true; showHidden controls display only
    } catch (err: any) {
      const msg = `[sessionStore] renameSession failed for ${sessionId}: ${err?.message ?? err}`;
      console.error(msg);
      update(s => ({ ...s, error: msg }));
      throw err;
    }
  }

  /**
   * Get the active session for a surface. Creates one if none exists
   * (lazy safety net — never returns null). Updates the store.
   */
  async function getActiveSession(projectId: string): Promise<Session> {
    try {
      const session = await apiGetActiveSession(projectId);
      update(s => ({
        ...s,
        activeSession: session,
        // Ensure it's in the sessions list (in case store not yet loaded)
        sessions: s.sessions.some(sess => sess.id === session.id)
          ? s.sessions
          : sortSessions([...s.sessions, session]),
      }));
      return session;
    } catch (err: any) {
      const msg = `[sessionStore] getActiveSession failed for ${projectId}: ${err?.message ?? err}`;
      console.error(msg);
      update(s => ({ ...s, error: msg }));
      throw err;
    }
  }

  /** Reset the store (e.g., on surface change). */
  function reset(): void {
    set(initial);
  }

  /**
   * #3166 C58 — sets `activeSession` directly from a `Session` the
   * caller ALREADY HAS (the backend's own `switch_session` response),
   * without a second round trip and without re-promoting/demoting
   * anything (`switch_session`'s own backend guard already did that, or
   * deliberately didn't — see its own doc comment). This is the piece
   * `loadSessions`'s `$effect` (`SessionDropdownHeader.svelte`) never
   * covers: that effect only re-fires when `projectId` changes, so a
   * job session opened via the bell/card in the SAME project the user
   * was already viewing left `activeSession` pinned on the project's
   * ordinary interactive session — the header kept showing that
   * session's stale name while the message body correctly showed the
   * job session's content.
   *
   * Both entry points into a job session (`NotificationPanel.svelte`'s
   * bell, `scheduledPanel.ts`'s Jobs-card `openScheduledJobSession`) call
   * this right after their own `switch_session` resolves. The RETURN
   * trip (`jobSessionBack.ts`'s `goToJobSessionBackTarget`) calls it too
   * — without that, going back would leave `activeSession` pinned on the
   * job session instead of correcting back to whatever was switched to,
   * the same staleness in the other direction.
   *
   * Adds the session to `sessions` if the store doesn't have it yet
   * (same shape as `getActiveSession`'s own update above) so
   * `sortSessions`/`SessionRow` consumers never see an active session
   * missing from the list.
   */
  function setActiveSessionFromSwitch(session: Session): void {
    update(s => ({
      ...s,
      activeSession: session,
      sessions: s.sessions.some(sess => sess.id === session.id)
        ? s.sessions.map(sess => (sess.id === session.id ? session : sess))
        : sortSessions([...s.sessions, session]),
    }));
  }

  return {
    subscribe,
    loadSessions,
    createSession,
    switchSession,
    hideSession,
    unhideSession,
    renameSession,
    getActiveSession,
    setActiveSessionFromSwitch,
    updateSessionCrew,
    reset,
  };
}

export const sessionStore = createSessionStore();

// ─── Filter logic ─────────────────────────────────────────────────────────────

/** Minimum session count to show the search field. */
export const FILTER_THRESHOLD = 8;

/**
 * Returns true if a search bar should be shown for this session list.
 * Threshold: ≥8 sessions (including hidden ones).
 */
export function shouldShowFilter(totalCount: number): boolean {
  return totalCount >= FILTER_THRESHOLD;
}

/**
 * Filter sessions by search query against title (case-insensitive).
 * Works across visible and hidden sessions so consumers can group results.
 *
 * Returns { visible, hidden } — matched sessions split by hidden status.
 * Hidden matched rows are surfaced so consumers can render a "Hidden" section.
 */
export function filterSessions(
  sessions: Session[],
  query: string
): { visible: Session[]; hidden: Session[] } {
  const q = query.trim().toLowerCase();
  if (!q) {
    const visible = sessions.filter(s => s.hidden_at === null);
    const hidden = sessions.filter(s => s.hidden_at !== null);
    return { visible, hidden };
  }
  const matched = sessions.filter(s => s.title.toLowerCase().includes(q));
  return {
    visible: matched.filter(s => s.hidden_at === null),
    hidden: matched.filter(s => s.hidden_at !== null),
  };
}

// ─── Auto-title helpers ───────────────────────────────────────────────────────

/**
 * Format a session's display name. When title is empty, falls back to
 * "Session YYYY-MM-DD" derived from created_at. When no messages have been
 * sent yet, this shows as "New session" via the fresh-session constant.
 */
export function getSessionDisplayTitle(session: Session): string {
  if (session.title && session.title.trim() !== '') {
    return session.title;
  }
  // Auto-title fallback: "Session YYYY-MM-DD"
  const d = new Date(session.created_at);
  const yyyy = d.getFullYear();
  const mm = String(d.getMonth() + 1).padStart(2, '0');
  const dd = String(d.getDate()).padStart(2, '0');
  return `Session ${yyyy}-${mm}-${dd}`;
}

// ─── Empty-state exports ──────────────────────────────────────────────────────

/** Copy shown when session count === 1. */
export const EMPTY_STATE_SINGLE = {
  headline: 'Just this one so far',
  body: 'New sessions you start in this project collect here. Old ones can be hidden, never deleted.',
} as const;

/** Display name for a session with no messages and no user title yet. */
export const FRESH_SESSION_NAME = 'New session';

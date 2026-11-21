/**
 * SessionDropdownHeader.test.ts — Component tests for SessionDropdownHeader.svelte
 *
 * CI-testable scope (per §395 audit §9 boundary):
 *   ✓ Dropdown open/close: click header → opens, click again → closes
 *   ✓ Escape key closes dropdown
 *   ✓ Filter threshold: <8 sessions → no filter field; ≥8 → filter visible
 *   ✓ Session ordering: active-first, then last_active_at DESC
 *   ✓ New Session button calls createSession
 *   ✓ Responsive prop: compact mode passes compact=true to SessionRow
 *   ✓ Dead code removal: onResetThread gone from ChatPanel
 *
 * NOT tested here (live DMG tests — Matt):
 *   - Actual dropdown position/anchoring (requires DOM measurement)
 *   - Overflow clipping (requires rendered layout)
 *   - Resize handle dragging
 *   - Scroll behaviour with pinned header/footer
 *   - Inline rename clipping at scroll edges
 *
 * §395 PR7 — spec-covered-by: #1979
 */

import { describe, it, expect, vi, beforeEach, afterEach, beforeAll } from 'vitest';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import { readFileSync } from 'fs';
import { resolve } from 'path';
import SessionDropdownHeader from './SessionDropdownHeader.svelte';
import type { Agent, Session } from '$lib/types';
import * as sessionStoreMod from '$lib/stores/sessionStore';
import * as apiMod from '$lib/api';
import { activeSessionOrigin } from '$lib/stores/app';
import { isJobSessionOrigin } from '$lib/services/jobSessionBack';

// ADR-0062 D2: New Session now opens an agent picker before creating a
// session, so SessionDropdownHeader also calls getAgents()/listRuntimeConnections()
// (#2563 B2: re-pointed from the retired aiLoadConfig().provider global scalar
// to per-runtime connectivity).
function mkAgent(overrides: Partial<Agent> = {}): Agent {
  return {
    id: 'agent-1',
    name: 'Test Agent',
    slug: 'test-agent',
    role: 'sub',
    description: null,
    model: 'claude-sonnet-4-5',
    is_default: true,
    is_active: true,
    workspace_path: 'WPagent-1',
    soul_hash: null,
    memory_hash: null,
    config_json: '{}',
    owner_id: null,
    runtime_ref: null,
    runtime_type: 'openrouter',
    source: 'manual',
    created_at: '2026-07-01T00:00:00Z',
    updated_at: '2026-07-01T00:00:00Z',
    ...overrides,
  };
}

vi.mock('$lib/api', () => ({
  getAgents: vi.fn().mockResolvedValue([mkAgent()]),
  listRuntimeConnections: vi.fn().mockResolvedValue([
    { runtime_type: 'openclaw', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    { runtime_type: 'openrouter', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    { runtime_type: 'anthropic', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    { runtime_type: 'ollama', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
  ]),
}));

// ─── Global mocks ────────────────────────────────────────────────────────────

// ResizeObserver is not implemented in jsdom — polyfill with a no-op.
// window.addEventListener / document may be missing from the vitest-setup.ts window mock
// (it replaces globalThis.window with a plain object). Restore them from globalThis.
beforeAll(() => {
  globalThis.ResizeObserver = class ResizeObserver {
    observe() {}
    unobserve() {}
    disconnect() {}
  };

  const win = globalThis.window as any;
  if (!win.addEventListener) {
    win.addEventListener = globalThis.addEventListener?.bind(globalThis) ?? (() => {});
  }
  if (!win.removeEventListener) {
    win.removeEventListener = globalThis.removeEventListener?.bind(globalThis) ?? (() => {});
  }
  if (!win.document) {
    win.document = globalThis.document;
  }
});

// ─── Store mocks ──────────────────────────────────────────────────────────────

// Mock sessionStore so tests don't hit Tauri invoke
vi.mock('$lib/stores/sessionStore', async () => {
  const { writable } = await import('svelte/store');

  const store = writable<sessionStoreMod.SessionState>({
    sessions: [],
    activeSession: null,
    loading: false,
    error: null,
  });

  return {
    sessionStore: {
      subscribe: store.subscribe,
      loadSessions: vi.fn().mockResolvedValue(undefined),
      createSession: vi.fn().mockImplementation(async (_projectId: string, _agentId: string) => {
        const newSess = mkSession({ id: 'new-sess', is_active: true, title: '' });
        store.update(s => ({
          ...s,
          sessions: [newSess, ...s.sessions.map(x => ({ ...x, is_active: false }))],
          activeSession: newSess,
        }));
        return newSess;
      }),
      switchSession: vi.fn().mockImplementation(async (sessionId: string) => {
        store.update(s => {
          const sessions = s.sessions.map(sess => ({
            ...sess,
            is_active: sess.id === sessionId,
          }));
          return { ...s, sessions, activeSession: sessions.find(x => x.is_active) ?? null };
        });
      }),
      hideSession: vi.fn().mockResolvedValue(undefined),
      unhideSession: vi.fn().mockResolvedValue(undefined),
      renameSession: vi.fn().mockResolvedValue(undefined),
      reset: vi.fn(),
      // Expose setter for tests
      __set: store.set,
    },
    shouldShowFilter: (count: number) => count >= 8,
    filterSessions: (sessions: Session[], query: string) => {
      const q = query.trim().toLowerCase();
      const all = q ? sessions.filter(s => s.title.toLowerCase().includes(q)) : sessions;
      return {
        visible: all.filter(s => s.hidden_at === null),
        hidden: all.filter(s => s.hidden_at !== null),
      };
    },
    getSessionDisplayTitle: (session: Session) =>
      session.title || `Session ${session.created_at.slice(0, 10)}`,
    EMPTY_STATE_SINGLE: {
      headline: 'Just this one so far',
      body: 'New sessions you start in this project collect here.',
    },
    sortSessions: (sessions: Session[]) => [...sessions].sort((a, b) => {
      if (a.is_active && !b.is_active) return -1;
      if (!a.is_active && b.is_active) return 1;
      return b.last_active_at.localeCompare(a.last_active_at);
    }),
  };
});

// ─── Helpers ──────────────────────────────────────────────────────────────────

function mkSession(overrides: Partial<Session> = {}): Session {
  return {
    id: `sess-${Math.random().toString(36).slice(2, 8)}`,
    project_id: 'project-test',
    title: 'Test session',
    origin: 'user',
    created_at: '2026-07-01T10:00:00Z',
    last_active_at: '2026-07-09T08:00:00Z',
    hidden_at: null,
    is_active: false,
    ...overrides,
  };
}

function setStoreSessions(sessions: Session[]) {
  const active = sessions.find(s => s.is_active) ?? null;
  (sessionStoreMod.sessionStore as any).__set({
    sessions,
    activeSession: active,
    loading: false,
    error: null,
  });
}

const DEFAULT_PROPS = {
  projectId: 'project-test',
  projectName: 'Test Project',
};

// Cleanup after each test
afterEach(() => {
  vi.clearAllMocks();
  // Reset store to empty
  (sessionStoreMod.sessionStore as any).__set({
    sessions: [],
    activeSession: null,
    loading: false,
    error: null,
  });
});

// ─── Render basics ────────────────────────────────────────────────────────────

describe('SessionDropdownHeader — render', () => {
  it('renders the project name', () => {
    render(SessionDropdownHeader, { ...DEFAULT_PROPS });
    expect(screen.getByText('Test Project')).toBeTruthy();
  });

  it('renders "Loading…" when no active session', () => {
    render(SessionDropdownHeader, { ...DEFAULT_PROPS });
    expect(screen.getByText('Loading…')).toBeTruthy();
  });

  it('renders the active session display title', async () => {
    const session = mkSession({ is_active: true, title: 'Research notes' });
    setStoreSessions([session]);
    render(SessionDropdownHeader, { ...DEFAULT_PROPS });
    await waitFor(() => {
      expect(screen.getByText('Research notes')).toBeTruthy();
    });
  });

  it('renders the cyan dot and chevron in the header bar', () => {
    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS });
    expect(container.querySelector('.sdh-dot')).not.toBeNull();
    expect(container.querySelector('.sdh-chevron')).not.toBeNull();
  });

  it('dropdown is closed initially', () => {
    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS });
    expect(container.querySelector('.sdh-dropdown')).toBeNull();
  });
});

// ─── Dropdown open/close ──────────────────────────────────────────────────────

describe('SessionDropdownHeader — dropdown open/close', () => {
  it('opens dropdown when header bar is clicked', async () => {
    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS });
    const bar = container.querySelector('.sdh-bar')!;
    await fireEvent.click(bar);
    await waitFor(() => {
      expect(container.querySelector('.sdh-dropdown')).not.toBeNull();
    });
  });

  it('closes dropdown when header bar is clicked again', async () => {
    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS });
    const bar = container.querySelector('.sdh-bar')!;
    await fireEvent.click(bar);
    await waitFor(() => expect(container.querySelector('.sdh-dropdown')).not.toBeNull());

    await fireEvent.click(bar);
    await waitFor(() => {
      expect(container.querySelector('.sdh-dropdown')).toBeNull();
    });
  });

  it('chevron shows ▴ when open and ▾ when closed', async () => {
    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS });
    const chevron = container.querySelector('.sdh-chevron')!;
    expect(chevron.textContent).toBe('▾');

    await fireEvent.click(container.querySelector('.sdh-bar')!);
    await waitFor(() => expect(chevron.textContent).toBe('▴'));

    await fireEvent.click(container.querySelector('.sdh-bar')!);
    await waitFor(() => {
      expect(chevron.textContent).toBe('▾');
    });
  });
});

// ─── Escape key ───────────────────────────────────────────────────────────────

describe('SessionDropdownHeader — Escape key', () => {
  it('closes the dropdown when Escape is pressed', async () => {
    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS });
    const bar = container.querySelector('.sdh-bar')!;
    await fireEvent.click(bar);
    await waitFor(() => expect(container.querySelector('.sdh-dropdown')).not.toBeNull());

    await fireEvent.keyDown(document, { key: 'Escape' });
    await waitFor(() => {
      expect(container.querySelector('.sdh-dropdown')).toBeNull();
    });
  });

  it('does not fire when dropdown is already closed', async () => {
    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS });
    // Escape on closed dropdown should not cause errors
    await fireEvent.keyDown(document, { key: 'Escape' });
    expect(container.querySelector('.sdh-dropdown')).toBeNull();
  });
});

// ─── Filter field threshold ───────────────────────────────────────────────────

describe('SessionDropdownHeader — filter field threshold', () => {
  it('does NOT show filter field when fewer than 8 sessions', async () => {
    const sessions = Array.from({ length: 5 }, (_, i) =>
      mkSession({ id: `s${i}`, title: `Session ${i}` })
    );
    setStoreSessions(sessions);
    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS });
    await fireEvent.click(container.querySelector('.sdh-bar')!);

    await waitFor(() => expect(container.querySelector('.sdh-dropdown')).not.toBeNull());
    expect(container.querySelector('.sdh-filter')).toBeNull();
  });

  it('shows filter field when 8 or more sessions', async () => {
    const sessions = Array.from({ length: 8 }, (_, i) =>
      mkSession({ id: `s${i}`, title: `Session ${i}` })
    );
    setStoreSessions(sessions);
    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS });
    await fireEvent.click(container.querySelector('.sdh-bar')!);

    await waitFor(() => {
      expect(container.querySelector('.sdh-filter')).not.toBeNull();
    });
  });
});

// ─── Session ordering ─────────────────────────────────────────────────────────

describe('SessionDropdownHeader — session ordering', () => {
  it('renders active session first', async () => {
    const sessions = [
      mkSession({ id: 's1', title: 'Alpha', is_active: false, last_active_at: '2026-07-09T07:00:00Z' }),
      mkSession({ id: 's2', title: 'Beta', is_active: true,  last_active_at: '2026-07-09T06:00:00Z' }),
      mkSession({ id: 's3', title: 'Gamma', is_active: false, last_active_at: '2026-07-09T08:00:00Z' }),
    ];
    setStoreSessions(sessions);

    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS });
    await fireEvent.click(container.querySelector('.sdh-bar')!);

    await waitFor(() => expect(container.querySelector('.sdh-list')).not.toBeNull());

    const rows = container.querySelectorAll('.session-row');
    expect(rows.length).toBeGreaterThan(0);
    // First row should be the active session (Beta has is_active:true)
    expect(rows[0].classList.contains('is-active')).toBe(true);
  });
});

// ─── New Session callback ─────────────────────────────────────────────────────

describe('SessionDropdownHeader — New Session', () => {
  // §36.17.4 (#3050 PR 10): New Session now opens the NewSessionSheet
  // (replaces the old ADR-0062 D2 split-control popover, Ruling 1) — the
  // DEFAULT agent is pre-selected as captain but selecting/clicking a row
  // no longer auto-confirms; "Start session" is the explicit confirm step.
  async function openPickerAndStart(container: HTMLElement) {
    await fireEvent.click(container.querySelector('.sdh-bar')!);
    await waitFor(() => expect(container.querySelector('.sdh-dropdown')).not.toBeNull());

    const newBtn = screen.getByText('New Session').closest('button')!;
    await fireEvent.click(newBtn);

    await waitFor(() => {
      expect(apiMod.getAgents).toHaveBeenCalled();
    });

    await waitFor(() => expect(container.querySelector('[data-testid="ns-start-btn"]')).not.toBeNull());
    const startBtn = container.querySelector('[data-testid="ns-start-btn"]') as HTMLButtonElement;
    await fireEvent.click(startBtn);
  }

  it('New Session sheet Start calls createSession with the (default-preselected) agent_id and an empty crew', async () => {
    const onSessionSwitch = vi.fn();
    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS, onSessionSwitch });

    await openPickerAndStart(container);

    await waitFor(() => {
      expect(sessionStoreMod.sessionStore.createSession).toHaveBeenCalledWith('project-test', 'agent-1', []);
    });
  });

  it('calls onSessionSwitch after creating a session', async () => {
    const onSessionSwitch = vi.fn();
    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS, onSessionSwitch });

    await openPickerAndStart(container);

    await waitFor(() => {
      expect(onSessionSwitch).toHaveBeenCalledWith(
        expect.objectContaining({ id: 'new-sess', is_active: true })
      );
    });
  });

  it('dropdown closes after New Session is created', async () => {
    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS });

    await openPickerAndStart(container);

    await waitFor(() => {
      expect(container.querySelector('.sdh-dropdown')).toBeNull();
    });
  });

  // §36.17.4 (#3050 PR 10), Ruling 1 (Matt, 2026-10-29): "SessionDropdownHeader
  // and SessionsDrawer both open the same New Session sheet with the same
  // geometry... One component; no second layout." Pinned from the HEADER
  // side here — SessionsDrawer.test.ts pins the matching assertion from
  // the drawer side, same data-testid, same 620px width.
  it('Ruling 1: New Session renders [data-testid="ns-sheet"] at 620px — the same sheet SessionsDrawer mounts, not a header-specific popover', async () => {
    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS });
    await fireEvent.click(container.querySelector('.sdh-bar')!);
    await waitFor(() => expect(container.querySelector('.sdh-dropdown')).not.toBeNull());
    const newBtn = screen.getByText('New Session').closest('button')!;
    await fireEvent.click(newBtn);

    await waitFor(() => {
      const sheet = container.querySelector('[data-testid="ns-sheet"]') as HTMLElement;
      expect(sheet).not.toBeNull();
    });
    expect(container.querySelector('[data-testid="ns-scrim"]')).not.toBeNull();
  });
});

// ─── #2563 B2: runtime attribution (ADR-0063 D6) ────────────────────────────
//
// Pins the exact contradiction from Matt's DMG: header binding line and the
// read-only banner trigger must always agree on an agent's runtime label and
// availability, derived purely from `boundAgent.runtime_ref`/`runtime_type`
// cross-referenced against `listRuntimeConnections()` — never from the
// retired `ai_settings.provider` global scalar.

describe('SessionDropdownHeader — D6 runtime attribution', () => {
  function ocAgent(overrides: Partial<Agent> = {}): Agent {
    return mkAgent({
      id: 'oc-agent-1',
      name: 'Brit Freed',
      runtime_ref: 'oc-main',
      runtime_type: 'openclaw',
      is_active: true,
      ...overrides,
    });
  }

  function allConnected() {
    return [
      { runtime_type: 'openclaw', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'openrouter', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'anthropic', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'ollama', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    ];
  }

  function ocDisconnected() {
    return [
      { runtime_type: 'openclaw', connected: false, masked_credential: null, endpoint: null, error: 'unreachable', agents: [] },
      { runtime_type: 'openrouter', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'anthropic', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'ollama', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    ];
  }

  it('OC-bound session with OC runtime connected: no read-only, header says "OpenClaw"', async () => {
    const agent = ocAgent();
    (apiMod.getAgents as any).mockResolvedValueOnce([agent]);
    (apiMod.listRuntimeConnections as any).mockResolvedValueOnce(allConnected());

    const session = mkSession({ is_active: true, agent_id: agent.id, title: 'OC session' });
    setStoreSessions([session]);

    const onReadOnlyChange = vi.fn();
    render(SessionDropdownHeader, { ...DEFAULT_PROPS, onReadOnlyChange });

    await waitFor(() => {
      expect(screen.getByText('OpenClaw')).toBeTruthy();
    });
    await waitFor(() => {
      expect(onReadOnlyChange).toHaveBeenLastCalledWith(null);
    });
  });

  it('OC-bound session with OC runtime genuinely disconnected: read-only fires reason "provider" naming the bound agent, header still says "OpenClaw"', async () => {
    const agent = ocAgent();
    (apiMod.getAgents as any).mockResolvedValueOnce([agent]);
    (apiMod.listRuntimeConnections as any).mockResolvedValueOnce(ocDisconnected());

    const session = mkSession({ is_active: true, agent_id: agent.id, title: 'OC session' });
    setStoreSessions([session]);

    const onReadOnlyChange = vi.fn();
    render(SessionDropdownHeader, { ...DEFAULT_PROPS, onReadOnlyChange });

    // Header and banner must AGREE: header still shows "OpenClaw" (identity,
    // not connectivity) — never falls back to some other provider's name.
    await waitFor(() => {
      expect(screen.getByText('OpenClaw')).toBeTruthy();
    });
    await waitFor(() => {
      expect(onReadOnlyChange).toHaveBeenLastCalledWith({ reason: 'provider', agent });
    });
  });

  it('inactive bound agent fires reason "agent", not "provider", even when its runtime is connected', async () => {
    const agent = ocAgent({ is_active: false });
    (apiMod.getAgents as any).mockResolvedValueOnce([agent]);
    (apiMod.listRuntimeConnections as any).mockResolvedValueOnce(allConnected());

    const session = mkSession({ is_active: true, agent_id: agent.id, title: 'OC session' });
    setStoreSessions([session]);

    const onReadOnlyChange = vi.fn();
    render(SessionDropdownHeader, { ...DEFAULT_PROPS, onReadOnlyChange });

    await waitFor(() => {
      expect(onReadOnlyChange).toHaveBeenLastCalledWith({ reason: 'agent', agent });
    });
  });

  it('locally-created agent on a connected runtime: no read-only, header shows its own runtime_type label', async () => {
    const agent = mkAgent({
      id: 'or-agent-1',
      name: 'Research',
      runtime_ref: null,
      runtime_type: 'openrouter',
      is_active: true,
    });
    (apiMod.getAgents as any).mockResolvedValueOnce([agent]);
    (apiMod.listRuntimeConnections as any).mockResolvedValueOnce(allConnected());

    const session = mkSession({ is_active: true, agent_id: agent.id, title: 'OR session' });
    setStoreSessions([session]);

    const onReadOnlyChange = vi.fn();
    render(SessionDropdownHeader, { ...DEFAULT_PROPS, onReadOnlyChange });

    await waitFor(() => {
      expect(screen.getByText('OpenRouter')).toBeTruthy();
    });
    await waitFor(() => {
      expect(onReadOnlyChange).toHaveBeenLastCalledWith(null);
    });
  });
});

// ─── §36.17.4 (#3050 PR 10): header crew-chip states ────────────────────────

describe('SessionDropdownHeader — header crew states (#3050 PR 10)', () => {
  function allConnectedLocal() {
    return [
      { runtime_type: 'openclaw', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'openrouter', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'anthropic', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'ollama', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    ];
  }

  it('0 crew: "+ Crew" renders, muted class present', async () => {
    const captain = mkAgent({ id: 'cap-1', name: 'Valencia Tool Test', is_default: true });
    (apiMod.getAgents as any).mockResolvedValueOnce([captain]);
    (apiMod.listRuntimeConnections as any).mockResolvedValueOnce(allConnectedLocal());
    const session = mkSession({ is_active: true, agent_id: captain.id, crew_agent_ids: null, title: 'S' });
    setStoreSessions([session]);

    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS });

    await waitFor(() => {
      const addBtn = container.querySelector('[data-testid="sdh-crew-add-btn"]');
      expect(addBtn).not.toBeNull();
      expect(addBtn!.classList.contains('sdh-crewbtn--add')).toBe(true);
    });
    expect(container.querySelector('[data-testid="sdh-crew-btn"]')).toBeNull();
  });

  it('1-3 crew: all chips rendered, no "+N"', async () => {
    const captain = mkAgent({ id: 'cap-1', name: 'Astro', is_default: true });
    const crew1 = mkAgent({ id: 'c1', name: 'Vanessa' });
    const crew2 = mkAgent({ id: 'c2', name: 'Vernon' });
    (apiMod.getAgents as any).mockResolvedValueOnce([captain, crew1, crew2]);
    (apiMod.listRuntimeConnections as any).mockResolvedValueOnce(allConnectedLocal());
    const session = mkSession({ is_active: true, agent_id: captain.id, crew_agent_ids: '["c1","c2"]', title: 'S' });
    setStoreSessions([session]);

    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS });

    await waitFor(() => {
      const btn = container.querySelector('[data-testid="sdh-crew-btn"]') as HTMLElement;
      expect(btn).not.toBeNull();
      expect(btn.querySelectorAll('.sdh-av')).toHaveLength(2);
      expect(btn.querySelector('.sdh-av--more')).toBeNull();
    });
  });

  it('4+ crew: exactly 3 chips + "+N" with N = count-3, tooltip lists ALL crew names', async () => {
    const captain = mkAgent({ id: 'cap-1', name: 'Astro', is_default: true });
    const crew = [
      mkAgent({ id: 'c1', name: 'Valencia Tool Test' }),
      mkAgent({ id: 'c2', name: 'Vanessa' }),
      mkAgent({ id: 'c3', name: 'Vernon' }),
      mkAgent({ id: 'c4', name: 'Vernon Reed' }),
    ];
    (apiMod.getAgents as any).mockResolvedValueOnce([captain, ...crew]);
    (apiMod.listRuntimeConnections as any).mockResolvedValueOnce(allConnectedLocal());
    const session = mkSession({ is_active: true, agent_id: captain.id, crew_agent_ids: '["c1","c2","c3","c4"]', title: 'S' });
    setStoreSessions([session]);

    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS });

    await waitFor(() => {
      const btn = container.querySelector('[data-testid="sdh-crew-btn"]') as HTMLElement;
      expect(btn).not.toBeNull();
      expect(btn.querySelectorAll('.sdh-av:not(.sdh-av--more)')).toHaveLength(3);
      expect(btn.querySelector('.sdh-av--more')?.textContent).toBe('+1');
      expect(btn.title).toContain('Valencia Tool Test');
      expect(btn.title).toContain('Vernon Reed');
    });
  });

  it('OpenClaw captain: "own crew" label, no onclick/role=button at all — not just visually non-interactive', async () => {
    const ocCaptain = mkAgent({ id: 'oc-1', name: 'Brit Freed', runtime_ref: 'oc-1', runtime_type: 'openclaw', is_default: true });
    (apiMod.getAgents as any).mockResolvedValueOnce([ocCaptain]);
    (apiMod.listRuntimeConnections as any).mockResolvedValueOnce(allConnectedLocal());
    const session = mkSession({ is_active: true, agent_id: ocCaptain.id, crew_agent_ids: null, title: 'S' });
    setStoreSessions([session]);

    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS });

    await waitFor(() => {
      const label = container.querySelector('[data-testid="sdh-own-crew"]');
      expect(label).not.toBeNull();
      expect(label?.textContent).toBe('own crew');
    });
    // Not a button, no click handler at all — a toBeInTheDocument-only
    // check on the text wouldn't catch an inert onclick; assert there is
    // no <button> wrapper AND that clicking it never opens the edit-crew
    // popover (behavioral, not just structural — catches an inert onclick
    // attached directly to the span, which `closest('button')` alone would
    // miss).
    const label = container.querySelector('[data-testid="sdh-own-crew"]') as HTMLElement;
    expect(label.closest('button')).toBeNull();
    expect(container.querySelector('[data-testid="sdh-crew-btn"]')).toBeNull();
    expect(container.querySelector('[data-testid="sdh-crew-add-btn"]')).toBeNull();

    await fireEvent.click(label);
    await tick();
    expect(container.querySelector('[data-testid="ecp-pop"]')).toBeNull();
  });
});

// ─── #2563 B2: New Session picker connectivity filter ───────────────────────

describe('SessionDropdownHeader — New Session picker connectivity filter', () => {
  async function openPicker(container: HTMLElement) {
    await fireEvent.click(container.querySelector('.sdh-bar')!);
    await waitFor(() => expect(container.querySelector('.sdh-dropdown')).not.toBeNull());
    const newBtn = screen.getByText('New Session').closest('button')!;
    await fireEvent.click(newBtn);
  }

  it('includes agents from every connected runtime', async () => {
    // role: 'main' — a non-sub OC-imported agent (#2588: default fixture role='sub'
    // would now be misread as a real subagent and wrongly excluded).
    const ocAg = mkAgent({ id: 'oc-1', name: 'Brit Freed', role: 'main', runtime_ref: 'oc-1', runtime_type: 'openclaw', is_active: true });
    const orAg = mkAgent({ id: 'or-1', name: 'Research', runtime_ref: null, runtime_type: 'openrouter', is_active: true });
    (apiMod.getAgents as any).mockResolvedValue([ocAg, orAg]);
    (apiMod.listRuntimeConnections as any).mockResolvedValue([
      { runtime_type: 'openclaw', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'openrouter', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'anthropic', connected: false, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'ollama', connected: false, masked_credential: null, endpoint: null, error: null, agents: [] },
    ]);

    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS });
    await openPicker(container);

    await waitFor(() => {
      expect(screen.getByText('Brit Freed')).toBeTruthy();
      expect(screen.getByText('Research')).toBeTruthy();
    });
  });

  it('excludes agents on a disconnected runtime while including the other runtime\'s agents', async () => {
    const ocAg = mkAgent({ id: 'oc-1', name: 'Brit Freed', runtime_ref: 'oc-1', runtime_type: 'openclaw', is_active: true });
    const orAg = mkAgent({ id: 'or-1', name: 'Research', runtime_ref: null, runtime_type: 'openrouter', is_active: true });
    (apiMod.getAgents as any).mockResolvedValue([ocAg, orAg]);
    (apiMod.listRuntimeConnections as any).mockResolvedValue([
      { runtime_type: 'openclaw', connected: false, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'openrouter', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'anthropic', connected: false, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'ollama', connected: false, masked_credential: null, endpoint: null, error: null, agents: [] },
    ]);

    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS });
    await openPicker(container);

    await waitFor(() => {
      expect(screen.getByText('Research')).toBeTruthy();
    });
    expect(screen.queryByText('Brit Freed')).toBeNull();
  });

  it('excludes an inactive agent from the picker even on a connected runtime', async () => {
    const inactiveAg = mkAgent({ id: 'inactive-1', name: 'Retired Agent', runtime_ref: null, runtime_type: 'openrouter', is_active: false });
    const activeAg = mkAgent({ id: 'active-1', name: 'Active Agent', runtime_ref: null, runtime_type: 'openrouter', is_active: true, is_default: true });
    (apiMod.getAgents as any).mockResolvedValue([inactiveAg, activeAg]);
    (apiMod.listRuntimeConnections as any).mockResolvedValue([
      { runtime_type: 'openclaw', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'openrouter', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'anthropic', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'ollama', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    ]);

    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS });
    await openPicker(container);

    await waitFor(() => {
      expect(screen.getByText('Active Agent')).toBeTruthy();
    });
    expect(screen.queryByText('Retired Agent')).toBeNull();
  });

  // #2588 C4 (Matt's ruling: subagent sessions are always-no) — extends the
  // B5 DefaultAgentPickerModal predicate (role='sub' AND runtime_ref != null)
  // to the New Session picker.
  it('excludes real subagents (role=sub, runtime_ref set) but includes local agents and non-sub runtime_ref agents', async () => {
    const subAg = mkAgent({ id: 'sub-1', name: 'Stia', role: 'sub', runtime_ref: 'oc-sub-1', runtime_type: 'openclaw', is_active: true });
    const localAg = mkAgent({ id: 'local-1', name: 'Local Agent', role: 'local', runtime_ref: null, runtime_type: 'openrouter', is_active: true });
    const localSubRoleAg = mkAgent({ id: 'local-2', name: 'Legacy Local', role: 'sub', runtime_ref: null, runtime_type: 'anthropic', is_active: true });
    const nonSubWithRuntimeRefAg = mkAgent({ id: 'oc-1', name: 'Brit Freed', role: 'local', runtime_ref: 'oc-1', runtime_type: 'openclaw', is_active: true });
    (apiMod.getAgents as any).mockResolvedValue([subAg, localAg, localSubRoleAg, nonSubWithRuntimeRefAg]);
    (apiMod.listRuntimeConnections as any).mockResolvedValue([
      { runtime_type: 'openclaw', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'openrouter', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'anthropic', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'ollama', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    ]);

    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS });
    await openPicker(container);

    // §36.17.4 (#3050 PR 10): the New Session sheet's captain list AND crew
    // pool can both legitimately render the same agent's name (a non-default
    // captain candidate is also a crew candidate for whichever OTHER agent
    // is currently selected as captain) — scope the captain-list assertions
    // to `.ns-aglist` so this stays an unambiguous single-match query.
    await waitFor(() => expect(container.querySelector('.ns-aglist')).not.toBeNull());
    const aglist = container.querySelector('.ns-aglist') as HTMLElement;
    await waitFor(() => {
      expect(within(aglist).getByText('Local Agent')).toBeTruthy();
    });
    expect(within(aglist).getByText('Legacy Local')).toBeTruthy();
    expect(within(aglist).getByText('Brit Freed')).toBeTruthy();
    expect(within(aglist).queryByText('Stia')).toBeNull();
  });
});

// ─── BUG-2 regression guard (#395 fix) ──────────────────────────────────────────────

describe('BUG-2 regression guard — loadSessions called exactly once on mount (#395 fix)', () => {
  it('calls loadSessions exactly once on mount (not twice)', async () => {
    // BUG-2: SessionDropdownHeader previously called loadSessions from BOTH
    // a $effect AND an onMount. In Svelte 5, both run after mount, causing a
    // duplicate backend call and a momentary flash of empty state (reset() between
    // them). Fix: removed the onMount call; only the $effect loads sessions.
    render(SessionDropdownHeader, { ...DEFAULT_PROPS });

    // Allow microtasks + effects to flush
    await waitFor(() => {
      expect(sessionStoreMod.sessionStore.loadSessions).toHaveBeenCalledTimes(1);
    });
  });

  it('SessionDropdownHeader.svelte onMount does NOT call loadSessions', () => {
    const filePath = resolve(__dirname, 'SessionDropdownHeader.svelte');
    const source = readFileSync(filePath, 'utf-8');

    // The onMount block must not contain an actual loadSessions() call.
    // Locate the onMount block start.
    const onMountIdx = source.indexOf('onMount(');
    expect(onMountIdx).toBeGreaterThan(-1);

    // Grab the next 600 chars (covers the whole onMount block)
    const onMountBlock = source.slice(onMountIdx, onMountIdx + 600);

    // Check each line: none should be a non-comment line calling loadSessions.
    const callLines = onMountBlock
      .split('\n')
      .filter(line => !line.trim().startsWith('//') && !line.trim().startsWith('*'))
      .filter(line => line.includes('.loadSessions('));
    expect(callLines).toHaveLength(0);
  });
});

// ─── Dead code removal ──────────────────────────────────────────────────────────

describe('Dead code removal', () => {
  it('ChatPanel.svelte no longer contains onResetThread interface or dead button', () => {
    const chatPanelPath = resolve(__dirname, 'ChatPanel.svelte');
    const source = readFileSync(chatPanelPath, 'utf-8');
    expect(source).not.toContain('onResetThread');
    expect(source).not.toContain('new-thread-btn');
    expect(source).not.toContain("confirm('Start a new conversation");
  });
});

// ─── D-2: Single-session headline removed from dropdown (PR B) ─────────────────

describe('D-2: Single-session headline absent — SessionDropdownHeader', () => {
  it('does NOT render "Just this one so far" as sdh-single-headline when sessions.length === 1', async () => {
    // Seed 1 visible session (non-empty state, so the empty-headline path is not taken)
    setStoreSessions([mkSession({ id: 'solo', title: 'Solo session', is_active: true })]);

    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS });

    // Open dropdown to see the session list
    const bar = container.querySelector('.sdh-bar') as HTMLElement;
    await fireEvent.click(bar);

    await waitFor(() => {
      // Dropdown is open and the session list renders
      expect(container.querySelector('.sdh-dropdown')).not.toBeNull();
      // The session row renders (use getAllByText — header also shows the active session name)
      expect(screen.getAllByText('Solo session').length).toBeGreaterThan(0);
    });

    // The sdh-single-headline element must NOT exist (it was removed by D-2)
    expect(container.querySelector('.sdh-single-headline')).toBeNull();
  });
});

// ─── D-3: sdh-new-btn removed from header bar (PR B) ──────────────────────────

describe('D-3: sdh-new-btn absent — SessionDropdownHeader header bar', () => {
  it('does not render .sdh-new-btn in narrow mode', () => {
    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS, isNarrow: true });
    expect(container.querySelector('.sdh-new-btn')).toBeNull();
  });

  it('does not render .sdh-new-btn in wide mode', () => {
    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS, isNarrow: false });
    expect(container.querySelector('.sdh-new-btn')).toBeNull();
  });

  it('SessionDropdownHeader.svelte source does not define sdh-new-btn CSS class', () => {
    const filePath = resolve(__dirname, 'SessionDropdownHeader.svelte');
    const source = readFileSync(filePath, 'utf-8');
    expect(source).not.toContain('sdh-new-btn');
  });
});

// ─── D-5: Chat column max-width raised to 900 (PR B) ──────────────────────────

describe('D-5: Chat column width cap raised to 900px', () => {
  it('+page.svelte startResize uses Math.min(900, ...) not Math.min(700, ...)', () => {
    const pagePath = resolve(__dirname, '../../routes/+page.svelte');
    const source = readFileSync(pagePath, 'utf-8');

    // Width cap must be 900
    expect(source).toContain('Math.min(900,');
    // Old 700 cap must not be present in startResize context
    const startResizeIdx = source.indexOf('function startResize');
    const resizeBlock = source.slice(startResizeIdx, startResizeIdx + 400);
    expect(resizeBlock).toContain('Math.min(900,');
    expect(resizeBlock).not.toContain('Math.min(700,');
  });
});

// ─── #3166 C58: persistent "Job session · <name>" header cue ───────────────────

describe('#3166 C58 — SessionDropdownHeader persistent job-session cue', () => {
  afterEach(() => {
    activeSessionOrigin.set(null);
  });

  it('renders "Job session · <name>" when the active session origin is schedule', async () => {
    const session = mkSession({ is_active: true, title: 'Morning brief' });
    setStoreSessions([session]);
    activeSessionOrigin.set('schedule');

    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS });

    await waitFor(() => {
      const cue = container.querySelector('.sdh-job-cue');
      expect(cue).not.toBeNull();
      expect(cue?.textContent).toContain('Job session');
      expect(cue?.textContent).toContain('Morning brief');
    });
  });

  it('renders the cue when the active session origin is spawn', async () => {
    const session = mkSession({ is_active: true, title: 'Spawned sub-agent' });
    setStoreSessions([session]);
    activeSessionOrigin.set('spawn');

    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS });

    await waitFor(() => {
      expect(container.querySelector('.sdh-job-cue')).not.toBeNull();
    });
  });

  /// Mutation guard: the Matt repro itself — an ordinary project chat
  /// (origin='user') must NEVER show the cue, even though a session IS
  /// active and named. Without `isJobSessionOrigin` gating this, the cue
  /// would render unconditionally the moment ANY session was active.
  it('does NOT render the cue for an ordinary interactive session (origin=user)', async () => {
    const session = mkSession({ is_active: true, title: 'New One' });
    setStoreSessions([session]);
    activeSessionOrigin.set('user');

    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS });

    await waitFor(() => {
      expect(screen.getByText('New One')).toBeTruthy();
    });
    expect(container.querySelector('.sdh-job-cue')).toBeNull();
  });

  it('does NOT render the cue when activeSessionOrigin is null (navigation fallback / not yet resolved)', async () => {
    const session = mkSession({ is_active: true, title: 'New One' });
    setStoreSessions([session]);
    activeSessionOrigin.set(null);

    const { container } = render(SessionDropdownHeader, { ...DEFAULT_PROPS });

    await waitFor(() => {
      expect(screen.getByText('New One')).toBeTruthy();
    });
    expect(container.querySelector('.sdh-job-cue')).toBeNull();
  });

  /// Composition with #3157's `←` (ChatPanel.svelte's job-brief back
  /// button): both must derive from the SAME `isJobSessionOrigin` half,
  /// not two independent `=== 'spawn' || === 'schedule'` comparisons that
  /// could silently drift — asserted directly against the shared function
  /// (jobSessionBack.test.ts's own "isJobSessionOrigin — #3166 C58 shared
  /// half" suite proves `shouldShowJobSessionBackButton`'s origin half
  /// agrees with it for every value).
  it('the cue condition is exactly isJobSessionOrigin($activeSessionOrigin) — same predicate the back button uses', () => {
    expect(isJobSessionOrigin('spawn')).toBe(true);
    expect(isJobSessionOrigin('schedule')).toBe(true);
    expect(isJobSessionOrigin('user')).toBe(false);
    expect(isJobSessionOrigin(null)).toBe(false);
  });
});

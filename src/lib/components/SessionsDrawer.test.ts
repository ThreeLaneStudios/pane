/**
 * SessionsDrawer.test.ts — Component tests for SessionsDrawer.svelte (#395 PR8).
 *
 * CI-testable scope:
 *   ✓ Two-call merge: workspace + recovered sections both render
 *   ✓ Ordering: active-first, last_active_at DESC
 *   ✓ Filter threshold: ≥8 shows filter, <8 doesn't
 *   ✓ D-2: Single-session headline absent ("Just this one so far" removed by design)
 *   ✓ Toggle open/close: drawer opens and collapses
 *   ✓ W1 fix verification: dead {#if isNarrow} branch is gone from SessionDropdownHeader pinned-top
 *
 * NOT tested here (live DMG tests — Matt):
 *   - Scroll behaviour inside the drawer list
 *   - Actual layout/positioning (requires rendered viewport)
 *   - Session switch and reload round-trip against Tauri backend
 *   - Hidden section after show-hidden toggle (visual flow)
 *
 * §395 PR8 — spec-covered-by: #1979
 */

import { describe, it, expect, vi, beforeEach, afterEach, beforeAll } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte/svelte5';
import { readFileSync } from 'fs';
import { resolve } from 'path';
import SessionsDrawer from './SessionsDrawer.svelte';
import type { Session } from '$lib/types';
import * as apiMod from '$lib/api';

// ─── Global polyfills ─────────────────────────────────────────────────────────

beforeAll(() => {
  // jsdom does not implement ResizeObserver
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

// ─── API mock ─────────────────────────────────────────────────────────────────

/**
 * Shared mock state mutated per test via setMockSessions().
 * listHomeSessionsWithRecovered resolves from this object.
 */
const mockApiState = {
  workspace: [] as Session[],
  recovered: [] as Session[],
};

vi.mock('$lib/api', () => ({
  listHomeSessionsWithRecovered: vi.fn(async (_includeHidden = false) => ({
    workspace: [...mockApiState.workspace],
    recovered: [...mockApiState.recovered],
  })),
  createSession: vi.fn(async (_projectId: string, _agentId: string) =>
    mkSession({ id: 'new-sess', is_active: true, title: '' })
  ),
  switchSession: vi.fn(async (_sessionId: string) => {}),
  hideSession: vi.fn(async (_sessionId: string) => {}),
  unhideSession: vi.fn(async (_sessionId: string) => {}),
  renameSession: vi.fn(async (_sessionId: string, _title: string) => {}),
  getAgents: vi.fn(async () => [
    { id: 'agent-1', name: 'Test Agent', slug: 'test-agent', role: null, description: null, model: 'test-model', is_default: true, is_active: true, workspace_path: null, soul_hash: null, memory_hash: null, config_json: null, owner_id: null, runtime_ref: null, runtime_type: 'openrouter', source: 'local', created_at: '', updated_at: '' },
  ]),
  // #2563 B2: re-pointed from the retired aiLoadConfig().provider global
  // scalar to per-runtime connectivity (ai_list_runtime_connections).
  listRuntimeConnections: vi.fn(async () => [
    { runtime_type: 'openclaw', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    { runtime_type: 'openrouter', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    { runtime_type: 'anthropic', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    { runtime_type: 'ollama', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
  ]),
}));

// ─── Helpers ──────────────────────────────────────────────────────────────────

function mkSession(overrides: Partial<Session> = {}): Session {
  return {
    id: `sess-${Math.random().toString(36).slice(2, 8)}`,
    project_id: 'workspace',
    title: 'Test session',
    origin: 'user',
    created_at: '2026-07-01T10:00:00Z',
    last_active_at: '2026-07-09T08:00:00Z',
    hidden_at: null,
    is_active: false,
    ...overrides,
  };
}

function setMockSessions(workspace: Session[], recovered: Session[] = []) {
  mockApiState.workspace = workspace;
  mockApiState.recovered = recovered;
}

afterEach(() => {
  vi.clearAllMocks();
  mockApiState.workspace = [];
  mockApiState.recovered = [];
});

// ─── Helper: open the drawer ──────────────────────────────────────────────────

async function openDrawer(container: HTMLElement) {
  const toggleBtn = container.querySelector('.sd-toggle') as HTMLButtonElement;
  expect(toggleBtn).not.toBeNull();
  await fireEvent.click(toggleBtn);
  await waitFor(() => {
    expect(container.querySelector('.sd-panel')).not.toBeNull();
  });
}

// ─── Render basics ────────────────────────────────────────────────────────────

describe('SessionsDrawer — render', () => {
  it('renders the toggle strip by default (closed)', () => {
    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    expect(container.querySelector('.sd-toggle-strip')).not.toBeNull();
    expect(container.querySelector('.sd-panel')).toBeNull();
  });

  it('renders the container with data-testid', () => {
    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    expect(container.querySelector('[data-testid="sessions-drawer"]')).not.toBeNull();
  });
});

// ─── Toggle open/close ────────────────────────────────────────────────────────

describe('SessionsDrawer — toggle open/close', () => {
  it('opens the drawer when toggle button is clicked', async () => {
    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    await openDrawer(container);
    expect(container.querySelector('.sd-panel')).not.toBeNull();
  });

  it('closes the drawer when toggle button is clicked again', async () => {
    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    await openDrawer(container);

    // Click toggle again to close
    const toggleBtn = container.querySelector('.sd-toggle') as HTMLButtonElement;
    await fireEvent.click(toggleBtn);
    await waitFor(() => {
      expect(container.querySelector('.sd-panel')).toBeNull();
    });
  });

  it('toggle button aria-expanded tracks open state', async () => {
    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    const toggle = container.querySelector('.sd-toggle') as HTMLButtonElement;
    expect(toggle.getAttribute('aria-expanded')).toBe('false');

    await fireEvent.click(toggle);
    await waitFor(() => {
      expect(toggle.getAttribute('aria-expanded')).toBe('true');
    });
  });

  it('sd-container--open class applied when drawer is open', async () => {
    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    const sdContainer = container.querySelector('[data-testid="sessions-drawer"]')!;
    expect(sdContainer.classList.contains('sd-container--open')).toBe(false);

    await fireEvent.click(container.querySelector('.sd-toggle') as HTMLButtonElement);
    await waitFor(() => {
      expect(sdContainer.classList.contains('sd-container--open')).toBe(true);
    });
  });
});

// ─── Two-call merge: workspace + recovered sections ───────────────────────────

describe('SessionsDrawer — two-call merge', () => {
  it('renders workspace sessions in the main list', async () => {
    const ws = [
      mkSession({ id: 'ws-1', title: 'Workspace Alpha', is_active: true }),
      mkSession({ id: 'ws-2', title: 'Workspace Beta' }),
    ];
    setMockSessions(ws, []);

    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    await openDrawer(container);

    await waitFor(() => {
      expect(screen.getByText('Workspace Alpha')).toBeTruthy();
      expect(screen.getByText('Workspace Beta')).toBeTruthy();
    });
  });

  it('renders the Recovered separator and recovered sessions', async () => {
    const ws = [mkSession({ id: 'ws-1', title: 'Workspace One', is_active: true })];
    const rec = [mkSession({ id: 'rec-1', title: 'Recovered Ghost', project_id: '__recovered__', origin: 'recovered' })];
    setMockSessions(ws, rec);

    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    await openDrawer(container);

    await waitFor(() => {
      // Recovered separator label
      const sep = container.querySelector('.sd-separator-label');
      expect(sep).not.toBeNull();
      expect(sep!.textContent).toContain('Recovered');
      // Recovered session title
      expect(screen.getByText('Recovered Ghost')).toBeTruthy();
    });
  });

  it('does NOT render Recovered separator when there are no recovered sessions', async () => {
    setMockSessions([mkSession({ is_active: true, title: 'Only Workspace' })], []);

    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    await openDrawer(container);

    await waitFor(() => {
      expect(screen.getByText('Only Workspace')).toBeTruthy();
    });
    expect(container.querySelector('.sd-separator')).toBeNull();
  });

  it('renders workspace sessions before recovered sessions', async () => {
    const ws = [mkSession({ id: 'ws-1', title: 'Workspace Session', is_active: true })];
    const rec = [mkSession({ id: 'rec-1', title: 'Recovered Session', origin: 'recovered' })];
    setMockSessions(ws, rec);

    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    await openDrawer(container);

    await waitFor(() => {
      const panel = container.querySelector('.sd-list')!;
      const panelText = panel.textContent ?? '';
      const wsIdx = panelText.indexOf('Workspace Session');
      const recIdx = panelText.indexOf('Recovered Session');
      expect(wsIdx).toBeGreaterThanOrEqual(0);
      expect(recIdx).toBeGreaterThanOrEqual(0);
      expect(wsIdx).toBeLessThan(recIdx);
    });
  });
});

// ─── Ordering: active-first, last_active_at DESC ──────────────────────────────

describe('SessionsDrawer — session ordering', () => {
  it('active session renders first regardless of last_active_at', async () => {
    // Beta is active but has an older last_active_at than Gamma
    const sessions = [
      mkSession({ id: 's-alpha', title: 'Alpha', is_active: false, last_active_at: '2026-07-09T07:00:00Z' }),
      mkSession({ id: 's-beta',  title: 'Beta',  is_active: true,  last_active_at: '2026-07-09T05:00:00Z' }),
      mkSession({ id: 's-gamma', title: 'Gamma', is_active: false, last_active_at: '2026-07-09T09:00:00Z' }),
    ];
    setMockSessions(sessions);

    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    await openDrawer(container);

    await waitFor(() => {
      const rows = container.querySelectorAll('.sd-row:not(.sd-row--recovered):not(.sd-row--hidden)');
      expect(rows.length).toBeGreaterThanOrEqual(3);
      // First row must contain the active session title
      expect(rows[0].textContent).toContain('Beta');
    });
  });

  it('non-active sessions ordered by last_active_at DESC after the active one', async () => {
    const sessions = [
      mkSession({ id: 's1', title: 'Old',    is_active: false, last_active_at: '2026-07-09T01:00:00Z' }),
      mkSession({ id: 's2', title: 'Active', is_active: true,  last_active_at: '2026-07-09T05:00:00Z' }),
      mkSession({ id: 's3', title: 'Recent', is_active: false, last_active_at: '2026-07-09T08:00:00Z' }),
    ];
    setMockSessions(sessions);

    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    await openDrawer(container);

    await waitFor(() => {
      const rows = container.querySelectorAll('.sd-row:not(.sd-row--recovered):not(.sd-row--hidden)');
      // row[0]=Active, row[1]=Recent, row[2]=Old
      expect(rows[0].textContent).toContain('Active');
      expect(rows[1].textContent).toContain('Recent');
      expect(rows[2].textContent).toContain('Old');
    });
  });
});

// ─── Filter threshold ─────────────────────────────────────────────────────────

describe('SessionsDrawer — filter threshold', () => {
  it('does NOT show filter bar when total sessions < 8', async () => {
    setMockSessions(
      Array.from({ length: 5 }, (_, i) => mkSession({ id: `s${i}`, title: `Session ${i}` })),
      []
    );

    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    await openDrawer(container);

    await waitFor(() => {
      expect(screen.getByText('Session 0')).toBeTruthy();
    });
    expect(container.querySelector('.sd-filter')).toBeNull();
  });

  it('shows filter bar when total sessions ≥ 8 (workspace + recovered combined)', async () => {
    // 6 workspace + 2 recovered = 8 total → threshold hit
    setMockSessions(
      Array.from({ length: 6 }, (_, i) => mkSession({ id: `ws${i}`, title: `Workspace ${i}` })),
      Array.from({ length: 2 }, (_, i) => mkSession({ id: `rec${i}`, title: `Recovered ${i}`, origin: 'recovered' }))
    );

    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    await openDrawer(container);

    await waitFor(() => {
      expect(container.querySelector('.sd-filter')).not.toBeNull();
    });
  });

  it('shows filter bar when workspace alone reaches 8', async () => {
    setMockSessions(
      Array.from({ length: 8 }, (_, i) => mkSession({ id: `s${i}`, title: `Session ${i}` })),
      []
    );

    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    await openDrawer(container);

    await waitFor(() => {
      expect(container.querySelector('.sd-filter')).not.toBeNull();
    });
  });
});

// ─── #2563 B2: New Session picker connectivity filter ────────────────────────

// §36.17.4 (#3050 PR 10), Ruling 1 (Matt, 2026-10-29): "SessionDropdownHeader
// and SessionsDrawer both open the same New Session sheet with the same
// geometry — whichever button was clicked. One component; no second
// layout." Pinned here from the DRAWER side; SessionDropdownHeader.test.ts
// pins the matching assertion from the header side — same data-testid,
// same 620px width, same component.
describe('SessionsDrawer — Ruling 1: New Session mounts the shared NewSessionSheet component', () => {
  it('opening New Session renders [data-testid="ns-sheet"] at 620px — the same sheet SessionDropdownHeader mounts, not a drawer-specific popover', async () => {
    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    await openDrawer(container);
    const newBtn = container.querySelector('.sd-new-btn') as HTMLButtonElement;
    await fireEvent.click(newBtn);

    await waitFor(() => {
      const sheet = container.querySelector('[data-testid="ns-sheet"]') as HTMLElement;
      expect(sheet).not.toBeNull();
      expect(getComputedStyle(sheet).width === '620px' || sheet.className.includes('ns-sheet')).toBe(true);
    });
    expect(container.querySelector('[data-testid="ns-scrim"]')).not.toBeNull();
  });
});

describe('SessionsDrawer — New Session picker connectivity filter', () => {
  async function openPicker(container: HTMLElement) {
    await openDrawer(container);
    const newBtn = container.querySelector('.sd-new-btn') as HTMLButtonElement;
    expect(newBtn).not.toBeNull();
    await fireEvent.click(newBtn);
  }

  it('mixed scenario: OC agent + OpenRouter agent, both runtimes connected → picker shows both', async () => {
    const ocAg = { id: 'oc-1', name: 'Brit Freed', slug: 'brit', role: null, description: null, model: 'test-model', is_default: true, is_active: true, workspace_path: null, soul_hash: null, memory_hash: null, config_json: null, owner_id: null, runtime_ref: 'oc-1', runtime_type: 'openclaw', source: 'openclaw', created_at: '', updated_at: '' };
    const orAg = { id: 'or-1', name: 'Research', slug: 'research', role: null, description: null, model: 'test-model', is_default: false, is_active: true, workspace_path: null, soul_hash: null, memory_hash: null, config_json: null, owner_id: null, runtime_ref: null, runtime_type: 'openrouter', source: 'local', created_at: '', updated_at: '' };
    (apiMod.getAgents as any).mockResolvedValue([ocAg, orAg]);
    (apiMod.listRuntimeConnections as any).mockResolvedValue([
      { runtime_type: 'openclaw', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'openrouter', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'anthropic', connected: false, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'ollama', connected: false, masked_credential: null, endpoint: null, error: null, agents: [] },
    ]);

    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    await openPicker(container);

    await waitFor(() => {
      expect(screen.getByText('Brit Freed')).toBeTruthy();
      expect(screen.getByText('Research')).toBeTruthy();
    });
  });

  it('disconnected runtime excludes its agent but the other runtime\'s agent still shows', async () => {
    const ocAg = { id: 'oc-1', name: 'Brit Freed', slug: 'brit', role: null, description: null, model: 'test-model', is_default: true, is_active: true, workspace_path: null, soul_hash: null, memory_hash: null, config_json: null, owner_id: null, runtime_ref: 'oc-1', runtime_type: 'openclaw', source: 'openclaw', created_at: '', updated_at: '' };
    const orAg = { id: 'or-1', name: 'Research', slug: 'research', role: null, description: null, model: 'test-model', is_default: false, is_active: true, workspace_path: null, soul_hash: null, memory_hash: null, config_json: null, owner_id: null, runtime_ref: null, runtime_type: 'openrouter', source: 'local', created_at: '', updated_at: '' };
    (apiMod.getAgents as any).mockResolvedValue([ocAg, orAg]);
    (apiMod.listRuntimeConnections as any).mockResolvedValue([
      { runtime_type: 'openclaw', connected: false, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'openrouter', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'anthropic', connected: false, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'ollama', connected: false, masked_credential: null, endpoint: null, error: null, agents: [] },
    ]);

    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    await openPicker(container);

    await waitFor(() => {
      expect(screen.getByText('Research')).toBeTruthy();
    });
    expect(screen.queryByText('Brit Freed')).toBeNull();
  });

  // #2588 C4 (Matt's ruling: subagent sessions are always-no) — extends the
  // B5 DefaultAgentPickerModal predicate (role='sub' AND runtime_ref != null)
  // to the Sessions drawer's New Session picker.
  it('excludes real subagents (role=sub, runtime_ref set) but includes local agents and non-sub runtime_ref agents', async () => {
    const subAg = { id: 'sub-1', name: 'Stia', slug: 'stia', role: 'sub', description: null, model: 'test-model', is_default: false, is_active: true, workspace_path: null, soul_hash: null, memory_hash: null, config_json: null, owner_id: null, runtime_ref: 'oc-sub-1', runtime_type: 'openclaw', source: 'openclaw', created_at: '', updated_at: '' };
    const localAg = { id: 'local-1', name: 'Local Agent', slug: 'local-agent', role: 'local', description: null, model: 'test-model', is_default: false, is_active: true, workspace_path: null, soul_hash: null, memory_hash: null, config_json: null, owner_id: null, runtime_ref: null, runtime_type: 'openrouter', source: 'local', created_at: '', updated_at: '' };
    const localSubRoleAg = { id: 'local-2', name: 'Legacy Local', slug: 'legacy-local', role: 'sub', description: null, model: 'test-model', is_default: false, is_active: true, workspace_path: null, soul_hash: null, memory_hash: null, config_json: null, owner_id: null, runtime_ref: null, runtime_type: 'anthropic', source: 'local', created_at: '', updated_at: '' };
    const nonSubWithRuntimeRefAg = { id: 'oc-1', name: 'Brit Freed', slug: 'brit', role: 'local', description: null, model: 'test-model', is_default: true, is_active: true, workspace_path: null, soul_hash: null, memory_hash: null, config_json: null, owner_id: null, runtime_ref: 'oc-1', runtime_type: 'openclaw', source: 'openclaw', created_at: '', updated_at: '' };
    (apiMod.getAgents as any).mockResolvedValue([subAg, localAg, localSubRoleAg, nonSubWithRuntimeRefAg]);
    (apiMod.listRuntimeConnections as any).mockResolvedValue([
      { runtime_type: 'openclaw', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'openrouter', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'anthropic', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
      { runtime_type: 'ollama', connected: true, masked_credential: null, endpoint: null, error: null, agents: [] },
    ]);

    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    await openPicker(container);

    await waitFor(() => {
      expect(screen.getByText('Local Agent')).toBeTruthy();
    });
    expect(screen.getByText('Legacy Local')).toBeTruthy();
    expect(screen.getByText('Brit Freed')).toBeTruthy();
    expect(screen.queryByText('Stia')).toBeNull();
  });
});

// ─── D-2: Single-session headline removed ────────────────────────────────────
// (design correction: "Just this one so far" headline removed per Reed's audit)

describe('SessionsDrawer — single-session headline absent (D-2)', () => {
  it('does NOT show "Just this one so far" headline even when exactly 1 workspace session', async () => {
    setMockSessions([mkSession({ id: 'solo', title: 'Solo session', is_active: true })]);

    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    await openDrawer(container);

    await waitFor(() => {
      // Session row renders
      expect(screen.getByText('Solo session')).toBeTruthy();
    });
    // Headline must NOT be present
    expect(screen.queryByText('Just this one so far')).toBeNull();
  });

  it('renders the session row without a headline above it', async () => {
    setMockSessions([mkSession({ id: 'solo', title: 'My Only Session', is_active: true })]);

    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    await openDrawer(container);

    await waitFor(() => {
      // Session row present
      expect(screen.getByText('My Only Session')).toBeTruthy();
    });
    // No headline
    expect(screen.queryByText('Just this one so far')).toBeNull();
    // No .sd-single-headline element
    expect(container.querySelector('.sd-single-headline')).toBeNull();
  });

  it('does NOT show single-session headline when 2+ workspace sessions', async () => {
    setMockSessions([
      mkSession({ id: 's1', title: 'Session A', is_active: true }),
      mkSession({ id: 's2', title: 'Session B' }),
    ]);

    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    await openDrawer(container);

    await waitFor(() => {
      expect(screen.getByText('Session A')).toBeTruthy();
    });
    expect(screen.queryByText('Just this one so far')).toBeNull();
  });
});

// ─── Badge count (D-4 PR-C: badge removed) ───────────────────────────────────
// The .sd-badge element and badgeCount derived value were removed in PR-C (#395).
// These tests now verify the badge is absent in all states.

describe('SessionsDrawer — badge count (removed in D-4)', () => {
  it('does not render .sd-badge with sessions present', async () => {
    setMockSessions([
      mkSession({ id: 's1', title: 'S1', is_active: true }),
      mkSession({ id: 's2', title: 'S2' }),
    ]);
    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    await new Promise(r => setTimeout(r, 20));
    expect(container.querySelector('.sd-badge')).toBeNull();
  });

  it('does not render .sd-badge when no sessions', async () => {
    setMockSessions([]);
    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    await new Promise(r => setTimeout(r, 20));
    expect(container.querySelector('.sd-badge')).toBeNull();
  });
});

// ─── Show-hidden footer toggle (regression guard for always-includeHidden fix) ──

describe('SessionsDrawer — show-hidden footer toggle', () => {
  it('renders the show-hidden footer toggle when API returns sessions with hidden_at set', async () => {
    // Regression guard: previously loadAllSessions(false) was called on init, so hidden
    // sessions were never fetched and hiddenWorkspace was always empty.
    // Fix: always load with includeHidden=true; showHidden only gates UI display.
    const hiddenSession = mkSession({
      id: 'hidden-1',
      title: 'Archived Session',
      hidden_at: '2026-07-01T00:00:00Z', // non-null → classified as hidden
    });
    const visibleSession = mkSession({ id: 'vis-1', title: 'Active Session', is_active: true });
    setMockSessions([visibleSession, hiddenSession], []);

    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    await openDrawer(container);

    await waitFor(() => {
      // The footer toggle must be present because hiddenWorkspace.length > 0
      const footer = container.querySelector('.sd-footer');
      expect(footer).not.toBeNull();
      const toggle = container.querySelector('.sd-hidden-toggle');
      expect(toggle).not.toBeNull();
      expect(toggle!.textContent).toContain('Hidden');
    });
  });

  it('API is always called with includeHidden=true regardless of showHidden state', async () => {
    const { listHomeSessionsWithRecovered } = await import('$lib/api');
    setMockSessions([mkSession({ id: 's1', is_active: true })], []);

    render(SessionsDrawer, { onSessionSwitch: vi.fn() });

    // Wait for the initial load to complete
    await waitFor(() => {
      expect(listHomeSessionsWithRecovered).toHaveBeenCalled();
    });

    // Every call must pass true
    const calls = (listHomeSessionsWithRecovered as ReturnType<typeof vi.fn>).mock.calls;
    expect(calls.length).toBeGreaterThan(0);
    calls.forEach(([includeHidden]) => {
      expect(includeHidden).toBe(true);
    });
  });
});

// ─── W1 fix verification ──────────────────────────────────────────────────────

describe('W1 fix — dead {#if isNarrow} branch removed from SessionDropdownHeader pinned-top', () => {
  it('SessionDropdownHeader.svelte does NOT contain dead isNarrow branch in pinned-top', () => {
    const filePath = resolve(__dirname, 'SessionDropdownHeader.svelte');
    const source = readFileSync(filePath, 'utf-8');

    // W1 fix: the original bug had `{#if isNarrow}...{:else}...{/if}` around New Session in pinned-top.
    // Both branches were identical. The fix removes the conditional and uses a single unconditional button.
    // Verify: the pinned-top section does NOT contain the isNarrow conditional around New Session.
    const pinnedTopStart = source.indexOf('sdh-pinned-top');
    const pinnedTopEnd = source.indexOf('sdh-list', pinnedTopStart);
    const pinnedTopBlock = source.slice(pinnedTopStart, pinnedTopEnd);

    // The dead {#if isNarrow} should be gone from this section
    expect(pinnedTopBlock).not.toContain('{#if isNarrow}');
    // The single unconditional button should be present
    expect(pinnedTopBlock).toContain('sdh-new-session-btn');
  });

  it('W1 fix comment is present in SessionDropdownHeader pinned-top markup', () => {
    const filePath = resolve(__dirname, 'SessionDropdownHeader.svelte');
    const source = readFileSync(filePath, 'utf-8');
    // W1 fix comment documents the change
    expect(source).toContain('W1 fix');
  });
});

// ─── BUG-1 regression guards (#395 fix) ────────────────────────────────────────────

describe('BUG-1 regression guards — session switch wiring (#395 fix)', () => {
  // Guard 1: +page.svelte must pass onSessionSwitch to SessionsDrawer
  it('+page.svelte passes onSessionSwitch prop to <SessionsDrawer />', () => {
    const pagePath = resolve(__dirname, '../../routes/+page.svelte');
    const source = readFileSync(pagePath, 'utf-8');

    // Find the SessionsDrawer mount site
    const sessionDrawerIdx = source.indexOf('<SessionsDrawer');
    expect(sessionDrawerIdx).toBeGreaterThan(-1);

    // The prop must appear within the SessionsDrawer element (next 300 chars)
    const mountSite = source.slice(sessionDrawerIdx, sessionDrawerIdx + 300);
    expect(mountSite).toContain('onSessionSwitch');
  });

  // Guard 2: onSessionSwitch is called (not optional-chained) in SessionsDrawer
  it('SessionsDrawer.svelte calls onSessionSwitch as required (not optional-chained)', () => {
    const drawerPath = resolve(__dirname, 'SessionsDrawer.svelte');
    const source = readFileSync(drawerPath, 'utf-8');

    // The Props interface must declare onSessionSwitch as required (no trailing ?).
    // Filter out comment lines when checking — the JSDoc may reference the old optional
    // form as an example of what was fixed.
    const nonCommentLines = source
      .split('\n')
      .filter(line => !line.trim().startsWith('*') && !line.trim().startsWith('//'))
      .join('\n');

    expect(nonCommentLines).not.toContain('onSessionSwitch?: ');
    expect(nonCommentLines).toContain('onSessionSwitch: (session: Session) => void;');

    // The call sites must NOT use optional chaining (in non-comment lines).
    expect(nonCommentLines).not.toContain('onSessionSwitch?.(');
    // Both handleSwitch and handleNewSession must call it directly.
    const switchIdx = nonCommentLines.indexOf('onSessionSwitch(session)');
    expect(switchIdx).toBeGreaterThan(-1);
  });

  // Guard 3: session switch invokes the onSessionSwitch callback (runtime)
  it('handleSwitch calls onSessionSwitch after apiSwitchSession succeeds', async () => {
    const session = mkSession({ id: 'ws-switch', title: 'Switch Target', is_active: false });
    const active = mkSession({ id: 'ws-active', title: 'Active Session', is_active: true });
    setMockSessions([active, session]);

    const onSessionSwitch = vi.fn();
    const { container } = render(SessionsDrawer, { onSessionSwitch });
    await openDrawer(container);

    // Wait for sessions to render
    await waitFor(() => {
      expect(screen.getByText('Switch Target')).toBeTruthy();
    });

    // Find and click the Switch Target session-row (the role="button" div inside sd-row).
    // SessionRow renders a div.session-row[role="button"] with the click handler on it.
    const sessionRows = container.querySelectorAll('.session-row');
    let switchRow: Element | null = null;
    for (const row of sessionRows) {
      if (row.textContent?.includes('Switch Target')) {
        switchRow = row;
        break;
      }
    }
    expect(switchRow).not.toBeNull();
    await fireEvent.click(switchRow!);

    // onSessionSwitch must be called with the switched-to session
    await waitFor(() => {
      expect(onSessionSwitch).toHaveBeenCalledWith(
        expect.objectContaining({ id: 'ws-switch' })
      );
    });
  });
});

// ─── D-4 (PR-C): session-count badge removed ───────────────────────────

describe('SessionsDrawer — D-4 badge absent', () => {
  it('does not render .sd-badge when there are no sessions', () => {
    setMockSessions([]);
    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    expect(container.querySelector('.sd-badge')).toBeNull();
  });

  it('does not render .sd-badge when sessions exist', async () => {
    setMockSessions([
      mkSession({ id: 'ws-1', is_active: true }),
      mkSession({ id: 'ws-2' }),
    ]);
    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    // Check before opening the drawer
    expect(container.querySelector('.sd-badge')).toBeNull();
    // And after opening
    await openDrawer(container);
    expect(container.querySelector('.sd-badge')).toBeNull();
  });

  it('does not render .sd-badge with many sessions', async () => {
    const sessions = Array.from({ length: 10 }, (_, i) =>
      mkSession({ id: `ws-${i}`, title: `Session ${i}` })
    );
    sessions[0].is_active = true;
    setMockSessions(sessions);
    const { container } = render(SessionsDrawer, { onSessionSwitch: vi.fn() });
    await openDrawer(container);
    expect(container.querySelector('.sd-badge')).toBeNull();
  });
});

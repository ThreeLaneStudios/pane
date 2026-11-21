/**
 * ProjectSettings.test.ts — Component tests for ProjectSettings.svelte
 * (#2563 B2 fix — New Project agent picker).
 *
 * CI-testable scope:
 *   ✓ New Project agent picker (create mode) uses `filterConnectedAgents`
 *     over `listRuntimeConnections()`, not the retired
 *     `ai_settings.provider` global scalar — same fix as SessionDropdownHeader
 *     and SessionsDrawer's New Session pickers.
 *
 * NOT tested here (live DMG tests — Matt):
 *   - Edit-mode project settings load (pinned notes, budget, memory)
 *   - Category picker / New Category flow
 *   - Save flow round-trip against Tauri backend
 */

import { describe, it, expect, vi, beforeAll, afterEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/svelte/svelte5';
import ProjectSettings from './ProjectSettings.svelte';

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: invokeMock,
}));

vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(),
  // #2972 stage 4: ProjectSettings.svelte now also calls
  // subscribeDataChanged() on mount (category-list refresh listener,
  // §34 inventory), which calls listen() internally — a bare `emit`-only
  // mock left `listen` undefined, breaking every test in this file that
  // mounts the component. Not exercised by any test here (none dispatch a
  // data-changed event) — just needs to resolve so mount doesn't throw.
  listen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock('$lib/api', async (importOriginal) => {
  const original = await importOriginal() as any;
  return {
    ...original,
    getAgents: vi.fn(),
    listRuntimeConnections: vi.fn(),
    listProjects: vi.fn().mockResolvedValue([]),
  };
});

import { getAgents, listRuntimeConnections } from '$lib/api';

beforeAll(() => {
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

function mkAgent(overrides: Partial<any> = {}) {
  return {
    id: 'a1',
    name: 'Test Agent',
    slug: 'test-agent',
    role: 'sub',
    description: null,
    model: 'claude-sonnet-4-5',
    is_default: false,
    is_active: true,
    workspace_path: '/tmp/a1',
    soul_hash: null,
    memory_hash: null,
    config_json: '{}',
    owner_id: null,
    runtime_ref: null,
    runtime_type: 'openrouter',
    source: 'local',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    ...overrides,
  };
}

function mkConn(overrides: Partial<any> = {}) {
  return {
    runtime_type: 'openrouter',
    connected: true,
    masked_credential: null,
    endpoint: null,
    error: null,
    agents: [],
    ...overrides,
  };
}

beforeAll(() => {
  invokeMock.mockImplementation(async (cmd: string) => {
    if (cmd === 'list_categories') return [];
    return null;
  });
});

afterEach(() => {
  vi.clearAllMocks();
  invokeMock.mockImplementation(async (cmd: string) => {
    if (cmd === 'list_categories') return [];
    return null;
  });
});

describe('ProjectSettings — New Project agent picker (create mode)', () => {
  it('mixed scenario: OC agent + OpenRouter agent, both runtimes connected → both shown', async () => {
    // role: 'main' — a non-sub OC-imported agent (#2588: default fixture role='sub'
    // would now be misread as a real subagent and wrongly excluded).
    const ocAg = mkAgent({ id: 'oc-1', name: 'Brit Freed', role: 'main', runtime_ref: 'oc-1', runtime_type: 'openclaw', is_default: true });
    const orAg = mkAgent({ id: 'or-1', name: 'Research', runtime_ref: null, runtime_type: 'openrouter' });
    (getAgents as any).mockResolvedValue([ocAg, orAg]);
    (listRuntimeConnections as any).mockResolvedValue([
      mkConn({ runtime_type: 'openclaw', connected: true }),
      mkConn({ runtime_type: 'openrouter', connected: true }),
      mkConn({ runtime_type: 'anthropic', connected: false }),
      mkConn({ runtime_type: 'ollama', connected: false }),
    ]);

    render(ProjectSettings, {});

    await waitFor(() => {
      expect(screen.getByText('Brit Freed')).toBeTruthy();
      expect(screen.getByText('Research')).toBeTruthy();
    });
  });

  it('excludes agents on a disconnected runtime while including the other runtime\'s agent', async () => {
    const ocAg = mkAgent({ id: 'oc-1', name: 'Brit Freed', runtime_ref: 'oc-1', runtime_type: 'openclaw', is_default: true });
    const orAg = mkAgent({ id: 'or-1', name: 'Research', runtime_ref: null, runtime_type: 'openrouter' });
    (getAgents as any).mockResolvedValue([ocAg, orAg]);
    (listRuntimeConnections as any).mockResolvedValue([
      mkConn({ runtime_type: 'openclaw', connected: false }),
      mkConn({ runtime_type: 'openrouter', connected: true }),
      mkConn({ runtime_type: 'anthropic', connected: false }),
      mkConn({ runtime_type: 'ollama', connected: false }),
    ]);

    render(ProjectSettings, {});

    await waitFor(() => {
      expect(screen.getByText('Research')).toBeTruthy();
    });
    expect(screen.queryByText('Brit Freed')).toBeNull();
  });

  it('excludes an inactive agent from the picker even on a connected runtime', async () => {
    const inactiveAg = mkAgent({ id: 'inactive-1', name: 'Retired Agent', is_active: false, runtime_type: 'openrouter' });
    const activeAg = mkAgent({ id: 'active-1', name: 'Active Agent', is_active: true, is_default: true, runtime_type: 'openrouter' });
    (getAgents as any).mockResolvedValue([inactiveAg, activeAg]);
    (listRuntimeConnections as any).mockResolvedValue([
      mkConn({ runtime_type: 'openclaw', connected: true }),
      mkConn({ runtime_type: 'openrouter', connected: true }),
      mkConn({ runtime_type: 'anthropic', connected: true }),
      mkConn({ runtime_type: 'ollama', connected: true }),
    ]);

    render(ProjectSettings, {});

    await waitFor(() => {
      expect(screen.getByText('Active Agent')).toBeTruthy();
    });
    expect(screen.queryByText('Retired Agent')).toBeNull();
  });

  it('no connected runtimes → shows the empty-picker hint, not an error', async () => {
    (getAgents as any).mockResolvedValue([mkAgent()]);
    (listRuntimeConnections as any).mockResolvedValue([
      mkConn({ runtime_type: 'openclaw', connected: false }),
      mkConn({ runtime_type: 'openrouter', connected: false }),
      mkConn({ runtime_type: 'anthropic', connected: false }),
      mkConn({ runtime_type: 'ollama', connected: false }),
    ]);

    render(ProjectSettings, {});

    await waitFor(() => {
      expect(screen.getByText('No agents available yet — set one up in Settings first.')).toBeTruthy();
    });
  });

  // #2588 C4 (Matt's ruling: subagent sessions are always-no) — extends the
  // B5 DefaultAgentPickerModal predicate (role='sub' AND runtime_ref != null)
  // to the New Project agent picker.
  it('excludes real subagents (role=sub, runtime_ref set) but includes local agents and non-sub runtime_ref agents', async () => {
    const subAg = mkAgent({ id: 'sub-1', name: 'Stia', role: 'sub', runtime_ref: 'oc-sub-1', runtime_type: 'openclaw' });
    const localAg = mkAgent({ id: 'local-1', name: 'Local Agent', role: 'local', runtime_ref: null, runtime_type: 'openrouter' });
    const localSubRoleAg = mkAgent({ id: 'local-2', name: 'Legacy Local', role: 'sub', runtime_ref: null, runtime_type: 'anthropic' });
    const nonSubWithRuntimeRefAg = mkAgent({ id: 'oc-1', name: 'Brit Freed', role: 'local', runtime_ref: 'oc-1', runtime_type: 'openclaw', is_default: true });
    (getAgents as any).mockResolvedValue([subAg, localAg, localSubRoleAg, nonSubWithRuntimeRefAg]);
    (listRuntimeConnections as any).mockResolvedValue([
      mkConn({ runtime_type: 'openclaw', connected: true }),
      mkConn({ runtime_type: 'openrouter', connected: true }),
      mkConn({ runtime_type: 'anthropic', connected: true }),
      mkConn({ runtime_type: 'ollama', connected: true }),
    ]);

    render(ProjectSettings, {});

    await waitFor(() => {
      expect(screen.getByText('Local Agent')).toBeTruthy();
    });
    expect(screen.getByText('Legacy Local')).toBeTruthy();
    expect(screen.getByText('Brit Freed')).toBeTruthy();
    expect(screen.queryByText('Stia')).toBeNull();
  });
});

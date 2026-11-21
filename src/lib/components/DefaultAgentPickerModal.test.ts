/**
 * DefaultAgentPickerModal.test.ts — Component tests for DefaultAgentPickerModal.svelte
 *
 * #2522 Stage 5d, ADR-0063 D8: cross-provider "Default agent" picker. Replaces the
 * per-agent "Set as Main" buttons that used to live inline in the Agents-tab roster.
 *
 * Test groups:
 *  A — Renders when mounted, shows title/subtitle
 *  B — Groups agents by runtime_type and shows agents from every provider present
 *  C — Disconnected-provider agents are shown dimmed and are not selectable
 *  D — Confirm calls setMainAgent (set_main_agent) with the selected agent id
 *  E — Modal closes (onConfirm fires) after a successful selection
 *  F — Cancel / Escape close without calling setMainAgent
 *  G — Error handling: failed setMainAgent surfaces an error and does not close
 *  H — #2567: subagents (role='sub') are excluded entirely, not shown as disabled rows
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import DefaultAgentPickerModal from './DefaultAgentPickerModal.svelte';
import type { Agent } from '$lib/types';

vi.mock('$lib/api', async (importOriginal) => {
  const original = await importOriginal() as any;
  return {
    ...original,
    setMainAgent: vi.fn(),
  };
});

import { setMainAgent } from '$lib/api';

// vitest-setup.ts replaces `globalThis.window` with a plain mock object (Tauri
// internals shim) that lacks addEventListener/removeEventListener. This modal uses
// <svelte:window onkeydown={...}> for Escape-to-close, so the DOM listener API needs
// to exist on that mock window for mount/unmount to succeed. Patch it here rather than
// touching the shared setup file (other suites depend on its exact shape).
if (typeof (globalThis.window as any).addEventListener !== 'function') {
  (globalThis.window as any).addEventListener = () => {};
}
if (typeof (globalThis.window as any).removeEventListener !== 'function') {
  (globalThis.window as any).removeEventListener = () => {};
}

function makeAgent(overrides: Partial<Agent> = {}): Agent {
  return {
    id: 'a1',
    name: 'Research',
    slug: 'research',
    role: 'main',
    model: 'claude-3-opus',
    is_default: false,
    is_active: true,
    workspace_path: '/tmp/research',
    config_json: '{}',
    runtime_ref: null,
    runtime_type: 'anthropic',
    source: 'local',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    ...overrides,
  };
}

beforeEach(() => {
  (setMainAgent as any).mockReset?.();
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.resetAllMocks();
});

// ---------------------------------------------------------------------------
// A — Renders
// ---------------------------------------------------------------------------

describe('A — renders when mounted', () => {
  it('renders the modal title and subtitle', () => {
    const { container } = render(DefaultAgentPickerModal, {
      props: {
        agents: [makeAgent()],
        connectedRuntimes: new Set(['anthropic']),
        onConfirm: vi.fn(),
        onCancel: vi.fn(),
      },
    });
    expect(container.textContent).toContain('Default agent');
    expect(container.textContent).toContain('Preselected for new sessions');
  });

  it('renders as a dialog with aria-modal', () => {
    const { container } = render(DefaultAgentPickerModal, {
      props: {
        agents: [makeAgent()],
        connectedRuntimes: new Set(['anthropic']),
        onConfirm: vi.fn(),
        onCancel: vi.fn(),
      },
    });
    const dialog = container.querySelector('[role="dialog"]');
    expect(dialog).not.toBeNull();
    expect(dialog?.getAttribute('aria-modal')).toBe('true');
  });
});

// ---------------------------------------------------------------------------
// B — Cross-provider grouping
// ---------------------------------------------------------------------------

describe('B — shows agents from all providers', () => {
  it('lists agents from every provider present, each under its own group label', () => {
    const agents = [
      makeAgent({ id: 'a1', name: 'Research', runtime_type: 'anthropic' }),
      makeAgent({ id: 'a2', name: 'Router Agent', runtime_type: 'openrouter' }),
      makeAgent({ id: 'a3', name: 'Local Llama', runtime_type: 'ollama' }),
      makeAgent({ id: 'a4', name: 'OC Main', runtime_type: 'openclaw' }),
    ];
    const { container } = render(DefaultAgentPickerModal, {
      props: {
        agents,
        connectedRuntimes: new Set(['anthropic', 'openrouter', 'ollama', 'openclaw']),
        onConfirm: vi.fn(),
        onCancel: vi.fn(),
      },
    });
    expect(container.textContent).toContain('Research');
    expect(container.textContent).toContain('Router Agent');
    expect(container.textContent).toContain('Local Llama');
    expect(container.textContent).toContain('OC Main');
    expect(container.textContent).toContain('Anthropic');
    expect(container.textContent).toContain('OpenRouter');
    expect(container.textContent).toContain('Ollama');
    expect(container.textContent).toContain('OpenClaw');
  });

  it('excludes agents with a null runtime_type (legacy-unpinned, D4.3)', () => {
    const agents = [
      makeAgent({ id: 'a1', name: 'Research', runtime_type: 'anthropic' }),
      makeAgent({ id: 'a2', name: 'Unpinned', runtime_type: null }),
    ];
    const { container } = render(DefaultAgentPickerModal, {
      props: {
        agents,
        connectedRuntimes: new Set(['anthropic']),
        onConfirm: vi.fn(),
        onCancel: vi.fn(),
      },
    });
    expect(container.textContent).toContain('Research');
    expect(container.textContent).not.toContain('Unpinned');
  });
});

// ---------------------------------------------------------------------------
// C — Disconnected providers dimmed + not selectable
// ---------------------------------------------------------------------------

describe('C — disconnected-provider agents are shown but not selectable', () => {
  it('renders a disabled pick button with a "Provider disconnected" reason for a disconnected runtime', () => {
    const agents = [makeAgent({ id: 'a1', name: 'Offline Agent', runtime_type: 'ollama' })];
    const { container } = render(DefaultAgentPickerModal, {
      props: {
        agents,
        connectedRuntimes: new Set(), // ollama not connected
        onConfirm: vi.fn(),
        onCancel: vi.fn(),
      },
    });
    const pickButtons = container.querySelectorAll('.dap-pick');
    expect(pickButtons.length).toBe(1);
    expect((pickButtons[0] as HTMLButtonElement).disabled).toBe(true);
    expect(container.textContent).toContain('Provider disconnected');
  });

  it('clicking a disabled (disconnected-provider) agent does not select it', async () => {
    const agents = [
      makeAgent({ id: 'a1', name: 'Online Agent', runtime_type: 'anthropic' }),
      makeAgent({ id: 'a2', name: 'Offline Agent', runtime_type: 'ollama' }),
    ];
    const { container } = render(DefaultAgentPickerModal, {
      props: {
        agents,
        connectedRuntimes: new Set(['anthropic']),
        onConfirm: vi.fn(),
        onCancel: vi.fn(),
      },
    });
    const offlineBtn = Array.from(container.querySelectorAll('.dap-pick')).find(
      (b) => b.textContent?.includes('Offline Agent')
    ) as HTMLButtonElement;
    await fireEvent.click(offlineBtn);
    await tick();
    expect(offlineBtn.classList.contains('dap-pick--sel')).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// D — Confirm calls the backend command
// ---------------------------------------------------------------------------

describe('D — setting a default calls the right backend command', () => {
  it('selecting an agent and clicking Set default calls setMainAgent with that agent id', async () => {
    (setMainAgent as any).mockResolvedValue(makeAgent({ id: 'a2', name: 'Router Agent', is_default: true }));
    const onConfirm = vi.fn();
    const agents = [
      makeAgent({ id: 'a1', name: 'Research', runtime_type: 'anthropic', is_default: true }),
      makeAgent({ id: 'a2', name: 'Router Agent', runtime_type: 'openrouter', is_default: false }),
    ];
    const { container } = render(DefaultAgentPickerModal, {
      props: {
        agents,
        connectedRuntimes: new Set(['anthropic', 'openrouter']),
        onConfirm,
        onCancel: vi.fn(),
      },
    });

    const routerBtn = Array.from(container.querySelectorAll('.dap-pick')).find(
      (b) => b.textContent?.includes('Router Agent')
    ) as HTMLButtonElement;
    await fireEvent.click(routerBtn);
    await tick();

    const setDefaultBtn = screen.getByText('Set default') as HTMLButtonElement;
    await fireEvent.click(setDefaultBtn);
    await tick();
    await tick();

    expect(setMainAgent).toHaveBeenCalledWith('a2');
  });
});

// ---------------------------------------------------------------------------
// E — Modal closes after selection
// ---------------------------------------------------------------------------

describe('E — modal closes after selection', () => {
  it('calls onConfirm with the updated agent once setMainAgent resolves', async () => {
    const updatedAgent = makeAgent({ id: 'a2', name: 'Router Agent', is_default: true });
    (setMainAgent as any).mockResolvedValue(updatedAgent);
    const onConfirm = vi.fn();
    const agents = [
      makeAgent({ id: 'a1', name: 'Research', runtime_type: 'anthropic', is_default: true }),
      makeAgent({ id: 'a2', name: 'Router Agent', runtime_type: 'openrouter', is_default: false }),
    ];
    const { container } = render(DefaultAgentPickerModal, {
      props: {
        agents,
        connectedRuntimes: new Set(['anthropic', 'openrouter']),
        onConfirm,
        onCancel: vi.fn(),
      },
    });

    const routerBtn = Array.from(container.querySelectorAll('.dap-pick')).find(
      (b) => b.textContent?.includes('Router Agent')
    ) as HTMLButtonElement;
    await fireEvent.click(routerBtn);
    await tick();
    await fireEvent.click(screen.getByText('Set default'));
    await tick();
    await tick();

    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(onConfirm).toHaveBeenCalledWith(updatedAgent);
  });

  it('dispatches pane-agent-switched on successful selection', async () => {
    (setMainAgent as any).mockResolvedValue(makeAgent({ id: 'a2', is_default: true }));
    const dispatchSpy = vi.spyOn(window, 'dispatchEvent');
    const agents = [
      makeAgent({ id: 'a1', name: 'Research', runtime_type: 'anthropic', is_default: true }),
      makeAgent({ id: 'a2', name: 'Router Agent', runtime_type: 'openrouter', is_default: false }),
    ];
    const { container } = render(DefaultAgentPickerModal, {
      props: {
        agents,
        connectedRuntimes: new Set(['anthropic', 'openrouter']),
        onConfirm: vi.fn(),
        onCancel: vi.fn(),
      },
    });
    const routerBtn = Array.from(container.querySelectorAll('.dap-pick')).find(
      (b) => b.textContent?.includes('Router Agent')
    ) as HTMLButtonElement;
    await fireEvent.click(routerBtn);
    await tick();
    await fireEvent.click(screen.getByText('Set default'));
    await tick();
    await tick();

    expect(dispatchSpy).toHaveBeenCalled();
    const evt = dispatchSpy.mock.calls.find((c: any[]) => c[0]?.type === 'pane-agent-switched');
    expect(evt).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------
// F — Cancel / Escape
// ---------------------------------------------------------------------------

describe('F — cancel and escape close without setting a default', () => {
  it('clicking Cancel calls onCancel and does not call setMainAgent', async () => {
    const onCancel = vi.fn();
    const { } = render(DefaultAgentPickerModal, {
      props: {
        agents: [makeAgent()],
        connectedRuntimes: new Set(['anthropic']),
        onConfirm: vi.fn(),
        onCancel,
      },
    });
    await fireEvent.click(screen.getByText('Cancel'));
    await tick();

    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(setMainAgent).not.toHaveBeenCalled();
  });

  it('confirming without changing the selection (same as current default) calls onCancel, not setMainAgent', async () => {
    const onCancel = vi.fn();
    const onConfirm = vi.fn();
    const agents = [makeAgent({ id: 'a1', name: 'Research', runtime_type: 'anthropic', is_default: true })];
    render(DefaultAgentPickerModal, {
      props: {
        agents,
        connectedRuntimes: new Set(['anthropic']),
        onConfirm,
        onCancel,
      },
    });
    await fireEvent.click(screen.getByText('Set default'));
    await tick();

    expect(setMainAgent).not.toHaveBeenCalled();
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onConfirm).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// H — #2567: subagents excluded from the eligible list
// ---------------------------------------------------------------------------

describe('H — subagents (role=sub) are excluded from the picker', () => {
  it('does not render sub-role agents as options in a mixed roster', () => {
    const agents = [
      makeAgent({ id: 'main-1', name: 'Brit Freed', role: 'main', runtime_type: 'openclaw', is_default: true }),
      makeAgent({ id: 'local-1', name: 'Local Coder', role: 'main', runtime_type: 'openrouter' }),
      makeAgent({ id: 'sub-1', name: 'Lane', role: 'sub', runtime_type: 'openclaw', runtime_ref: 'oc-sub-1' }),
      makeAgent({ id: 'sub-2', name: 'Reed', role: 'sub', runtime_type: 'openclaw', runtime_ref: 'oc-sub-2' }),
      makeAgent({ id: 'sub-3', name: 'Stia', role: 'sub', runtime_type: 'openclaw', runtime_ref: 'oc-sub-3' }),
    ];
    const { container } = render(DefaultAgentPickerModal, {
      props: {
        agents,
        connectedRuntimes: new Set(['openclaw', 'openrouter']),
        onConfirm: vi.fn(),
        onCancel: vi.fn(),
      },
    });

    expect(container.textContent).toContain('Brit Freed');
    expect(container.textContent).toContain('Local Coder');
    expect(container.textContent).not.toContain('Lane');
    expect(container.textContent).not.toContain('Reed');
    expect(container.textContent).not.toContain('Stia');

    // Not just hidden text — no pick buttons at all for the sub-role agents.
    const pickButtons = container.querySelectorAll('.dap-pick');
    expect(pickButtons.length).toBe(2);
  });

  it('omits the provider group label entirely when every agent under it is sub-role', () => {
    const agents = [
      makeAgent({ id: 'main-1', name: 'Brit Freed', role: 'main', runtime_type: 'openrouter', is_default: true }),
      makeAgent({ id: 'sub-1', name: 'Pane Support', role: 'sub', runtime_type: 'openclaw', runtime_ref: 'oc-sub-1' }),
    ];
    const { container } = render(DefaultAgentPickerModal, {
      props: {
        agents,
        connectedRuntimes: new Set(['openrouter', 'openclaw']),
        onConfirm: vi.fn(),
        onCancel: vi.fn(),
      },
    });

    expect(container.textContent).toContain('OpenRouter');
    expect(container.textContent).not.toContain('OpenClaw');
    expect(container.textContent).not.toContain('Pane Support');
  });

  it('a sub-role agent is never selectable even if it were somehow marked is_default', async () => {
    const agents = [
      makeAgent({ id: 'sub-1', name: 'Lane', role: 'sub', runtime_type: 'openclaw', is_default: true, runtime_ref: 'oc-sub-1' }),
      makeAgent({ id: 'main-1', name: 'Brit Freed', role: 'main', runtime_type: 'openrouter', is_default: false }),
    ];
    const { container } = render(DefaultAgentPickerModal, {
      props: {
        agents,
        connectedRuntimes: new Set(['openclaw', 'openrouter']),
        onConfirm: vi.fn(),
        onCancel: vi.fn(),
      },
    });

    expect(container.textContent).not.toContain('Lane');
    const pickButtons = container.querySelectorAll('.dap-pick');
    expect(pickButtons.length).toBe(1);
    expect(pickButtons[0].textContent).toContain('Brit Freed');
  });

  it('#B5: a locally-created agent with role=sub but runtime_ref=null still appears as a pickable option', () => {
    // Reed's PR #2577 audit: role='sub' alone is too broad a predicate — locally-created
    // agents can share role='sub' but have runtime_ref = null (never imported from OC,
    // so not a real subagent). Such an agent must remain eligible.
    const agents = [
      makeAgent({ id: 'main-1', name: 'Brit Freed', role: 'main', runtime_type: 'openrouter', is_default: true }),
      makeAgent({ id: 'local-sub-1', name: 'Locally Created', role: 'sub', runtime_type: 'openrouter', runtime_ref: null }),
    ];
    const { container } = render(DefaultAgentPickerModal, {
      props: {
        agents,
        connectedRuntimes: new Set(['openrouter']),
        onConfirm: vi.fn(),
        onCancel: vi.fn(),
      },
    });

    expect(container.textContent).toContain('Locally Created');
    const pickButtons = container.querySelectorAll('.dap-pick');
    expect(pickButtons.length).toBe(2);
  });

  it("#2567 B6: a locally-created agent with role='local', runtime_ref=null appears as a pickable option", () => {
    // #2567 B6: role='local' is the new birth label for locally-created agents (was
    // role='sub' + runtime_ref=null workaround, see the #B5 test above). The B5
    // predicate (`a.role !== 'sub' || a.runtime_ref == null`) never rejects
    // role='local' agents in the first place, so this proves the modal handles the
    // new literal correctly without relying on the runtime_ref workaround at all.
    const agents = [
      makeAgent({ id: 'main-1', name: 'Brit Freed', role: 'main', runtime_type: 'openrouter', is_default: true }),
      makeAgent({ id: 'local-1', name: 'Locally Created', role: 'local', runtime_type: 'openrouter', runtime_ref: null }),
    ];
    const { container } = render(DefaultAgentPickerModal, {
      props: {
        agents,
        connectedRuntimes: new Set(['openrouter']),
        onConfirm: vi.fn(),
        onCancel: vi.fn(),
      },
    });

    expect(container.textContent).toContain('Locally Created');
    const pickButtons = container.querySelectorAll('.dap-pick');
    expect(pickButtons.length).toBe(2);
  });
});

// ---------------------------------------------------------------------------
// G — Error handling
// ---------------------------------------------------------------------------

describe('G — error handling', () => {
  it('shows an error message and does not close when setMainAgent rejects', async () => {
    (setMainAgent as any).mockRejectedValue(new Error('agent not found'));
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    const agents = [
      makeAgent({ id: 'a1', name: 'Research', runtime_type: 'anthropic', is_default: true }),
      makeAgent({ id: 'a2', name: 'Router Agent', runtime_type: 'openrouter', is_default: false }),
    ];
    const { container } = render(DefaultAgentPickerModal, {
      props: {
        agents,
        connectedRuntimes: new Set(['anthropic', 'openrouter']),
        onConfirm,
        onCancel,
      },
    });
    const routerBtn = Array.from(container.querySelectorAll('.dap-pick')).find(
      (b) => b.textContent?.includes('Router Agent')
    ) as HTMLButtonElement;
    await fireEvent.click(routerBtn);
    await tick();
    await fireEvent.click(screen.getByText('Set default'));
    await tick();
    await tick();

    expect(container.querySelector('.dap-modal__error')?.textContent).toContain('agent not found');
    expect(onConfirm).not.toHaveBeenCalled();
    expect(onCancel).not.toHaveBeenCalled();
  });
});

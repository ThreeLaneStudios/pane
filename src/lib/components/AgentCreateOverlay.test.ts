/**
 * AgentCreateOverlay.test.ts — Component tests for AgentCreateOverlay.svelte
 *
 * #2522 Stage 6b, ADR-0063 D8/D1: real runtime + model picker replacing Stage 6a's
 * temporary auto-resolve seam (`resolveRuntimeTypeForCreate` used to call
 * `listRuntimeConnections()` and silently pick the first connected non-openclaw
 * runtime). This suite covers the voluntary create path only (Settings → "+ New
 * agent" / AgentPickerModal "Create new" — forcedCreate=false), which is the entry
 * point Stage 6b's picker applies to.
 *
 * Design ruling under test (README-group-b.md §8, "The model these surfaces
 * express" #2): "Every agent permanently belongs to one provider. No
 * change-provider control anywhere, by design." — the runtime picker is the ONE
 * place that choice gets made, and it only ever lists CONNECTED runtimes
 * (openclaw excluded, per module comment / Stage 6a's original exclusion).
 *
 * Test groups:
 *  A — Runtime picker: shows connected runtimes only, excludes openclaw + disconnected
 *  B — Model picker: appears after a runtime is selected, models filtered per-runtime
 *  C — Creation: create_local_agent called with the picked runtime_type (+ model)
 *  D — Forced-create path unaffected: skips runtime/model picker entirely
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import AgentCreateOverlay from './AgentCreateOverlay.svelte';
import type { RuntimeConnectionInfo } from '$lib/api';
import type { Agent } from '$lib/types';

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: invokeMock,
}));

vi.mock('$lib/api', async (importOriginal) => {
  const original = await importOriginal() as any;
  return {
    ...original,
    listRuntimeConnections: vi.fn(),
    createLocalAgent: vi.fn(),
    checkNameCollision: vi.fn().mockResolvedValue(false),
  };
});

import { listRuntimeConnections, createLocalAgent, checkNameCollision } from '$lib/api';

// vitest-setup.ts replaces globalThis.window with a plain mock object lacking
// addEventListener/removeEventListener. AgentCreateOverlay uses <svelte:window
// onkeydown={...}> for Escape-to-close, so patch those in (same pattern
// DefaultAgentPickerModal.test.ts uses for the same reason).
if (typeof (globalThis.window as any).addEventListener !== 'function') {
  (globalThis.window as any).addEventListener = () => {};
}
if (typeof (globalThis.window as any).removeEventListener !== 'function') {
  (globalThis.window as any).removeEventListener = () => {};
}
if (typeof (globalThis.window as any).dispatchEvent !== 'function') {
  (globalThis.window as any).dispatchEvent = () => true;
}

function makeConn(overrides: Partial<RuntimeConnectionInfo> = {}): RuntimeConnectionInfo {
  return {
    runtime_type: 'anthropic',
    connected: true,
    masked_credential: 'sk-ant-...abcd',
    endpoint: null,
    error: null,
    agents: [],
    ...overrides,
  };
}

/**
 * #2590 item 4: open SearchableModelSelect's dropdown and click the item whose
 * visible text (model name or id) matches `label`. Mirrors real usage — the
 * component only renders `.dropdown-item` buttons while focused/open.
 */
async function pickSearchableModel(container: HTMLElement, label: string) {
  const input = container.querySelector('.model-input') as HTMLInputElement;
  await fireEvent.focus(input);
  await tick();
  const items = Array.from(container.querySelectorAll('.dropdown-item'));
  const item = items.find((el) => el.textContent?.includes(label)) as HTMLElement;
  await fireEvent.mouseDown(item);
  await tick();
}

function makeAgent(overrides: Partial<Agent> = {}): Agent {
  return {
    id: 'agent-1',
    name: 'Research',
    slug: 'research',
    role: 'sub',
    model: null,
    is_default: true,
    is_active: true,
    workspace_path: 'ABC123',
    config_json: '{}',
    runtime_ref: null,
    runtime_type: 'anthropic',
    source: 'ai_created',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    ...overrides,
  };
}

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue([]);
  (listRuntimeConnections as any).mockReset();
  (createLocalAgent as any).mockReset();
  (checkNameCollision as any).mockReset().mockResolvedValue(false);
});

afterEach(() => {
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// A — Runtime picker: connected only
// ---------------------------------------------------------------------------

describe('A — runtime picker shows connected runtimes only', () => {
  it('lists connected non-openclaw runtimes, excludes disconnected ones', async () => {
    (listRuntimeConnections as any).mockResolvedValue([
      makeConn({ runtime_type: 'anthropic', connected: true }),
      makeConn({ runtime_type: 'openrouter', connected: false }),
      makeConn({ runtime_type: 'ollama', connected: true, masked_credential: null, endpoint: 'http://localhost:11434' }),
    ]);

    const { container } = render(AgentCreateOverlay, {
      props: { onClose: vi.fn() },
    });
    await tick();
    await tick();

    expect(container.textContent).toContain('Anthropic');
    expect(container.textContent).toContain('Ollama');
    expect(container.textContent).not.toContain('OpenRouter');
    expect(container.querySelectorAll('[data-runtime]').length).toBe(2);
  });

  it('excludes openclaw even when connected — it has no local-agent creation UI of its own', async () => {
    (listRuntimeConnections as any).mockResolvedValue([
      makeConn({ runtime_type: 'openclaw', connected: true, endpoint: 'ws://localhost:9000' }),
      makeConn({ runtime_type: 'anthropic', connected: true }),
    ]);

    const { container } = render(AgentCreateOverlay, {
      props: { onClose: vi.fn() },
    });
    await tick();
    await tick();

    expect(container.querySelector('[data-runtime="openclaw"]')).toBeNull();
    expect(container.querySelector('[data-runtime="anthropic"]')).not.toBeNull();
  });

  it('shows an error message when no providers are connected', async () => {
    (listRuntimeConnections as any).mockResolvedValue([
      makeConn({ runtime_type: 'anthropic', connected: false }),
      makeConn({ runtime_type: 'openrouter', connected: false }),
    ]);

    const { container } = render(AgentCreateOverlay, {
      props: { onClose: vi.fn() },
    });
    await tick();
    await tick();

    expect(container.textContent).toContain('No connected providers found');
  });

  it('surfaces an error and does not crash when listRuntimeConnections rejects', async () => {
    (listRuntimeConnections as any).mockRejectedValue(new Error('gateway unreachable'));

    const { container } = render(AgentCreateOverlay, {
      props: { onClose: vi.fn() },
    });
    await tick();
    await tick();

    expect(container.textContent).toContain('gateway unreachable');
  });
});

// ---------------------------------------------------------------------------
// B — Model picker: appears after runtime selection
// ---------------------------------------------------------------------------

describe('B — model picker appears after picking a runtime', () => {
  beforeEach(() => {
    (listRuntimeConnections as any).mockResolvedValue([
      makeConn({ runtime_type: 'anthropic', connected: true }),
      makeConn({ runtime_type: 'openrouter', connected: true }),
    ]);
  });

  it('clicking a runtime card advances to the model step and fetches models', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'fetch_available_models') {
        return [
          { id: 'claude-opus-4-5', name: 'Claude Opus 4.5', provider: 'anthropic' },
          { id: 'claude-sonnet-4-5', name: 'Claude Sonnet 4.5', provider: 'anthropic' },
          { id: 'gpt-4o', name: 'GPT-4o', provider: 'openrouter' },
        ];
      }
      return null;
    });

    const { container } = render(AgentCreateOverlay, {
      props: { onClose: vi.fn() },
    });
    await tick();
    await tick();

    const anthropicCard = container.querySelector('[data-runtime="anthropic"]') as HTMLElement;
    expect(anthropicCard).not.toBeNull();
    await fireEvent.click(anthropicCard);
    await tick();
    await tick();

    expect(container.textContent).toContain('Choose a model');
    // Only anthropic's models should be listed (filtered from the merged response) —
    // #2590 item 4: rendered via SearchableModelSelect's dropdown, not [data-model] divs.
    const picker = container.querySelector('[data-testid="agent-create-model-picker"]') as HTMLElement;
    expect(picker).not.toBeNull();
    const input = picker.querySelector('.model-input') as HTMLInputElement;
    await fireEvent.focus(input);
    await tick();
    const itemLabels = Array.from(picker.querySelectorAll('.dropdown-item')).map((el) => el.textContent);
    expect(itemLabels.some((t) => t?.includes('Claude Opus 4.5'))).toBe(true);
    expect(itemLabels.some((t) => t?.includes('Claude Sonnet 4.5'))).toBe(true);
    expect(itemLabels.some((t) => t?.includes('GPT-4o'))).toBe(false);
  });

  it('clicking a model advances to the name step', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'fetch_available_models') {
        return [{ id: 'claude-opus-4-5', name: 'Claude Opus 4.5', provider: 'anthropic' }];
      }
      return null;
    });

    const { container } = render(AgentCreateOverlay, {
      props: { onClose: vi.fn() },
    });
    await tick();
    await tick();
    await fireEvent.click(container.querySelector('[data-runtime="anthropic"]') as HTMLElement);
    await tick();
    await tick();
    await pickSearchableModel(container, 'Claude Opus 4.5');

    expect(container.textContent).toContain('Name your agent');
  });

  it('"Skip — decide later" also advances to the name step without a model selected', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'fetch_available_models') {
        return [{ id: 'claude-opus-4-5', name: 'Claude Opus 4.5', provider: 'anthropic' }];
      }
      return null;
    });

    const { container } = render(AgentCreateOverlay, {
      props: { onClose: vi.fn() },
    });
    await tick();
    await tick();
    await fireEvent.click(container.querySelector('[data-runtime="anthropic"]') as HTMLElement);
    await tick();
    await tick();
    await fireEvent.click(screen.getByText('Skip — decide later'));
    await tick();

    expect(container.textContent).toContain('Name your agent');
  });
});

// ---------------------------------------------------------------------------
// C — Creation: create_local_agent gets the picked runtime_type
// ---------------------------------------------------------------------------

describe('C — creating an agent passes the picked runtime_type (+ model)', () => {
  beforeEach(() => {
    (listRuntimeConnections as any).mockResolvedValue([
      makeConn({ runtime_type: 'ollama', connected: true, masked_credential: null, endpoint: 'http://localhost:11434' }),
    ]);
  });

  it('calls createLocalAgent with the selected runtime_type and model', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'fetch_available_models') {
        return [{ id: 'llama3', name: 'llama3', provider: 'ollama' }];
      }
      return null;
    });
    (createLocalAgent as any).mockResolvedValue(makeAgent({ runtime_type: 'ollama', model: 'llama3' }));

    const { container } = render(AgentCreateOverlay, {
      props: { onClose: vi.fn() },
    });
    await tick();
    await tick();

    await fireEvent.click(container.querySelector('[data-runtime="ollama"]') as HTMLElement);
    await tick();
    await tick();
    await pickSearchableModel(container, 'llama3');

    const nameInput = container.querySelector('.agent-name-input') as HTMLInputElement;
    await fireEvent.input(nameInput, { target: { value: 'Local Helper' } });
    await tick();

    const createBtn = Array.from(container.querySelectorAll('.ag-btn--primary')).find(
      (b) => b.textContent?.includes('Create Local Helper')
    ) as HTMLButtonElement;
    expect(createBtn).toBeTruthy();
    await fireEvent.click(createBtn);
    await tick();
    await tick();

    expect(createLocalAgent).toHaveBeenCalledWith(
      expect.objectContaining({ name: 'Local Helper', runtime_type: 'ollama', model: 'llama3' })
    );
  });

  it('creates with model=undefined when the user skipped model selection', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'fetch_available_models') {
        return [{ id: 'llama3', name: 'llama3', provider: 'ollama' }];
      }
      return null;
    });
    (createLocalAgent as any).mockResolvedValue(makeAgent({ runtime_type: 'ollama' }));

    const { container } = render(AgentCreateOverlay, {
      props: { onClose: vi.fn() },
    });
    await tick();
    await tick();

    await fireEvent.click(container.querySelector('[data-runtime="ollama"]') as HTMLElement);
    await tick();
    await tick();
    await fireEvent.click(screen.getByText('Skip — decide later'));
    await tick();

    const nameInput = container.querySelector('.agent-name-input') as HTMLInputElement;
    await fireEvent.input(nameInput, { target: { value: 'Local Helper' } });
    await tick();

    const createBtn = Array.from(container.querySelectorAll('.ag-btn--primary')).find(
      (b) => b.textContent?.includes('Create Local Helper')
    ) as HTMLButtonElement;
    await fireEvent.click(createBtn);
    await tick();
    await tick();

    expect(createLocalAgent).toHaveBeenCalledWith(
      expect.objectContaining({ name: 'Local Helper', runtime_type: 'ollama', model: undefined })
    );
  });
});

// ---------------------------------------------------------------------------
// D — Forced-create path is unaffected by the 6b picker
// ---------------------------------------------------------------------------

describe('D — forced-create (provider switch) skips the runtime/model picker', () => {
  it('starts at Step 0 (provider probe), never shows the Step R runtime picker', async () => {
    const { container } = render(AgentCreateOverlay, {
      props: { onClose: vi.fn(), forcedCreate: true },
    });
    await tick();
    await tick();

    expect(container.textContent).toContain('Set up your AI provider');
    expect(container.querySelector('[role="radiogroup"][aria-label="Provider"]')).toBeNull();
    // listRuntimeConnections should not even be called on this path
    expect(listRuntimeConnections).not.toHaveBeenCalled();
  });

  it('with valid pending credentials, goes straight to Name (no runtime/model step)', async () => {
    const { container } = render(AgentCreateOverlay, {
      props: {
        onClose: vi.fn(),
        forcedCreate: true,
        pendingProvider: 'anthropic',
        pendingApiKey: 'sk-ant-test',
      },
    });
    await tick();
    await tick();

    expect(container.textContent).toContain('Name your agent');
    expect(listRuntimeConnections).not.toHaveBeenCalled();
  });

  it('creating from the forced path uses pendingProvider as runtime_type, not a picker selection', async () => {
    (createLocalAgent as any).mockResolvedValue(makeAgent({ runtime_type: 'anthropic' }));

    const { container } = render(AgentCreateOverlay, {
      props: {
        onClose: vi.fn(),
        forcedCreate: true,
        pendingProvider: 'anthropic',
        pendingApiKey: 'sk-ant-test',
      },
    });
    await tick();
    await tick();

    const nameInput = container.querySelector('.agent-name-input') as HTMLInputElement;
    await fireEvent.input(nameInput, { target: { value: 'Switched Agent' } });
    await tick();

    const createBtn = Array.from(container.querySelectorAll('.ag-btn--primary')).find(
      (b) => b.textContent?.includes('Switched Agent')
    ) as HTMLButtonElement;
    await fireEvent.click(createBtn);
    await tick();
    await tick();

    expect(createLocalAgent).toHaveBeenCalledWith(
      expect.objectContaining({ name: 'Switched Agent', runtime_type: 'anthropic', model: undefined })
    );
  });
});

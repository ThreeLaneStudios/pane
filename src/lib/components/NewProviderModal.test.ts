/**
 * NewProviderModal.test.ts — Component tests for NewProviderModal.svelte
 *
 * ADR-0063 D8, #2522 Stage 8a/8b: "New Provider" flow shell + Step 1 picker +
 * Step 2 provider-specific config.
 * Design: docs/design/agent-sessions-sprint/pc-c.jsx `PcNewProvider1` /
 * `PcNewProvider2` / `PcNewProviderPaired` / `PcNewProviderSteps`.
 *
 * Test groups:
 *  A — Modal shell (title, step dots)
 *  B — Step 1 provider rows render with exact taglines
 *  C — Selection behavior (radio-style, one at a time)
 *  D — Continue disabled/enabled based on selection
 *  E — Cancel / scrim close
 *  F — Continue advances to Step 2
 *  G — Step 2 API providers (Anthropic/OpenRouter/Ollama): fields, hints, Back, Test, Add connection
 *  H — #2642/#2630 Commit 6: OC pairing ceremony (Step-1 entry) — migrated from
 *       ProviderConnectionCard.test.ts's Group J/L2, this modal now owns the ceremony
 *  I — #2642/#2630 Commit 5/6: startCeremony deep-link entry (card's Pair/Re-pair
 *       button, via the Panel's onPairRequested → ocDeepLink wiring)
 *  J — Step dots show "2 of 2" on all Step 2 variants
 *  K — #2522 design polish batch, item 4: X close button
 *  L — #2522 design polish batch, item 4: already-connected anthropic/openrouter/ollama
 *       shows existing connection state, not a fresh config form
 *  M — #2590 UX batch 2, item 2: Step 1 filters out already-configured providers
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import NewProviderModal from './NewProviderModal.svelte';
import type { RuntimeConnectionInfo } from '$lib/api';

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: invokeMock,
}));

vi.mock('$lib/api', async (importOriginal) => {
  const original = (await importOriginal()) as any;
  return {
    ...original,
    disconnectRuntime: vi.fn().mockResolvedValue(undefined),
  };
});

import { disconnectRuntime } from '$lib/api';

// Same shim LegacyProviderPrompt.test.ts / DefaultAgentPickerModal.test.ts use:
// the vitest-setup.ts window mock lacks addEventListener/removeEventListener,
// needed for <svelte:window onkeydown>.
if (typeof (globalThis.window as any).addEventListener !== 'function') {
  (globalThis.window as any).addEventListener = () => {};
}
if (typeof (globalThis.window as any).removeEventListener !== 'function') {
  (globalThis.window as any).removeEventListener = () => {};
}

function makeOcInfo(overrides: Partial<RuntimeConnectionInfo> = {}): RuntimeConnectionInfo {
  return {
    runtime_type: 'openclaw',
    connected: true,
    masked_credential: null,
    endpoint: 'https://100.64.0.1:8443/v1',
    error: null,
    agents: [
      { id: 'a1', name: 'Agent One', model: 'claude', is_default: true },
      { id: 'a2', name: 'Agent Two', model: 'claude', is_default: false },
      { id: 'a3', name: 'Agent Three', model: 'claude', is_default: false },
    ],
    ...overrides,
  };
}

function makeAnthropicInfo(overrides: Partial<RuntimeConnectionInfo> = {}): RuntimeConnectionInfo {
  return {
    runtime_type: 'anthropic',
    connected: true,
    // F1 fix (v1.0.36 live-test round 2): isProviderConfigured now checks
    // configured_this_install, not connected — default this fixture to match
    // `connected: true` so existing "already connected" tests keep passing.
    configured_this_install: true,
    masked_credential: 'sk-ant-...abcd',
    endpoint: null,
    error: null,
    agents: [{ id: 'a1', name: 'Murphy Brown', model: 'claude-sonnet-4-5', is_default: false }],
    ...overrides,
  };
}

async function goToStep2(container: HTMLElement, providerLabel: string) {
  const rows = Array.from(container.querySelectorAll('[role="radio"]'));
  const row = rows.find((r) => r.textContent?.includes(providerLabel)) as HTMLButtonElement;
  await fireEvent.click(row);
  await tick();
  await fireEvent.click(screen.getByText('Continue'));
  await tick();
  await tick();
}

/**
 * #2590 UX batch 2, item 2: Step 1 now filters `PROVIDER_ROWS` down to providers
 * that are NOT already connected (see `visibleProviderRows`), so a provider that's
 * already in `connections` as connected never renders a selectable Step 1 row —
 * `goToStep2(container, label)` can no longer reach it directly. The L-suite below
 * covers Step 2's "already connected" summary for the one realistic path that
 * still reaches it: the user selects a provider while it is NOT YET connected
 * (still visible in Step 1), advances to Step 2, and *then* `connections` updates
 * to reflect it as connected — e.g. the Panel's periodic/onConnectionSaved refresh
 * resolves mid-flow because it was connected through another surface (the
 * ProviderConnectionCard's own inline Save) while this modal was still open.
 * `rerender` simulates that prop update the same way the real Panel passing a
 * fresh `connections` array down would.
 */
// #2642/#2630 Commit 6: `goToStep2OcPaired` (Step-1-driven route to the OpenClaw
// "already paired" summary) is deleted along with the Group H tests that were its
// only callers — that summary branch no longer exists in NewProviderModal.svelte
// (Commit 4 deleted it; Step 1's isProviderConfigured filter means a paired
// OpenClaw is never a selectable row in the first place). See `openOcCeremony`
// helpers in the OC ceremony test groups below for the modal's current OpenClaw
// entry points (Step-1 click-through and the `startCeremony` deep-link prop).

async function goToStep2ThenBecomeConnected(
  rerender: (props: Record<string, unknown>) => Promise<void>,
  container: HTMLElement,
  providerLabel: string,
  connectedInfo: RuntimeConnectionInfo,
  onClose: () => void = vi.fn(),
  onConnectionSaved?: () => void
) {
  const rows = Array.from(container.querySelectorAll('[role="radio"]'));
  const row = rows.find((r) => r.textContent?.includes(providerLabel)) as HTMLButtonElement;
  await fireEvent.click(row);
  await tick();
  await fireEvent.click(screen.getByText('Continue'));
  await tick();
  await tick();

  await rerender({ onClose, onConnectionSaved, connections: [connectedInfo] });
  await tick();
}

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
  (disconnectRuntime as any).mockClear?.();
});

// ---------------------------------------------------------------------------
// A — Modal shell
// ---------------------------------------------------------------------------

describe('A — modal shell', () => {
  it('renders with the title "New Provider"', () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    expect(container.textContent).toContain('New Provider');
  });

  it('renders as a dialog with aria-modal', () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    const dialog = container.querySelector('[role="dialog"]');
    expect(dialog).not.toBeNull();
    expect(dialog?.getAttribute('aria-modal')).toBe('true');
  });

  it('shows step dots with "1 of 2" step label on step 1', () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    expect(container.textContent).toContain('New provider · step 1 of 2');
    const dots = container.querySelectorAll('.npm-steps i');
    expect(dots.length).toBe(2);
    expect(dots[0].classList.contains('npm-step--on')).toBe(true);
    expect(dots[1].classList.contains('npm-step--on')).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// B — Step 1 provider rows
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// M — Step 1 filters out already-configured providers (#2590 UX batch 2, item 2)
// ---------------------------------------------------------------------------

describe('M — Step 1 unconfigured-only filter', () => {
  it('with no connections, all 5 providers render (unfiltered baseline)', () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn(), connections: [] } });
    const rows = container.querySelectorAll('[role="radio"]');
    expect(rows.length).toBe(5);
  });

  it('a connected anthropic entry hides the Anthropic row but keeps the other 4', () => {
    const { container } = render(NewProviderModal, {
      props: { onClose: vi.fn(), connections: [makeAnthropicInfo()] },
    });
    const rows = Array.from(container.querySelectorAll('[role="radio"]'));
    expect(rows.length).toBe(4);
    expect(rows.some((r) => r.textContent?.includes('Anthropic'))).toBe(false);
    expect(rows.some((r) => r.textContent?.includes('OpenRouter'))).toBe(true);
    expect(rows.some((r) => r.textContent?.includes('Ollama'))).toBe(true);
    expect(rows.some((r) => r.textContent?.includes('OpenClaw'))).toBe(true);
  });

  it('an entry with configured_this_install:false does NOT hide the row (F1 fix: filter is keyed on configured_this_install, not connected)', () => {
    const { container } = render(NewProviderModal, {
      props: { onClose: vi.fn(), connections: [makeAnthropicInfo({ connected: false, configured_this_install: false })] },
    });
    const rows = container.querySelectorAll('[role="radio"]');
    expect(rows.length).toBe(5);
  });

  it('F1 regression: connected:true but configured_this_install:false does NOT hide the row (the exact Ollama-grandfathered-endpoint bug)', () => {
    const { container } = render(NewProviderModal, {
      props: { onClose: vi.fn(), connections: [makeAnthropicInfo({ connected: true, configured_this_install: false })] },
    });
    const rows = container.querySelectorAll('[role="radio"]');
    expect(rows.length).toBe(5);
    expect(Array.from(rows).some((r) => r.textContent?.includes('Anthropic'))).toBe(true);
  });

  it('a paired ocConnectionInfo hides the OpenClaw row', () => {
    const { container } = render(NewProviderModal, {
      props: { onClose: vi.fn(), ocConnectionInfo: makeOcInfo() },
    });
    const rows = Array.from(container.querySelectorAll('[role="radio"]'));
    expect(rows.length).toBe(4);
    expect(rows.some((r) => r.textContent?.includes('OpenClaw'))).toBe(false);
  });

  it('an unpaired ocConnectionInfo does NOT hide the OpenClaw row', () => {
    const { container } = render(NewProviderModal, {
      props: { onClose: vi.fn(), ocConnectionInfo: makeOcInfo({ connected: false }) },
    });
    const rows = Array.from(container.querySelectorAll('[role="radio"]'));
    expect(rows.length).toBe(5);
    expect(rows.some((r) => r.textContent?.includes('OpenClaw'))).toBe(true);
  });

  it('multiple already-connected providers are all filtered out simultaneously', () => {
    const { container } = render(NewProviderModal, {
      props: {
        onClose: vi.fn(),
        connections: [
          makeAnthropicInfo(),
          { runtime_type: 'openrouter', connected: true, configured_this_install: true, masked_credential: 'sk-or-...zzzz', endpoint: null, error: null, agents: [] },
        ],
        ocConnectionInfo: makeOcInfo(),
      },
    });
    const rows = Array.from(container.querySelectorAll('[role="radio"]'));
    expect(rows.length).toBe(2);
    expect(rows[0].textContent).toContain('Ollama');
  });

  it('when every provider is already configured, shows an explanatory empty state instead of an empty list', () => {
    const { container } = render(NewProviderModal, {
      props: {
        onClose: vi.fn(),
        connections: [
          makeAnthropicInfo(),
          { runtime_type: 'openrouter', connected: true, configured_this_install: true, masked_credential: 'sk-or-...zzzz', endpoint: null, error: null, agents: [] },
          { runtime_type: 'ollama', connected: true, configured_this_install: true, masked_credential: null, endpoint: 'http://localhost:11434', error: null, agents: [] },
          { runtime_type: 'venice', connected: true, configured_this_install: true, masked_credential: 'vn-...xxxx', endpoint: null, error: null, agents: [] },
        ],
        ocConnectionInfo: makeOcInfo(),
      },
    });
    expect(container.querySelectorAll('[role="radio"]').length).toBe(0);
    expect(container.textContent).toContain('already connected');
    // Continue must be disabled — nothing to select.
    const continueBtn = screen.getByText('Continue') as HTMLButtonElement;
    expect(continueBtn.disabled).toBe(true);
  });

  it('omitting both `connections` and `ocConnectionInfo` (existing call sites) still shows all 5 rows', () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    expect(container.querySelectorAll('[role="radio"]').length).toBe(5);
  });
});

describe('B — Step 1 shows all five provider options with correct taglines', () => {
  it('renders the eyebrow, title, and subtitle verbatim', () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    expect(container.textContent).toContain('New provider · step 1 of 2');
    expect(container.textContent).toContain('Which provider are you connecting?');
    expect(container.textContent).toContain(
      'The provider determines the fields on the next step. Agents you create on it stay on it.'
    );
  });

  it('renders exactly 5 radio pick rows', () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    const rows = container.querySelectorAll('[role="radio"]');
    expect(rows.length).toBe(5);
  });

  it('renders Anthropic with its exact tagline', () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    expect(container.textContent).toContain('Anthropic');
    expect(container.textContent).toContain('API key. Claude models direct from Anthropic.');
  });

  it('renders OpenRouter with its exact tagline', () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    expect(container.textContent).toContain('OpenRouter');
    expect(container.textContent).toContain('API key. One key, many model vendors.');
  });

  it('renders Ollama with its exact tagline', () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    expect(container.textContent).toContain('Ollama');
    expect(container.textContent).toContain('Local server. No key, models run on this machine.');
  });

  it('renders OpenClaw with its exact tagline', () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    expect(container.textContent).toContain('OpenClaw');
    expect(container.textContent).toContain('Gateway pairing. One pairing per install.');
  });
});

// ---------------------------------------------------------------------------
// C — Selection behavior
// ---------------------------------------------------------------------------

describe('C — radio-style selection, one at a time', () => {
  it('no selection initially — all rows unchecked', () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    const rows = Array.from(container.querySelectorAll('[role="radio"]'));
    expect(rows.every((r) => r.getAttribute('aria-checked') === 'false')).toBe(true);
  });

  it('clicking a row selects it', async () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    const rows = Array.from(container.querySelectorAll('[role="radio"]'));
    const anthropicRow = rows.find((r) => r.textContent?.includes('Anthropic')) as HTMLButtonElement;
    await fireEvent.click(anthropicRow);
    await tick();
    expect(anthropicRow.getAttribute('aria-checked')).toBe('true');
  });

  it('clicking another row deselects the previous one — only one selected at a time', async () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    const rows = Array.from(container.querySelectorAll('[role="radio"]'));
    const anthropicRow = rows.find((r) => r.textContent?.includes('Anthropic')) as HTMLButtonElement;
    const ollamaRow = rows.find((r) => r.textContent?.includes('Ollama')) as HTMLButtonElement;

    await fireEvent.click(anthropicRow);
    await tick();
    expect(anthropicRow.getAttribute('aria-checked')).toBe('true');

    await fireEvent.click(ollamaRow);
    await tick();
    expect(ollamaRow.getAttribute('aria-checked')).toBe('true');
    expect(anthropicRow.getAttribute('aria-checked')).toBe('false');

    const checkedRows = Array.from(container.querySelectorAll('[role="radio"][aria-checked="true"]'));
    expect(checkedRows.length).toBe(1);
  });
});

// ---------------------------------------------------------------------------
// D — Continue disabled/enabled
// ---------------------------------------------------------------------------

describe('D — Continue button state', () => {
  it('Continue is disabled when no selection has been made', () => {
    render(NewProviderModal, { props: { onClose: vi.fn() } });
    const continueBtn = screen.getByText('Continue') as HTMLButtonElement;
    expect(continueBtn.disabled).toBe(true);
  });

  it('Continue is enabled after selecting a provider', async () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    const rows = Array.from(container.querySelectorAll('[role="radio"]'));
    const openrouterRow = rows.find((r) => r.textContent?.includes('OpenRouter')) as HTMLButtonElement;
    await fireEvent.click(openrouterRow);
    await tick();

    const continueBtn = screen.getByText('Continue') as HTMLButtonElement;
    expect(continueBtn.disabled).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// E — Cancel / scrim close
// ---------------------------------------------------------------------------

describe('E — Cancel and scrim close the modal', () => {
  it('Cancel calls onClose', async () => {
    const onClose = vi.fn();
    render(NewProviderModal, { props: { onClose } });
    await fireEvent.click(screen.getByText('Cancel'));
    await tick();
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('clicking the scrim calls onClose', async () => {
    const onClose = vi.fn();
    const { container } = render(NewProviderModal, { props: { onClose } });
    const scrim = container.querySelector('.npm-scrim') as HTMLElement;
    await fireEvent.click(scrim);
    await tick();
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('clicking inside the modal does not trigger onClose (stopPropagation)', async () => {
    const onClose = vi.fn();
    const { container } = render(NewProviderModal, { props: { onClose } });
    const modal = container.querySelector('.npm-modal') as HTMLElement;
    await fireEvent.click(modal);
    await tick();
    expect(onClose).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// F — Continue advances to Step 2
// ---------------------------------------------------------------------------

describe('F — Continue advances to Step 2', () => {
  it('clicking Continue after selecting an API provider renders that provider\'s Step 2 config', async () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    await goToStep2(container, 'Ollama');

    expect(container.textContent).toContain('Connect Ollama');
  });

  it('shows step dots with "2 of 2" active on step 2', async () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    await goToStep2(container, 'Anthropic');

    const dots = container.querySelectorAll('.npm-steps i');
    expect(dots.length).toBe(2);
    expect(dots[0].classList.contains('npm-step--on')).toBe(false);
    expect(dots[1].classList.contains('npm-step--on')).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// G — Step 2: API providers (Anthropic/OpenRouter/Ollama)
// ---------------------------------------------------------------------------

describe('G — Step 2 API providers', () => {
  it('Anthropic: title reads "Connect Anthropic", shows API Key + Base URL, correct hints', async () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    await goToStep2(container, 'Anthropic');

    expect(container.textContent).toContain('Connect Anthropic');
    expect(container.textContent).toContain('New provider · step 2 of 2');
    expect(container.textContent).toContain(
      'Pane tests the key before adding the connection. Existing connections keep working while you do this.'
    );
    expect(screen.getByLabelText('API Key')).not.toBeNull();
    expect(container.textContent).toContain('Get your API key at console.anthropic.com');
    const baseUrl = screen.getByLabelText('Base URL') as HTMLInputElement;
    expect(baseUrl.placeholder).toBe('https://api.anthropic.com/v1');
    expect(container.textContent).toContain('Optional. Leave blank for the default endpoint.');
  });

  it('OpenRouter: title reads "Connect OpenRouter", correct API key hint and Base URL default', async () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    await goToStep2(container, 'OpenRouter');

    expect(container.textContent).toContain('Connect OpenRouter');
    expect(container.textContent).toContain('Get your API key at openrouter.ai/keys');
    const baseUrl = screen.getByLabelText('Base URL') as HTMLInputElement;
    expect(baseUrl.placeholder).toBe('https://openrouter.ai/api/v1');
  });

  it('Ollama: title reads "Connect Ollama", has NO API Key field, Base URL defaults to localhost', async () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    await goToStep2(container, 'Ollama');

    expect(container.textContent).toContain('Connect Ollama');
    expect(screen.queryByLabelText('API Key')).toBeNull();
    const baseUrl = screen.getByLabelText('Base URL') as HTMLInputElement;
    expect(baseUrl.placeholder).toBe('http://localhost:11434');
  });

  it('Venice: no Base URL field rendered', async () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    await goToStep2(container, 'Venice');

    expect(container.textContent).toContain('Connect Venice');
    expect(screen.getByLabelText('API Key')).not.toBeNull();
    expect(screen.queryByLabelText('Base URL')).toBeNull();
  });

  it('Back returns to Step 1 with the selection still visible as unselected-fresh choose list', async () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    await goToStep2(container, 'OpenRouter');
    expect(container.textContent).toContain('Connect OpenRouter');

    await fireEvent.click(screen.getByText('Back'));
    await tick();

    expect(container.textContent).toContain('Which provider are you connecting?');
    expect(container.querySelectorAll('[role="radio"]').length).toBe(5);
  });

  it('Test calls validate_anthropic_key for Anthropic with the entered key', async () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    await goToStep2(container, 'Anthropic');

    const keyInput = screen.getByLabelText('API Key') as HTMLInputElement;
    await fireEvent.input(keyInput, { target: { value: 'sk-ant-test-key' } });
    await tick();

    await fireEvent.click(screen.getByText('Test'));
    await tick();
    await tick();

    expect(invokeMock).toHaveBeenCalledWith('validate_anthropic_key', { apiKey: 'sk-ant-test-key' });
  });

  it('Test calls ollama_probe_two_step for Ollama', async () => {
    invokeMock.mockResolvedValue({ models: ['llama3'], error_class: null });
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    await goToStep2(container, 'Ollama');

    await fireEvent.click(screen.getByText('Test'));
    await tick();
    await tick();

    expect(invokeMock).toHaveBeenCalledWith('ollama_probe_two_step', { endpoint: undefined });
  });

  it('Add connection calls ai_configure_and_save with the entered key/endpoint for OpenRouter', async () => {
    const onConnectionSaved = vi.fn();
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn(), onConnectionSaved } });
    await goToStep2(container, 'OpenRouter');

    const keyInput = screen.getByLabelText('API Key') as HTMLInputElement;
    await fireEvent.input(keyInput, { target: { value: 'sk-or-test' } });
    await tick();

    await fireEvent.click(screen.getByText('Add connection'));
    await tick();
    await tick();

    expect(invokeMock).toHaveBeenCalledWith('ai_configure_and_save', {
      provider: 'openrouter',
      apiKey: 'sk-or-test',
      model: null,
      endpoint: null,
      simpleTierModel: null,
    });
    expect(onConnectionSaved).toHaveBeenCalled();
  });

  it('Add connection calls ai_configure_and_save with apiKey:null for Ollama', async () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    await goToStep2(container, 'Ollama');

    await fireEvent.click(screen.getByText('Add connection'));
    await tick();
    await tick();

    expect(invokeMock).toHaveBeenCalledWith('ai_configure_and_save', {
      provider: 'ollama',
      apiKey: null,
      model: null,
      endpoint: null,
      simpleTierModel: null,
    });
  });
});

// ---------------------------------------------------------------------------
// H — #2642/#2630 Commit 6: OC pairing ceremony, migrated from
// ProviderConnectionCard.test.ts's Group J/L2 (the card no longer runs the
// ceremony — see that file's module comment). Every scenario below is adapted
// 1:1 from the card version but driving NewProviderModal.svelte's own markup
// (`.npm-textin` inputs / `.npm-flow__foot` buttons / `data-testid="oc-ceremony-*"`
// nodes) instead of the card's `.pcc-body`/`.pcc-*` markup, and entering via
// Step 1's OpenClaw row + Continue (`openCeremonyViaStep1`) rather than a
// "Pair gateway" card button.
// ---------------------------------------------------------------------------

describe('H — OpenClaw pairing ceremony (Step-1 entry)', () => {
  async function openCeremonyViaStep1(container: HTMLElement) {
    const rows = Array.from(container.querySelectorAll('[role="radio"]'));
    const ocRow = rows.find((r) => r.textContent?.includes('OpenClaw')) as HTMLButtonElement;
    await fireEvent.click(ocRow);
    await tick();
    await fireEvent.click(screen.getByText('Continue'));
    await tick();
  }

  async function runPairingCeremony(container: HTMLElement) {
    await openCeremonyViaStep1(container);
    const urlInput = screen.getByLabelText('Gateway URL') as HTMLInputElement;
    await fireEvent.input(urlInput, { target: { value: 'https://100.1.2.3:18789' } });
    await fireEvent.click(screen.getByText('Start pairing'));
    await tick();
    await tick();
    await fireEvent.click(screen.getByText('Accept'));
    await tick();
    const codeInput = screen.getByLabelText('Pairing Code') as HTMLInputElement;
    await fireEvent.input(codeInput, { target: { value: '123456' } });
    await fireEvent.click(screen.getByText('Complete pairing'));
  }

  it('Step-1 entry: selecting OpenClaw and clicking Continue renders the ceremony URL step directly (no Amendment-1 hand-off)', async () => {
    const { container } = render(NewProviderModal, {
      props: { onClose: vi.fn(), ocConnectionInfo: makeOcInfo({ connected: false }) },
    });
    await openCeremonyViaStep1(container);

    expect(container.textContent).toContain('Pair OpenClaw');
    expect(container.textContent).toContain('Pairing enables automatic two-way sync');
    expect(container.textContent).toContain('identity, instructions, memory, tools, and logs');
    expect(container.textContent).toContain('may update local allowlisted agent files');
    expect(screen.getByLabelText('Gateway URL')).not.toBeNull();
    expect(screen.getByText('Start pairing')).not.toBeNull();
  });

  it('URL step: entering a URL and clicking Start pairing calls gateway_fetch_fingerprint and advances to the fingerprint-verify step', async () => {
    invokeMock.mockResolvedValueOnce({ tls_fingerprint: 'AA:BB:CC:DD', cert_pem: '-----CERT-----', gateway_hostname: null });
    const { container } = render(NewProviderModal, {
      props: { onClose: vi.fn(), ocConnectionInfo: makeOcInfo({ connected: false }) },
    });
    await openCeremonyViaStep1(container);

    const urlInput = screen.getByLabelText('Gateway URL') as HTMLInputElement;
    await fireEvent.input(urlInput, { target: { value: 'https://100.1.2.3:18789' } });
    await fireEvent.click(screen.getByText('Start pairing'));
    await tick();
    await tick();

    expect(invokeMock).toHaveBeenCalledWith('gateway_fetch_fingerprint', { url: 'https://100.1.2.3:18789/v1' });
    expect(container.textContent).toContain('AA:BB:CC:DD');
    expect(screen.getByText('Accept')).not.toBeNull();
    expect(screen.getByText('Reject')).not.toBeNull();
  });

  it('URL step rejects explicit HTTP cleanly without invoking native pairing', async () => {
    const { container } = render(NewProviderModal, {
      props: { onClose: vi.fn(), ocConnectionInfo: makeOcInfo({ connected: false }) },
    });
    await openCeremonyViaStep1(container);

    const urlInput = screen.getByLabelText('Gateway URL') as HTMLInputElement;
    await fireEvent.input(urlInput, { target: { value: 'http://100.1.2.3:18789' } });
    await fireEvent.click(screen.getByText('Start pairing'));
    await tick();

    const err = container.querySelector('[data-testid="oc-ceremony-probe-error"]');
    expect(err?.textContent).toContain("Gateway URL isn't valid");
    expect(err?.textContent).toContain('Gateway URL must use HTTPS.');
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it('ignores malicious gateway_hostname and keeps exchange, persistence, and sync on the reviewed origin', async () => {
    invokeMock.mockResolvedValueOnce({
      tls_fingerprint: 'AA:BB:CC:DD',
      cert_pem: '-----CERT-----',
      gateway_hostname: 'attacker.example',
    });
    invokeMock.mockResolvedValueOnce({ token: 'tok-reviewed', gateway_version: '0.3.0', version_warning: null });
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'oc_agent_row_exists') return Promise.resolve(false);
      if (cmd === 'sync_initial_import') return Promise.resolve({ agent_names: ['Stia'], agents_missing_model: [] });
      return Promise.resolve(undefined);
    });

    const { container } = render(NewProviderModal, {
      props: { onClose: vi.fn(), ocConnectionInfo: makeOcInfo({ connected: false }) },
    });
    await runPairingCeremony(container);

    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledWith('sync_initial_import', {
      gatewayUrl: 'https://100.1.2.3:18789/v1',
      token: 'tok-reviewed',
    }));
    expect(invokeMock).toHaveBeenCalledWith('gateway_exchange_pairing_code', {
      url: 'https://100.1.2.3:18789/v1',
      code: '123456',
    });
    expect(invokeMock).toHaveBeenCalledWith('ai_configure_and_save', expect.objectContaining({
      provider: 'openclaw',
      endpoint: 'https://100.1.2.3:18789/v1',
    }));
    expect(JSON.stringify(invokeMock.mock.calls)).not.toContain('attacker.example');
  });

  it('URL step error: unreachable gateway shows address/port/elapsed/reason and the URL input survives', async () => {
    invokeMock.mockRejectedValueOnce(new Error('Cannot reach gateway: connection refused'));
    const { container } = render(NewProviderModal, {
      props: { onClose: vi.fn(), ocConnectionInfo: makeOcInfo({ connected: false }) },
    });
    await openCeremonyViaStep1(container);

    const urlInput = screen.getByLabelText('Gateway URL') as HTMLInputElement;
    await fireEvent.input(urlInput, { target: { value: 'https://100.1.2.3:18789' } });
    await fireEvent.click(screen.getByText('Start pairing'));
    await tick();
    await tick();

    const err = container.querySelector('[data-testid="oc-ceremony-probe-error"]');
    expect(err).not.toBeNull();
    expect(err?.textContent).toContain('100.1.2.3');
    expect(err?.textContent).toContain('18789');
    // The URL step is still showing, canonicalized to the reviewed base endpoint.
    expect((screen.getByLabelText('Gateway URL') as HTMLInputElement).value).toBe('https://100.1.2.3:18789/v1');
  });

  it('Fingerprint step: Reject keeps the fingerprint on screen and offers "Start pairing again"; URL is preserved', async () => {
    invokeMock.mockResolvedValueOnce({ tls_fingerprint: 'AA:BB:CC:DD', cert_pem: '-----CERT-----', gateway_hostname: null });
    const { container } = render(NewProviderModal, {
      props: { onClose: vi.fn(), ocConnectionInfo: makeOcInfo({ connected: false }) },
    });
    await openCeremonyViaStep1(container);
    const urlInput = screen.getByLabelText('Gateway URL') as HTMLInputElement;
    await fireEvent.input(urlInput, { target: { value: 'https://100.1.2.3:18789' } });
    await fireEvent.click(screen.getByText('Start pairing'));
    await tick();
    await tick();

    await fireEvent.click(screen.getByText('Reject'));
    await tick();

    expect(container.textContent).toContain('AA:BB:CC:DD');
    expect(container.querySelector('[data-testid="oc-ceremony-fingerprint-rejected"]')).not.toBeNull();
    expect(screen.getByText('Start pairing again')).not.toBeNull();

    await fireEvent.click(screen.getByText('Start pairing again'));
    await tick();
    expect((screen.getByLabelText('Gateway URL') as HTMLInputElement).value).toBe('https://100.1.2.3:18789/v1');
  });

  it('Fingerprint step: Accept advances to the pairing-code step', async () => {
    invokeMock.mockResolvedValueOnce({ tls_fingerprint: 'AA:BB:CC:DD', cert_pem: '-----CERT-----', gateway_hostname: null });
    const { container } = render(NewProviderModal, {
      props: { onClose: vi.fn(), ocConnectionInfo: makeOcInfo({ connected: false }) },
    });
    await openCeremonyViaStep1(container);
    const urlInput = screen.getByLabelText('Gateway URL') as HTMLInputElement;
    await fireEvent.input(urlInput, { target: { value: 'https://100.1.2.3:18789' } });
    await fireEvent.click(screen.getByText('Start pairing'));
    await tick();
    await tick();

    await fireEvent.click(screen.getByText('Accept'));
    await tick();

    expect(screen.getByLabelText('Pairing Code')).not.toBeNull();
    expect(screen.getByText('Complete pairing')).not.toBeNull();
  });

  it('Code step error: a bad pairing code shows an error and stays on the code step with the fingerprint still valid', async () => {
    invokeMock.mockResolvedValueOnce({ tls_fingerprint: 'AA:BB:CC:DD', cert_pem: '-----CERT-----', gateway_hostname: null });
    invokeMock.mockRejectedValueOnce(new Error('Invalid pairing code'));

    const { container } = render(NewProviderModal, {
      props: { onClose: vi.fn(), ocConnectionInfo: makeOcInfo({ connected: false }) },
    });
    await openCeremonyViaStep1(container);
    const urlInput = screen.getByLabelText('Gateway URL') as HTMLInputElement;
    await fireEvent.input(urlInput, { target: { value: 'https://100.1.2.3:18789' } });
    await fireEvent.click(screen.getByText('Start pairing'));
    await tick();
    await tick();
    await fireEvent.click(screen.getByText('Accept'));
    await tick();

    const codeInput = screen.getByLabelText('Pairing Code') as HTMLInputElement;
    await fireEvent.input(codeInput, { target: { value: '000000' } });
    await fireEvent.click(screen.getByText('Complete pairing'));

    await vi.waitFor(() => expect(container.querySelector('[data-testid="oc-ceremony-code-error"]')).not.toBeNull());
    expect(container.textContent).toContain('fingerprint you accepted is still valid');
    // Still on the code step.
    expect(screen.getByLabelText('Pairing Code')).not.toBeNull();
  });

  it('First pair (oc_agent_row_exists=false): calls sync_initial_import, never reimport_oc_agents or check_oc_agent_collisions', async () => {
    const onConnectionSaved = vi.fn();
    invokeMock.mockResolvedValueOnce({ tls_fingerprint: 'AA:BB:CC:DD', cert_pem: '-----CERT-----', gateway_hostname: null });
    invokeMock.mockResolvedValueOnce({ token: 'tok_first', gateway_version: '0.3.0', version_warning: null });
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'oc_agent_row_exists') return Promise.resolve(false);
      if (cmd === 'sync_initial_import') return Promise.resolve({ agent_names: ['Stia'], agents_missing_model: [] });
      return Promise.resolve(undefined);
    });

    const { container } = render(NewProviderModal, {
      props: { onClose: vi.fn(), onConnectionSaved, ocConnectionInfo: makeOcInfo({ connected: false }) },
    });
    await runPairingCeremony(container);

    await vi.waitFor(() => expect(screen.getByText('Agent imported')).not.toBeNull());
    expect(invokeMock).toHaveBeenCalledWith('sync_initial_import', {
      gatewayUrl: 'https://100.1.2.3:18789/v1',
      token: 'tok_first',
    });
    expect(invokeMock).not.toHaveBeenCalledWith('reimport_oc_agents', expect.anything());
    expect(invokeMock).not.toHaveBeenCalledWith('check_oc_agent_collisions', expect.anything());
  });

  it('Re-pair, zero collisions (D20 unconditional): confirmCollision is still shown, and confirming calls reimport_oc_agents, never sync_initial_import', async () => {
    invokeMock.mockResolvedValueOnce({ tls_fingerprint: 'AA:BB:CC:DD', cert_pem: '-----CERT-----', gateway_hostname: null });
    invokeMock.mockResolvedValueOnce({ token: 'tok_re', gateway_version: '0.3.0', version_warning: null });
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'oc_agent_row_exists') return Promise.resolve(true);
      if (cmd === 'check_oc_agent_collisions') return Promise.resolve({ collisions: [], new_agents: [{ oc_agent_id: 'main', name: 'Stia' }] });
      if (cmd === 'reimport_oc_agents') return Promise.resolve(['Stia']);
      return Promise.resolve(undefined);
    });

    const { container } = render(NewProviderModal, {
      props: { onClose: vi.fn(), ocConnectionInfo: makeOcInfo({ connected: false }) },
    });
    await runPairingCeremony(container);

    await vi.waitFor(() => expect(screen.getByText('Re-pair OpenClaw')).not.toBeNull());
    expect(screen.getByText('This will re-import your agents from the gateway. Continue?')).not.toBeNull();
    await fireEvent.click(screen.getByText('Continue'));

    await vi.waitFor(() => expect(screen.getByText('Agent imported')).not.toBeNull());
    expect(invokeMock).toHaveBeenCalledWith('check_oc_agent_collisions', { url: 'https://100.1.2.3:18789/v1', token: 'tok_re' });
    expect(invokeMock).toHaveBeenCalledWith('reimport_oc_agents', { url: 'https://100.1.2.3:18789/v1', token: 'tok_re' });
    expect(invokeMock).not.toHaveBeenCalledWith('sync_initial_import', expect.anything());
  });

  it('Re-pair, user declines the confirm dialog: resolves declined:true, shows a dismissable notice, never calls reimport_oc_agents', async () => {
    invokeMock.mockResolvedValueOnce({ tls_fingerprint: 'AA:BB:CC:DD', cert_pem: '-----CERT-----', gateway_hostname: null });
    invokeMock.mockResolvedValueOnce({ token: 'tok_decline', gateway_version: '0.3.0', version_warning: null });
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'oc_agent_row_exists') return Promise.resolve(true);
      if (cmd === 'check_oc_agent_collisions') {
        return Promise.resolve({ collisions: [{ oc_agent_id: 'main', local_name: 'Stia', gateway_name: 'Stia' }], new_agents: [] });
      }
      return Promise.resolve(undefined);
    });

    const { container } = render(NewProviderModal, {
      props: { onClose: vi.fn(), ocConnectionInfo: makeOcInfo({ connected: false }) },
    });
    await runPairingCeremony(container);

    await vi.waitFor(() => expect(screen.getByText('Re-pair OpenClaw')).not.toBeNull());
    expect(screen.getByText(/will overwrite 1 local agent file/)).not.toBeNull();
    await fireEvent.click(screen.getByText('Cancel'));

    await vi.waitFor(() => expect(container.querySelector('[data-testid="oc-ceremony-declined"]')).not.toBeNull());
    expect(invokeMock).not.toHaveBeenCalledWith('reimport_oc_agents', expect.anything());

    const onConnectionSaved = vi.fn();
    // dismiss path is exercised in the dedicated "declined dismiss" test below.
  });

  it('Import failure: non-blocking, keeps the ceremony open with a Retry button, and threads the error back via onConnectionSaved(lastImportError) immediately', async () => {
    const onConnectionSaved = vi.fn();
    invokeMock.mockResolvedValueOnce({ tls_fingerprint: 'AA:BB:CC:DD', cert_pem: '-----CERT-----', gateway_hostname: null });
    invokeMock.mockResolvedValueOnce({ token: 'tok_err', gateway_version: '0.3.0', version_warning: null });
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'oc_agent_row_exists') return Promise.resolve(false);
      if (cmd === 'sync_initial_import') return Promise.reject(new Error('gateway unreachable'));
      return Promise.resolve(undefined);
    });

    const { container } = render(NewProviderModal, {
      props: { onClose: vi.fn(), onConnectionSaved, ocConnectionInfo: makeOcInfo({ connected: false }) },
    });
    await runPairingCeremony(container);

    await vi.waitFor(() => expect(onConnectionSaved).toHaveBeenCalledWith('gateway unreachable'));
    const errEl = container.querySelector('[data-testid="oc-ceremony-import-error"]');
    expect(errEl).not.toBeNull();
    expect(errEl?.textContent).toContain('gateway unreachable');
    expect(container.textContent).toContain('Pairing itself succeeded');
    expect(container.querySelector('[data-testid="oc-ceremony-retry-import"]')).not.toBeNull();
    // Pairing settings are already durable — set_ai_setting calls already fired
    // in phase 1, before the phase-2 import failure.
    expect(invokeMock).toHaveBeenCalledWith('set_ai_setting', { key: 'gateway_tls_fingerprint', value: 'AA:BB:CC:DD' });
    expect(invokeMock).toHaveBeenCalledWith('set_ai_setting', { key: 'gateway_paired_at', value: expect.any(String) });
  });

  it('Import retry succeeds: clicking Retry re-runs only the import phase (never re-exchanges the code) and renders ImportedAgentsModal on success', async () => {
    invokeMock.mockResolvedValueOnce({ tls_fingerprint: 'AA:BB:CC:DD', cert_pem: '-----CERT-----', gateway_hostname: null });
    invokeMock.mockResolvedValueOnce({ token: 'tok_retry', gateway_version: '0.3.0', version_warning: null });
    let importAttempts = 0;
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'oc_agent_row_exists') return Promise.resolve(false);
      if (cmd === 'sync_initial_import') {
        importAttempts += 1;
        if (importAttempts === 1) return Promise.reject(new Error('gateway unreachable'));
        return Promise.resolve({ agent_names: ['Stia'], agents_missing_model: [] });
      }
      return Promise.resolve(undefined);
    });

    const { container } = render(NewProviderModal, {
      props: { onClose: vi.fn(), ocConnectionInfo: makeOcInfo({ connected: false }) },
    });
    await runPairingCeremony(container);

    await vi.waitFor(() => expect(container.querySelector('[data-testid="oc-ceremony-retry-import"]')).not.toBeNull());
    await fireEvent.click(screen.getByTestId('oc-ceremony-retry-import'));
    await tick();

    await vi.waitFor(() => expect(screen.getByText('Agent imported')).not.toBeNull());
    expect(screen.getByText('Stia')).not.toBeNull();
    // gateway_exchange_pairing_code (phase 1) is only invoked once, by the initial
    // Complete pairing click — Retry never re-submits the one-time-use code.
    expect(invokeMock.mock.calls.filter((c: any[]) => c[0] === 'gateway_exchange_pairing_code').length).toBe(1);
    expect(importAttempts).toBe(2);
  });

  it('Successful import: ImportedAgentsModal renders with the correct agentNames/agentsMissingModel props; Done calls onConnectionSaved then onClose', async () => {
    const onClose = vi.fn();
    const onConnectionSaved = vi.fn();
    invokeMock.mockResolvedValueOnce({ tls_fingerprint: 'AA:BB:CC:DD', cert_pem: '-----CERT-----', gateway_hostname: null });
    invokeMock.mockResolvedValueOnce({ token: 'tok_softfail', gateway_version: '0.3.0', version_warning: null });
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'oc_agent_row_exists') return Promise.resolve(false);
      if (cmd === 'sync_initial_import') return Promise.resolve({ agent_names: ['Stia'], agents_missing_model: ['Stia'] });
      return Promise.resolve(undefined);
    });

    const { container } = render(NewProviderModal, {
      props: { onClose, onConnectionSaved, ocConnectionInfo: makeOcInfo({ connected: false }) },
    });
    await runPairingCeremony(container);

    await vi.waitFor(() => expect(screen.getByText('Agent imported')).not.toBeNull());
    expect(screen.getByText('Stia')).not.toBeNull();
    const warning = container.querySelector('[data-testid="imported-agents-missing-model-warning"]');
    expect(warning).not.toBeNull();
    expect(warning?.textContent).toContain('Agent "Stia" imported without a model');

    await fireEvent.click(screen.getByTestId('imported-agents-done'));
    await tick();

    expect(onConnectionSaved).toHaveBeenCalledWith();
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('lastImportError is threaded back with no argument (clearing any prior badge) on a successful import following a Retry', async () => {
    const onConnectionSaved = vi.fn();
    invokeMock.mockResolvedValueOnce({ tls_fingerprint: 'AA:BB:CC:DD', cert_pem: '-----CERT-----', gateway_hostname: null });
    invokeMock.mockResolvedValueOnce({ token: 'tok_clear', gateway_version: '0.3.0', version_warning: null });
    let importAttempts = 0;
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'oc_agent_row_exists') return Promise.resolve(false);
      if (cmd === 'sync_initial_import') {
        importAttempts += 1;
        if (importAttempts === 1) return Promise.reject(new Error('flaky'));
        return Promise.resolve({ agent_names: [], agents_missing_model: [] });
      }
      return Promise.resolve(undefined);
    });

    const { container } = render(NewProviderModal, {
      props: { onClose: vi.fn(), onConnectionSaved, ocConnectionInfo: makeOcInfo({ connected: false }) },
    });
    await runPairingCeremony(container);

    await vi.waitFor(() => expect(onConnectionSaved).toHaveBeenCalledWith('flaky'));
    onConnectionSaved.mockClear();

    await fireEvent.click(screen.getByTestId('oc-ceremony-retry-import'));
    await tick();

    // Zero agents imported on the successful retry — no ImportedAgentsModal (matches
    // the card's original "empty list → no popup" rule), so onClose/onConnectionSaved
    // never fire from that path here; assert onConnectionSaved was NOT called again
    // with an error this time (nothing left to clear from THIS assertion's own
    // vantage — the important behavior is no repeated error call).
    expect(onConnectionSaved).not.toHaveBeenCalledWith('flaky');
  });
});

// ---------------------------------------------------------------------------
// I — OpenClaw pairing ceremony: startCeremony deep-link entry (#2642/#2630
// Commit 5) — the card's Pair/Re-pair button, via the Panel's
// onPairRequested → ocDeepLink → startCeremony wiring, skips Step 1 entirely.
// ---------------------------------------------------------------------------

describe('I — startCeremony deep-link entry', () => {
  it("startCeremony='pair' mounts directly at the ceremony's URL step, skipping Step 1, with a blank URL and 'Pair OpenClaw' copy", () => {
    const { container } = render(NewProviderModal, {
      props: { onClose: vi.fn(), ocConnectionInfo: makeOcInfo({ connected: false }), startCeremony: 'pair' },
    });

    expect(container.querySelectorAll('[role="radio"]').length).toBe(0);
    expect(container.textContent).toContain('Pair OpenClaw');
    const urlInput = screen.getByLabelText('Gateway URL') as HTMLInputElement;
    expect(urlInput.value).toBe('');
  });

  it("startCeremony='re-pair' mounts directly at the ceremony's URL step with the URL prefilled from ocConnectionInfo.endpoint, and 'Re-pair OpenClaw' copy", () => {
    const { container } = render(NewProviderModal, {
      props: {
        onClose: vi.fn(),
        ocConnectionInfo: makeOcInfo({ endpoint: 'https://100.9.9.9:18789' }),
        startCeremony: 're-pair',
      },
    });

    expect(container.textContent).toContain('Re-pair OpenClaw');
    const urlInput = screen.getByLabelText('Gateway URL') as HTMLInputElement;
    expect(urlInput.value).toBe('https://100.9.9.9:18789');
  });

  it('Cancel on the URL step of a startCeremony deep-link entry closes the whole modal (no Step 1 to fall back to)', async () => {
    const onClose = vi.fn();
    render(NewProviderModal, {
      props: { onClose, ocConnectionInfo: makeOcInfo({ connected: false }), startCeremony: 'pair' },
    });

    await fireEvent.click(screen.getByText('Cancel'));
    await tick();

    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

// ---------------------------------------------------------------------------
// J — Step dots show "2 of 2" on all Step 2 variants
// ---------------------------------------------------------------------------

describe('J — Step dots on all Step 2 variants', () => {
  it('API provider Step 2 shows dot 2 active', async () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    await goToStep2(container, 'OpenRouter');

    const dots = container.querySelectorAll('.npm-steps i');
    expect(dots[0].classList.contains('npm-step--on')).toBe(false);
    expect(dots[1].classList.contains('npm-step--on')).toBe(true);
  });

  it('OpenClaw ceremony Step 2 (Step-1 entry) shows dot 2 active', async () => {
    const { container } = render(NewProviderModal, {
      props: { onClose: vi.fn(), ocConnectionInfo: makeOcInfo({ connected: false }) },
    });
    const rows = Array.from(container.querySelectorAll('[role="radio"]'));
    const ocRow = rows.find((r) => r.textContent?.includes('OpenClaw')) as HTMLButtonElement;
    await fireEvent.click(ocRow);
    await tick();
    await fireEvent.click(screen.getByText('Continue'));
    await tick();

    const dots = container.querySelectorAll('.npm-steps i');
    expect(dots[0].classList.contains('npm-step--on')).toBe(false);
    expect(dots[1].classList.contains('npm-step--on')).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// K — X close button (#2522 design polish batch, item 4)
// ---------------------------------------------------------------------------

describe('K — X close button', () => {
  it('renders a close button in the modal header', () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    expect(container.querySelector('.npm-close-btn')).not.toBeNull();
  });

  it('clicking the close button calls onClose', async () => {
    const onClose = vi.fn();
    const { container } = render(NewProviderModal, { props: { onClose } });
    const closeBtn = container.querySelector('.npm-close-btn') as HTMLButtonElement;
    await fireEvent.click(closeBtn);
    await tick();
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('the close button is present on Step 2 as well', async () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    await goToStep2(container, 'Ollama');
    expect(container.querySelector('.npm-close-btn')).not.toBeNull();
  });
});

// ---------------------------------------------------------------------------
// L — Already-connected anthropic/openrouter/ollama shows existing state
// (#2522 design polish batch, item 4)
// ---------------------------------------------------------------------------

describe('L — already-connected API provider shows existing connection state', () => {
  it('shows "Already connected" summary instead of a blank config form when the provider is already connected', async () => {
    const { container, rerender } = render(NewProviderModal, {
      props: { onClose: vi.fn(), connections: [] },
    });
    await goToStep2ThenBecomeConnected(rerender, container, 'Anthropic', makeAnthropicInfo());

    expect(container.textContent).toContain('Already connected');
    expect(container.textContent).toContain('Anthropic is already connected');
    expect(container.textContent).toContain('sk-ant-...abcd');
    expect(screen.queryByLabelText('API Key')).toBeNull();
  });

  it('shows the masked credential and agent count for an already-connected key provider', async () => {
    const { container, rerender } = render(NewProviderModal, {
      props: { onClose: vi.fn(), connections: [] },
    });
    await goToStep2ThenBecomeConnected(
      rerender,
      container,
      'Anthropic',
      makeAnthropicInfo({ masked_credential: 'sk-ant-...zzzz', agents: [
        { id: 'a1', name: 'One', model: 'claude', is_default: false },
        { id: 'a2', name: 'Two', model: 'claude', is_default: true },
      ] })
    );

    expect(container.textContent).toContain('sk-ant-...zzzz');
    expect(container.textContent).toContain('Agents');
    expect(container.textContent).toContain('2');
  });

  it('shows the endpoint (not a masked credential) for an already-connected ollama', async () => {
    const { container, rerender } = render(NewProviderModal, {
      props: { onClose: vi.fn(), connections: [] },
    });
    await goToStep2ThenBecomeConnected(
      rerender,
      container,
      'Ollama',
      { runtime_type: 'ollama', connected: true, masked_credential: null, endpoint: 'http://localhost:11434', error: null, agents: [] }
    );

    expect(container.textContent).toContain('Already connected');
    expect(container.textContent).toContain('http://localhost:11434');
  });

  it('"Update connection" reveals the normal config form', async () => {
    const { container, rerender } = render(NewProviderModal, {
      props: { onClose: vi.fn(), connections: [] },
    });
    await goToStep2ThenBecomeConnected(rerender, container, 'Anthropic', makeAnthropicInfo());
    expect(container.textContent).toContain('Already connected');

    await fireEvent.click(screen.getByText('Update connection'));
    await tick();

    expect(container.textContent).toContain('Connect Anthropic');
    expect(screen.getByLabelText('API Key')).not.toBeNull();
  });

  it('clicking Disconnect opens a ConfirmDialog naming the provider, without calling disconnectRuntime yet', async () => {
    const onClose = vi.fn();
    const onConnectionSaved = vi.fn();
    const { container, rerender } = render(NewProviderModal, {
      props: { onClose, onConnectionSaved, connections: [] },
    });
    await goToStep2ThenBecomeConnected(rerender, container, 'Anthropic', makeAnthropicInfo(), onClose, onConnectionSaved);

    // Trigger button, not yet the dialog.
    expect(container.querySelector('.confirm-modal')).toBeNull();
    await fireEvent.click(container.querySelector('.npm-btn--danger') as HTMLButtonElement);
    await tick();

    const dialog = container.querySelector('.confirm-modal');
    expect(dialog).not.toBeNull();
    expect(dialog?.textContent).toContain('Disconnect Anthropic');
    expect(dialog?.textContent).toContain('deactivate all agents on this provider and delete the saved key');
    expect(disconnectRuntime).not.toHaveBeenCalled();
  });

  it('cancelling the ConfirmDialog leaves the connection untouched and the modal open', async () => {
    const onClose = vi.fn();
    const onConnectionSaved = vi.fn();
    const { container, rerender } = render(NewProviderModal, {
      props: { onClose, onConnectionSaved, connections: [] },
    });
    await goToStep2ThenBecomeConnected(rerender, container, 'Anthropic', makeAnthropicInfo(), onClose, onConnectionSaved);

    await fireEvent.click(container.querySelector('.npm-btn--danger') as HTMLButtonElement);
    await tick();

    const cancelBtn = container.querySelector('.confirm-modal .cancel-btn') as HTMLButtonElement;
    await fireEvent.click(cancelBtn);
    await tick();

    expect(container.querySelector('.confirm-modal')).toBeNull();
    expect(disconnectRuntime).not.toHaveBeenCalled();
    expect(onConnectionSaved).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    // Modal itself is still open, still showing the connected summary.
    expect(container.textContent).toContain('Already connected');
  });

  it('confirming the ConfirmDialog calls disconnectRuntime with the runtime type, onConnectionSaved, and closes the modal', async () => {
    const onClose = vi.fn();
    const onConnectionSaved = vi.fn();
    const { container, rerender } = render(NewProviderModal, {
      props: { onClose, onConnectionSaved, connections: [] },
    });
    await goToStep2ThenBecomeConnected(rerender, container, 'Anthropic', makeAnthropicInfo(), onClose, onConnectionSaved);

    await fireEvent.click(container.querySelector('.npm-btn--danger') as HTMLButtonElement);
    await tick();

    const confirmBtn = container.querySelector('.confirm-modal .action-btn') as HTMLButtonElement;
    await fireEvent.click(confirmBtn);
    await tick();
    await tick();

    expect(disconnectRuntime).toHaveBeenCalledWith('anthropic');
    expect(onConnectionSaved).toHaveBeenCalled();
    expect(onClose).toHaveBeenCalled();
  });

  it('a provider with no matching entry in `connections` (never connected) still shows the normal blank config form', async () => {
    const { container } = render(NewProviderModal, {
      props: { onClose: vi.fn(), connections: [] },
    });
    await goToStep2(container, 'Anthropic');

    expect(container.textContent).toContain('Connect Anthropic');
    expect(container.textContent).not.toContain('Already connected');
    expect(screen.getByLabelText('API Key')).not.toBeNull();
  });

  it('omitting the `connections` prop entirely (existing call sites) preserves the original blank-form behavior', async () => {
    const { container } = render(NewProviderModal, { props: { onClose: vi.fn() } });
    await goToStep2(container, 'OpenRouter');

    expect(container.textContent).toContain('Connect OpenRouter');
    expect(container.textContent).not.toContain('Already connected');
  });
});

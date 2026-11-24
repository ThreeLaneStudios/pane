/**
 * Onboarding.test.ts — Component tests for the "Choose Your Model" step
 * (Onboarding.svelte, `screen === 'model-select'`).
 *
 * #2617 regression coverage: `fetchPrimaryModels()` used to call
 * `fetch_primary_provider_models`, which derived the provider from `ai_agents`
 * rows via `derive_active_provider_label`. On a fresh install (zero `ai_agents`
 * rows, credential just entered/tested but no agent created yet) that command
 * always errored with "No primary provider configured", permanently blocking
 * onboarding — Retry re-ran the same broken call, Back was the only exit.
 *
 * The fix routes through `fetch_available_models` (credential-based,
 * agent-independent — same command `AgentCreateOverlay.svelte`'s
 * `loadModelsForRuntime()` already uses) and filters the merged,
 * provider-namespaced response down to `connectProvider` client-side. This
 * suite mirrors `AgentCreateOverlay.test.ts`'s pattern for that filter +
 * empty-after-filter behavior, plus the fresh-install regression itself.
 *
 * Test groups:
 *  A — Fresh-install regression: zero ai_agents rows, credential present → models load
 *  B — Filter: merged multi-provider response is filtered down to connectProvider
 *  C — Empty-after-filter: distinguishes provider-present-zero-models from
 *      provider-absent (gotcha 3). fetch_available_models (commands.rs) only
 *      returns Err when EVERY connected runtime's fetch fails — a single
 *      runtime's silent failure while others succeed still resolves Ok with
 *      that runtime simply missing from the merged list, which looks
 *      identical to "connected but genuinely has zero models" unless the
 *      unfiltered list is checked for the provider's presence.
 *  D — Fetch-call failure: generic error surfaced, Retry re-invokes fetch_available_models
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, fireEvent } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import Onboarding from './Onboarding.svelte';

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: invokeMock,
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

// vitest-setup.ts replaces globalThis.window with a plain mock object lacking
// addEventListener/removeEventListener/matchMedia. Patch in what
// Onboarding.svelte / its children need for jsdom rendering, same pattern
// AgentCreateOverlay.test.ts uses for the same reason.
if (typeof (globalThis.window as any).addEventListener !== 'function') {
  (globalThis.window as any).addEventListener = () => {};
}
if (typeof (globalThis.window as any).removeEventListener !== 'function') {
  (globalThis.window as any).removeEventListener = () => {};
}
if (typeof (globalThis.window as any).dispatchEvent !== 'function') {
  (globalThis.window as any).dispatchEvent = () => true;
}

/** Default mock for the invoke calls that happen before/around the
 *  'connect' → 'model-select' transition, so each test only needs to
 *  override what it actually cares about. */
function defaultInvokeImpl(overrides: Record<string, (args: any) => any> = {}) {
  return async (cmd: string, args: any = {}) => {
    if (overrides[cmd]) return overrides[cmd](args);
    switch (cmd) {
      case 'check_fresh_install':
        return false;
      case 'ai_get_default_soul':
        return '';
      case 'validate_anthropic_key':
      case 'validate_openrouter_key':
        return null;
      case 'ai_configure_and_save':
        return null;
      case 'get_agents':
        // Fresh install: zero agents — this is the precondition that used to
        // break fetch_primary_provider_models's derive_active_provider_label lookup.
        return [];
      case 'ollama_probe_two_step':
        return { error_class: null, models: [] };
      default:
        return null;
    }
  };
}

/** Drive the component from the initial 'connect' screen through to
 *  'model-select' for the given provider, using Test Connection + Continue —
 *  the same path a real user takes. Returns the rendered container. */
async function navigateToModelSelect(container: HTMLElement, provider: 'anthropic' | 'openrouter' | 'ollama' = 'anthropic') {
  await tick();
  await tick();

  const providerCard = Array.from(container.querySelectorAll('.provider-card')).find((el) =>
    el.textContent?.includes(provider === 'anthropic' ? 'Anthropic' : provider === 'openrouter' ? 'OpenRouter' : 'Ollama')
  ) as HTMLElement;
  await fireEvent.click(providerCard);
  await tick();

  if (provider !== 'ollama') {
    const keyInput = container.querySelector('input[type="password"]') as HTMLInputElement;
    await fireEvent.input(keyInput, { target: { value: 'sk-test-key' } });
    await tick();
  }

  const testBtn = Array.from(container.querySelectorAll('button')).find((b) =>
    b.textContent?.includes('Test Connection')
  ) as HTMLButtonElement;
  await fireEvent.click(testBtn);
  await tick();
  await tick();
  await new Promise((r) => setTimeout(r, 0));
  await tick();
  await tick();
  await new Promise((r) => setTimeout(r, 0));
  await tick();
  await tick();

  const continueBtn = Array.from(container.querySelectorAll('button')).find((b) =>
    b.textContent?.trim() === 'Continue' && b.classList.contains('connect-continue')
  ) as HTMLButtonElement;
  expect(continueBtn).toBeTruthy();
  await fireEvent.click(continueBtn);
  // fetchPrimaryModels() is fire-and-forget from proceedFromConnect() — allow
  // its promise chain (including a thrown/rejected invoke) to settle before
  // the caller inspects rendered state.
  await tick();
  await tick();
  await tick();
  await new Promise((r) => setTimeout(r, 0));
  await tick();
  await tick();
}

beforeEach(() => {
  invokeMock.mockReset();
});

afterEach(() => {
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// A — Fresh-install regression (#2617 core bug)
// ---------------------------------------------------------------------------

describe('A — fresh-install regression: zero ai_agents rows, credential present', () => {
  it('loads models via fetch_available_models (not fetch_primary_provider_models) and reaches Choose Your Model', async () => {
    invokeMock.mockImplementation(
      defaultInvokeImpl({
        fetch_available_models: async () => [
          { id: 'claude-opus-4-5', name: 'Claude Opus 4.5', provider: 'anthropic' },
        ],
      })
    );

    const { container } = render(Onboarding, { props: { onComplete: vi.fn() } });
    await navigateToModelSelect(container, 'anthropic');

    expect(container.textContent).toContain('Choose Your Model');
    // The broken command must never be called by the fixed code path.
    expect(invokeMock).not.toHaveBeenCalledWith('fetch_primary_provider_models', expect.anything());
    expect(invokeMock).toHaveBeenCalledWith('fetch_available_models');
    // No blocking error — the old bug always rendered "No primary provider configured" here.
    expect(container.textContent).not.toContain('No primary provider configured');
  });
});

// ---------------------------------------------------------------------------
// B — Filter: merged multi-provider response filtered to connectProvider
// ---------------------------------------------------------------------------

describe('B — filters the merged fetch_available_models response to connectProvider', () => {
  it('only shows models matching the just-connected provider (anthropic), not leftover openrouter/ollama credentials', async () => {
    invokeMock.mockImplementation(
      defaultInvokeImpl({
        fetch_available_models: async () => [
          { id: 'claude-opus-4-5', name: 'Claude Opus 4.5', provider: 'anthropic' },
          { id: 'claude-sonnet-4-5', name: 'Claude Sonnet 4.5', provider: 'anthropic' },
          { id: 'gpt-4o', name: 'GPT-4o', provider: 'openrouter' },
          { id: 'llama3', name: 'llama3', provider: 'ollama' },
        ],
      })
    );

    const { container } = render(Onboarding, { props: { onComplete: vi.fn() } });
    await navigateToModelSelect(container, 'anthropic');

    const input = container.querySelector('.model-input') as HTMLInputElement;
    expect(input).not.toBeNull();
    await fireEvent.focus(input);
    await tick();

    const labels = Array.from(container.querySelectorAll('.dropdown-item')).map((el) => el.textContent);
    expect(labels.some((t) => t?.includes('Claude Opus 4.5'))).toBe(true);
    expect(labels.some((t) => t?.includes('Claude Sonnet 4.5'))).toBe(true);
    expect(labels.some((t) => t?.includes('GPT-4o'))).toBe(false);
    expect(labels.some((t) => t?.includes('llama3'))).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// C — Empty-after-filter: distinguishes provider-present-zero-models from
// provider-absent (gotcha 3)
// ---------------------------------------------------------------------------

describe('C — empty-after-filter distinguishes provider-present-zero-models from provider-absent', () => {
  it('shows "No models available for Ollama." when the reachability re-probe confirms ollama is still connected (case 1: connected, genuinely zero models)', async () => {
    // testConnection() → probeOllama() calls ollama_probe_two_step ONCE during
    // navigation (must return error_class: null there so probeOllama() treats
    // it as success and lets the flow proceed to model-select at all — any
    // other error_class on that first call routes to an ollama-* error screen
    // instead, never reaching model-select). The gotcha-3 re-probe inside
    // fetchPrimaryModels() is the SECOND call; only it reports 'no_models'
    // here, modeling an Ollama instance that was reachable a moment ago and
    // still is, but genuinely has zero models pulled.
    let ollamaProbeCallCount = 0;
    invokeMock.mockImplementation(
      defaultInvokeImpl({
        // fetch_available_models succeeds (no error thrown) but the merged
        // response has zero ollama entries — e.g. reachable but a genuinely
        // empty Ollama daemon.
        fetch_available_models: async () => [
          { id: 'claude-opus-4-5', name: 'Claude Opus 4.5', provider: 'anthropic' },
        ],
        ollama_probe_two_step: async () => {
          ollamaProbeCallCount++;
          if (ollamaProbeCallCount === 1) return { error_class: null, models: [] }; // Test Connection step succeeds
          return { error_class: 'no_models', models: [] }; // gotcha-3 re-probe: reachable, zero models
        },
      })
    );

    const { container } = render(Onboarding, { props: { onComplete: vi.fn() } });
    await navigateToModelSelect(container, 'ollama');

    expect(container.textContent).toContain('No models available for Ollama.');
    // Must be distinguishable from the generic "call itself threw" error path —
    // no generic "Failed to fetch models" text alongside it.
    expect(container.textContent).not.toContain('Failed to fetch models');
    // Must be distinguishable from the provider-unreachable case below.
    expect(container.textContent).not.toContain('Could not reach');
  });

  it('shows a provider-unreachable message when the reachability re-probe reports ollama unreachable (case 2: this provider\'s fetch failed silently)', async () => {
    // Same call-count reasoning as the test above: the first ollama_probe_two_step
    // call (Test Connection) must succeed so navigation reaches model-select;
    // only the SECOND call (the gotcha-3 re-probe in fetchPrimaryModels()) is
    // made to report 'unreachable', modeling a connection that dropped between
    // Test Connection and the model-select screen's fetch.
    let ollamaProbeCallCount = 0;
    invokeMock.mockImplementation(
      defaultInvokeImpl({
        // fetch_available_models returns Ok (another connected runtime, anthropic,
        // succeeded) but ollama's own per-runtime fetch failed silently and
        // contributed zero entries — fetch_available_models (commands.rs) only
        // returns Err when EVERY connected runtime's fetch fails
        // (`all_models.is_empty() && !errors.is_empty()`), so a lone ollama
        // failure alongside a successful anthropic fetch is invisible as an
        // error here, and the merged ModelInfo array alone can't tell this apart
        // from case 1 (checking presence in the unfiltered list is a tautology —
        // it's the exact predicate that produced the empty post-filter result).
        fetch_available_models: async () => [
          { id: 'claude-opus-4-5', name: 'Claude Opus 4.5', provider: 'anthropic' },
        ],
        ollama_probe_two_step: async () => {
          ollamaProbeCallCount++;
          if (ollamaProbeCallCount === 1) return { error_class: null, models: [] }; // Test Connection step succeeds
          return { error_class: 'unreachable', models: [] }; // gotcha-3 re-probe: connection dropped
        },
      })
    );

    const { container } = render(Onboarding, { props: { onComplete: vi.fn() } });
    await navigateToModelSelect(container, 'ollama');

    expect(container.textContent).toContain('Could not reach Ollama. Check your connection and try again.');
    expect(container.textContent).not.toContain('No models available for Ollama.');
    expect(container.textContent).not.toContain('Failed to fetch models');
  });

  it('shows a provider-unreachable message for anthropic when the gotcha-3 re-probe (validate_anthropic_key, called AFTER Test Connection already succeeded once) now fails', async () => {
    // testConnection() already calls validate_anthropic_key once during
    // navigateToModelSelect()'s Test Connection step — that first call must
    // succeed for the flow to reach model-select at all. The gotcha-3 re-probe
    // inside fetchPrimaryModels() is the SECOND call; only it fails here,
    // modeling a connection that dropped between Test Connection and the
    // model-fetch (or a provider whose key-validate succeeds but whose
    // model-list endpoint doesn't, if such skew existed).
    let callCount = 0;
    invokeMock.mockImplementation(
      defaultInvokeImpl({
        fetch_available_models: async () => [
          { id: 'llama3', name: 'llama3', provider: 'ollama' },
        ],
        validate_anthropic_key: async () => {
          callCount++;
          if (callCount === 1) return null; // Test Connection step succeeds
          throw new Error('Network error'); // gotcha-3 re-probe fails
        },
      })
    );

    const { container } = render(Onboarding, { props: { onComplete: vi.fn() } });
    await navigateToModelSelect(container, 'anthropic');

    expect(container.textContent).toContain('Could not reach Anthropic. Check your connection and try again.');
    expect(container.textContent).not.toContain('No models available for Anthropic.');
  });

  it('shows "No models available for Anthropic." when the gotcha-3 re-probe still succeeds (case 1 for the key-based providers)', async () => {
    invokeMock.mockImplementation(
      defaultInvokeImpl({
        fetch_available_models: async () => [
          { id: 'llama3', name: 'llama3', provider: 'ollama' },
        ],
        // Both the Test Connection call and the gotcha-3 re-probe succeed (key
        // still valid) — genuinely zero anthropic models available right now,
        // not a hidden fetch failure.
        validate_anthropic_key: async () => null,
      })
    );

    const { container } = render(Onboarding, { props: { onComplete: vi.fn() } });
    await navigateToModelSelect(container, 'anthropic');

    expect(container.textContent).toContain('No models available for Anthropic.');
    expect(container.textContent).not.toContain('Could not reach');
  });
});

// ---------------------------------------------------------------------------
// D — Fetch-call failure: generic error surfaced, Retry re-invokes the fetch
// ---------------------------------------------------------------------------

describe('D — fetch_available_models call failure surfaces an error with a working Retry', () => {
  it('shows the thrown error message and Retry re-calls fetch_available_models', async () => {
    let callCount = 0;
    invokeMock.mockImplementation(
      defaultInvokeImpl({
        fetch_available_models: async () => {
          callCount++;
          if (callCount === 1) {
            throw new Error('No AI provider configured');
          }
          return [{ id: 'claude-opus-4-5', name: 'Claude Opus 4.5', provider: 'anthropic' }];
        },
      })
    );

    const { container } = render(Onboarding, { props: { onComplete: vi.fn() } });
    await navigateToModelSelect(container, 'anthropic');

    expect(container.textContent).toContain('No AI provider configured');

    const retryBtn = Array.from(container.querySelectorAll('button')).find((b) =>
      b.textContent?.trim() === 'Retry'
    ) as HTMLButtonElement;
    expect(retryBtn).toBeTruthy();
    await fireEvent.click(retryBtn);
    await tick();
    await tick();

    expect(callCount).toBe(2);
    expect(container.textContent).not.toContain('No AI provider configured');
  });
});

async function navigateToOpenClawPairing(container: HTMLElement) {
  await tick();
  await tick();
  const openClawCard = Array.from(container.querySelectorAll('.provider-card')).find((el) =>
    el.textContent?.includes('OpenClaw')
  ) as HTMLButtonElement;
  expect(openClawCard).toBeTruthy();
  await fireEvent.click(openClawCard);
  await tick();
}

describe('E — OpenClaw pairing trust boundary and sync grant', () => {
  it('shows the automatic two-way-sync grant before pairing', async () => {
    invokeMock.mockImplementation(defaultInvokeImpl());
    const { container } = render(Onboarding, { props: { onComplete: vi.fn() } });
    await navigateToOpenClawPairing(container);

    expect(container.textContent).toContain('Pairing enables automatic two-way sync');
    expect(container.textContent).toContain('identity, instructions, memory, tools, and logs');
    expect(container.textContent).toContain('may update local allowlisted agent files');
  });

  it('rejects explicit HTTP before invoking gateway_fetch_fingerprint', async () => {
    invokeMock.mockImplementation(defaultInvokeImpl());
    const { container } = render(Onboarding, { props: { onComplete: vi.fn() } });
    await navigateToOpenClawPairing(container);

    const input = container.querySelector('.connect-form input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'http://100.1.2.3:18789' } });
    const continueButton = container.querySelector('.connect-form .btn-primary') as HTMLButtonElement;
    await fireEvent.click(continueButton);
    await tick();

    expect(container.textContent).toContain('Gateway URL must use HTTPS.');
    expect(invokeMock).not.toHaveBeenCalledWith('gateway_fetch_fingerprint', expect.anything());
  });

  it('ignores malicious gateway_hostname and uses one reviewed origin for fetch, exchange, persistence, and sync', async () => {
    invokeMock.mockImplementation(defaultInvokeImpl({
      gateway_fetch_fingerprint: async () => ({
        tls_fingerprint: 'AA:BB:CC:DD',
        cert_pem: '-----CERT-----',
        gateway_hostname: 'attacker.example',
      }),
      gateway_exchange_pairing_code: async () => ({
        token: 'tok-reviewed',
        gateway_version: '0.3.0',
        version_warning: null,
      }),
      oc_agent_row_exists: async () => false,
      sync_initial_import: async () => ({ agent_names: [], agents_missing_model: [] }),
      get_main_agent: async () => ({ id: 'agent-main', name: 'Main' }),
    }));
    const { container } = render(Onboarding, { props: { onComplete: vi.fn() } });
    await navigateToOpenClawPairing(container);

    const input = container.querySelector('.connect-form input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'https://100.1.2.3:18789' } });
    await fireEvent.click(container.querySelector('.connect-form .btn-primary') as HTMLButtonElement);
    await vi.waitFor(() => expect(container.textContent).toContain('Verify & Connect'));

    const codeInput = container.querySelector('[data-testid="oc-pairing-code-input"]') as HTMLInputElement;
    await fireEvent.input(codeInput, { target: { value: '123456' } });
    const connectButton = Array.from(container.querySelectorAll('button')).find((button) =>
      button.textContent?.trim() === 'Connect'
    ) as HTMLButtonElement;
    await fireEvent.click(connectButton);

    const reviewedUrl = 'https://100.1.2.3:18789/v1';
    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledWith('sync_initial_import', {
      gatewayUrl: reviewedUrl,
      token: 'tok-reviewed',
    }));
    expect(invokeMock).toHaveBeenCalledWith('gateway_fetch_fingerprint', { url: reviewedUrl });
    expect(invokeMock).toHaveBeenCalledWith('gateway_exchange_pairing_code', {
      url: reviewedUrl,
      code: '123456',
    });
    expect(invokeMock).toHaveBeenCalledWith('ai_configure_and_save', expect.objectContaining({
      provider: 'openclaw',
      endpoint: reviewedUrl,
    }));
    expect(JSON.stringify(invokeMock.mock.calls)).not.toContain('attacker.example');
  });
});

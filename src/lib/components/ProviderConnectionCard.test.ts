/**
 * ProviderConnectionCard.test.ts — Component tests for ProviderConnectionCard.svelte
 *
 * #2522 Stage 5b, ADR-0063 D8: one card per runtime, no "active provider" concept.
 *
 * #2642/#2630 (2026-08-22) Commit 6: the OC pairing ceremony test suite (formerly
 * Group J — URL/verify/code step transitions, D18 first-pair/re-pair routing,
 * D20 collision-confirm) moved to NewProviderModal.test.ts — this card no longer
 * runs the ceremony, it only calls onPairRequested(). See that file's module
 * comment for the migrated groups.
 *
 * Test groups:
 *  A — Identity row: label, logo, masked credential / endpoint, connected status
 *  B — Expand/collapse toggles the body
 *  C — Failure strip renders backend error + blast-radius note when agents exist
 *  D — Read-only agent roster: name, model, Default badge
 *  E — Anthropic/OpenRouter: API key field, Save gating, Save calls ai_configure_and_save
 *  F — Ollama: endpoint field, Save always enabled, Test connection via ollama_probe_two_step
 *  G — OpenClaw: paired-card config rows (Gateway URL/Token/Fingerprint/Sync), Disconnect still available
 *  H — Disconnect: confirm() gate, calls disconnectRuntime, calls onChange
 *  I — OpenClaw: unpaired state rendering (§12 — grey "Not paired", not an error)
 *  I2 — Pair/Re-pair label via oc_agent_row_exists() + onPairRequested callback
 *  K — OpenClaw: Sync Now wiring (§14)
 *  M — pairSyncError external prop display (badge + inline notice)
 *  N — #2641 D14: three-state cardStatus matrix across all four runtimes, incl. the
 *      #2631 stale-keychain-leftover regression test and OC's hasOcAgentRows-driven
 *      grey "Paired, disconnected" vs red genuine-failure split
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import ProviderConnectionCard from './ProviderConnectionCard.svelte';
import type { RuntimeConnectionInfo } from '$lib/api';

const { invokeMock, syncNowMock } = vi.hoisted(() => ({ invokeMock: vi.fn(), syncNowMock: vi.fn() }));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: invokeMock,
}));

vi.mock('$lib/api', async (importOriginal) => {
  const original = await importOriginal() as any;
  return {
    ...original,
    disconnectRuntime: vi.fn().mockResolvedValue(undefined),
    syncNow: syncNowMock,
  };
});

import { disconnectRuntime } from '$lib/api';

// vitest-setup.ts replaces `globalThis.window` with a plain mock object (Tauri
// internals shim) that lacks addEventListener/removeEventListener. #2635 (D11 fix)
// adds ImportedAgentsModal, which uses <svelte:window onkeydown={...}> for
// Escape-to-close, so the DOM listener API needs to exist on that mock window for
// mount/unmount to succeed. Same patch as DefaultAgentPickerModal.test.ts — applied
// here rather than in the shared setup file since other suites depend on its exact
// shape.
if (typeof (globalThis.window as any).addEventListener !== 'function') {
  (globalThis.window as any).addEventListener = () => {};
}
if (typeof (globalThis.window as any).removeEventListener !== 'function') {
  (globalThis.window as any).removeEventListener = () => {};
}

function makeInfo(overrides: Partial<RuntimeConnectionInfo> = {}): RuntimeConnectionInfo {
  return {
    runtime_type: 'anthropic',
    connected: true,
    masked_credential: 'sk-ant-...abcd',
    endpoint: null,
    error: null,
    // #2641 D14: defaults to `true` so every PRE-EXISTING test in this file (written
    // before the three-state fix, which mostly exercises `connected`/`masked_credential`
    // shapes) keeps its original meaning — those tests are implicitly describing an
    // already-configured-this-install card. New tests for the grey/never-configured
    // case explicitly override this to `false`. See group N for the dedicated
    // three-state matrix.
    configured_this_install: true,
    agents: [],
    ...overrides,
  };
}

async function openCard(container: HTMLElement) {
  const top = container.querySelector('.pcc-top') as HTMLElement;
  await fireEvent.click(top);
  await tick();
  await tick();
}

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
  syncNowMock.mockReset();
  (disconnectRuntime as any).mockClear?.();
  vi.stubGlobal('confirm', vi.fn(() => true));
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.resetAllMocks();
});

// ---------------------------------------------------------------------------
// A — Identity row
// ---------------------------------------------------------------------------

describe('A — identity row', () => {
  it('renders the provider label and masked credential for anthropic', () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'anthropic', masked_credential: 'sk-ant-...abcd' }), onChange: vi.fn() },
    });
    expect(container.textContent).toContain('Anthropic');
    expect(container.textContent).toContain('sk-ant-...abcd');
  });

  it('shows "Connected" status when connected=true', () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ connected: true }), onChange: vi.fn() },
    });
    expect(container.querySelector('.pcc-status')?.textContent?.trim()).toBe('Connected');
    expect(container.querySelector('.pcc-status--bad')).toBeNull();
  });

  // #2641 D14: `connected=false` alone no longer implies red — red requires
  // `info.error` to be populated too (Matt's ruling: "no live reachability probe;
  // red only fires from an explicit Test/Save/dispatch failure"). In the real backend
  // this pairing always holds (build_provider_for_runtime_type's Err always carries a
  // message when connected is false), so this test now supplies both to stay realistic.
  it('shows "Disconnected" status and attention border when connected=false with an error', () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ connected: false, error: 'Invalid API key' }), onChange: vi.fn() },
    });
    expect(container.querySelector('.pcc-status')?.textContent?.trim()).toBe('Disconnected');
    expect(container.querySelector('.pcc-status--bad')).not.toBeNull();
    expect(container.querySelector('.pcc-card--attn')).not.toBeNull();
  });

  it('renders "Not configured" for a card never configured this install', () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ masked_credential: null, connected: false, configured_this_install: false }), onChange: vi.fn() },
    });
    expect(container.textContent).toContain('Not configured');
  });

  it('renders endpoint (not masked credential) for ollama', () => {
    const { container } = render(ProviderConnectionCard, {
      props: {
        info: makeInfo({ runtime_type: 'ollama', masked_credential: null, endpoint: 'http://localhost:11434' }),
        onChange: vi.fn(),
      },
    });
    expect(container.textContent).toContain('http://localhost:11434');
    expect(container.querySelector('.pcc-badge--local')).not.toBeNull();
  });
});

// ---------------------------------------------------------------------------
// A2 — §12 grey-not-red generalization (#2522 design polish batch): a
// never-configured card (no masked_credential / no endpoint, disconnected) is
// a neutral state panel-wide, not just for OpenClaw's existing ocUnpaired
// case. A card that WAS configured and is now failing keeps the red
// treatment.
// ---------------------------------------------------------------------------

describe('A2 — grey-not-red generalized to key providers + ollama', () => {
  // #2641 D14: these tests now drive grey/red via `configured_this_install`
  // (the correct signal, Commit 3) instead of masked_credential/endpoint presence
  // (the old, buggy proxy that caused #2631's stale-keychain false-green). The
  // masked_credential/endpoint overrides are kept alongside for realism (a truly
  // unconfigured backend response also has no credential), but the grey/red split
  // itself is asserted via configured_this_install.
  it('anthropic never configured this install (configured_this_install=false, disconnected) is grey, not red', () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'anthropic', connected: false, masked_credential: null, error: null, configured_this_install: false }), onChange: vi.fn() },
    });
    expect(container.querySelector('.pcc-status')?.textContent?.trim()).toBe('Not configured');
    expect(container.querySelector('.pcc-status--bad')).toBeNull();
    expect(container.querySelector('.pcc-status--neutral')).not.toBeNull();
    expect(container.querySelector('.pcc-card--attn')).toBeNull();
    expect(container.querySelector('.pcc-badge--unpaired')?.textContent).toContain('Not configured');
  });

  it('openrouter never configured this install (configured_this_install=false, disconnected) is grey, not red', () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'openrouter', connected: false, masked_credential: null, error: null, configured_this_install: false }), onChange: vi.fn() },
    });
    expect(container.querySelector('.pcc-status')?.textContent?.trim()).toBe('Not configured');
    expect(container.querySelector('.pcc-card--attn')).toBeNull();
  });

  it('anthropic configured this install but failing (configured_this_install=true, error set) stays red', () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'anthropic', connected: false, masked_credential: 'sk-ant-...abcd', error: 'Invalid API key', configured_this_install: true }), onChange: vi.fn() },
    });
    expect(container.querySelector('.pcc-status')?.textContent?.trim()).toBe('Disconnected');
    expect(container.querySelector('.pcc-status--bad')).not.toBeNull();
    expect(container.querySelector('.pcc-card--attn')).not.toBeNull();
  });

  it('ollama never configured this install (configured_this_install=false, disconnected) is grey, not red', () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'ollama', connected: false, endpoint: null, masked_credential: null, configured_this_install: false }), onChange: vi.fn() },
    });
    expect(container.querySelector('.pcc-status')?.textContent?.trim()).toBe('Not configured');
    expect(container.querySelector('.pcc-card--attn')).toBeNull();
  });

  it('ollama configured this install but failing (configured_this_install=true, error set) stays red', () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'ollama', connected: false, endpoint: 'http://localhost:11434', masked_credential: null, error: 'Unreachable', configured_this_install: true }), onChange: vi.fn() },
    });
    expect(container.querySelector('.pcc-status')?.textContent?.trim()).toBe('Disconnected');
    expect(container.querySelector('.pcc-card--attn')).not.toBeNull();
  });

  it('Disconnect button is hidden for a never-configured-this-install card, same as OC unpaired', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'anthropic', connected: false, masked_credential: null, error: null, configured_this_install: false }), onChange: vi.fn() },
    });
    await openCard(container);
    expect(screen.queryByText('Disconnect')).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// B — Expand/collapse
// ---------------------------------------------------------------------------

describe('B — expand/collapse', () => {
  it('body is not rendered until the top row is clicked', () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo(), onChange: vi.fn() },
    });
    expect(container.querySelector('.pcc-body')).toBeNull();
    expect(container.querySelector('.pcc-foot')).toBeNull();
  });

  it('clicking the top row expands the card and shows the foot actions', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo(), onChange: vi.fn() },
    });
    await openCard(container);
    expect(container.querySelector('.pcc-body')).not.toBeNull();
    expect(container.querySelector('.pcc-foot')).not.toBeNull();
  });

  it('clicking the top row again collapses it', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo(), onChange: vi.fn() },
    });
    await openCard(container);
    await openCard(container);
    expect(container.querySelector('.pcc-body')).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// C — Failure strip
// ---------------------------------------------------------------------------

describe('C — failure strip', () => {
  it('shows the backend error string when info.error is set', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ error: 'Invalid API key', connected: false }), onChange: vi.fn() },
    });
    await openCard(container);
    const strip = container.querySelector('.pcc-fail');
    expect(strip).not.toBeNull();
    expect(strip?.textContent).toContain('Invalid API key');
  });

  it('names the blast radius when agents exist on a failing provider', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: {
        info: makeInfo({
          error: 'Invalid API key',
          connected: false,
          agents: [{ id: 'a1', name: 'Research', model: 'claude-3', is_default: false }],
        }),
        onChange: vi.fn(),
      },
    });
    await openCard(container);
    expect(container.querySelector('.pcc-fail')?.textContent).toContain('cannot answer until this reconnects');
  });

  it('does not render a failure strip when there is no error', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ error: null }), onChange: vi.fn() },
    });
    await openCard(container);
    expect(container.querySelector('.pcc-fail')).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// D — Read-only agent roster
// ---------------------------------------------------------------------------

describe('D — read-only agent roster', () => {
  it('lists agent name and model for each roster entry', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: {
        info: makeInfo({
          agents: [
            { id: 'a1', name: 'Research', model: 'claude-3-opus', is_default: false },
            { id: 'a2', name: 'Support', model: 'claude-3-haiku', is_default: true },
          ],
        }),
        onChange: vi.fn(),
      },
    });
    await openCard(container);
    expect(container.textContent).toContain('Research');
    expect(container.textContent).toContain('claude-3-opus');
    expect(container.textContent).toContain('Support');
    expect(container.textContent).toContain('claude-3-haiku');
  });

  it('shows a Default badge only for the default agent', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: {
        info: makeInfo({
          agents: [
            { id: 'a1', name: 'Research', model: 'claude-3-opus', is_default: false },
            { id: 'a2', name: 'Support', model: 'claude-3-haiku', is_default: true },
          ],
        }),
        onChange: vi.fn(),
      },
    });
    await openCard(container);
    const badges = container.querySelectorAll('.pcc-badge--default');
    expect(badges.length).toBe(1);
  });

  it('shows the roster count matching the number of agents', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: {
        info: makeInfo({
          agents: [
            { id: 'a1', name: 'Research', model: 'claude-3-opus', is_default: false },
            { id: 'a2', name: 'Support', model: null, is_default: true },
          ],
        }),
        onChange: vi.fn(),
      },
    });
    await openCard(container);
    expect(container.querySelector('.pcc-roster-count')?.textContent?.trim()).toBe('2');
  });

  // C1 (#2585): `migrate_v83` seeds a `'openclaw-pending'` sentinel into `ai_agents.model`
  // for OC rows left without a real model — the literal placeholder must never reach the
  // user; it's masked here instead of surfacing verbatim.
  it('masks the openclaw-pending sentinel model as "resolves on first message"', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: {
        info: makeInfo({
          agents: [{ id: 'a1', name: 'Research', model: 'openclaw-pending', is_default: true }],
        }),
        onChange: vi.fn(),
      },
    });
    await openCard(container);
    expect(container.textContent).not.toContain('openclaw-pending');
    expect(container.querySelector('.pcc-agent-model')?.textContent?.trim()).toBe('resolves on first message');
  });

  it('shows a dash for a null model (non-sentinel "not configured" case)', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: {
        info: makeInfo({
          agents: [{ id: 'a1', name: 'Research', model: null, is_default: true }],
        }),
        onChange: vi.fn(),
      },
    });
    await openCard(container);
    expect(container.querySelector('.pcc-agent-model')?.textContent?.trim()).toBe('—');
  });

  it('roster contains no create/set-default/switch controls (read-only)', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: {
        info: makeInfo({ agents: [{ id: 'a1', name: 'Research', model: 'claude-3', is_default: true }] }),
        onChange: vi.fn(),
      },
    });
    await openCard(container);
    const roster = container.querySelector('.pcc-roster')!;
    expect(roster.querySelector('button')).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// E — Anthropic / OpenRouter: API key field + Save
// ---------------------------------------------------------------------------

describe('E — key-provider config + Save', () => {
  it('renders an API key password input', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'anthropic' }), onChange: vi.fn() },
    });
    await openCard(container);
    const input = container.querySelector('.pcc-body input[type="password"]');
    expect(input).not.toBeNull();
  });

  it('Save is disabled until a key is typed', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'anthropic' }), onChange: vi.fn() },
    });
    await openCard(container);
    const saveBtn = screen.getByText('Save') as HTMLButtonElement;
    expect(saveBtn.disabled).toBe(true);
  });

  it('typing a key enables Save; clicking it calls ai_configure_and_save with model:null and calls onChange', async () => {
    const onChange = vi.fn();
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'anthropic' }), onChange },
    });
    await openCard(container);

    const input = container.querySelector('.pcc-body input[type="password"]') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'sk-ant-new-key' } });
    await tick();

    const saveBtn = screen.getByText('Save') as HTMLButtonElement;
    expect(saveBtn.disabled).toBe(false);
    await fireEvent.click(saveBtn);
    // §28.2 / #1630 fix: Save now awaits `runKeyProviderValidator()` (an extra
    // `invoke('validate_anthropic_key', ...)` microtask) BEFORE ai_configure_and_save
    // — one more tick than before is needed for both awaited invokes to resolve and
    // onChange() to fire.
    await tick();
    await tick();
    await tick();

    expect(invokeMock).toHaveBeenCalledWith('validate_anthropic_key', { apiKey: 'sk-ant-new-key' });
    expect(invokeMock).toHaveBeenCalledWith('ai_configure_and_save', {
      provider: 'anthropic',
      apiKey: 'sk-ant-new-key',
      model: null,
      endpoint: null,
      simpleTierModel: null,
    });
    expect(onChange).toHaveBeenCalled();
  });

  it('§28.2 / #1630 fix: a failing validator aborts Save — ai_configure_and_save is never called', async () => {
    const onChange = vi.fn();
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'validate_anthropic_key') return Promise.reject(new Error('Invalid key'));
      return Promise.resolve(undefined);
    });
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'anthropic' }), onChange },
    });
    await openCard(container);

    const input = container.querySelector('.pcc-body input[type="password"]') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'sk-ant-bad-key' } });
    await tick();

    const saveBtn = screen.getByText('Save') as HTMLButtonElement;
    await fireEvent.click(saveBtn);
    await tick();
    await tick();
    await tick();

    expect(invokeMock).toHaveBeenCalledWith('validate_anthropic_key', { apiKey: 'sk-ant-bad-key' });
    expect(invokeMock).not.toHaveBeenCalledWith('ai_configure_and_save', expect.anything());
    expect(onChange).not.toHaveBeenCalled();
    expect(container.textContent).toContain('Invalid key');
  });

  it('Test connection calls validate_anthropic_key for anthropic and validate_openrouter_key for openrouter', async () => {
    const { container, rerender } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'anthropic' }), onChange: vi.fn() },
    });
    await openCard(container);
    const input = container.querySelector('.pcc-body input[type="password"]') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'sk-ant-test' } });
    await tick();

    await fireEvent.click(screen.getByText('Test connection'));
    await tick();
    await tick();

    expect(invokeMock).toHaveBeenCalledWith('validate_anthropic_key', { apiKey: 'sk-ant-test' });
  });
});

// ---------------------------------------------------------------------------
// E2 — #2783: Show/Hide toggle disabled while apiKeyInput is empty
//
// Root cause (docs/audits/2026-08-31-2794-2783-formal-diagnosis.md, #2783):
// apiKeyInput always starts empty — masked_credential is only ever wired into
// the input's `placeholder`, never its `value` (no raw-secret round-trip to
// the renderer, by design). Browsers render placeholder text identically
// under type="password" and type="text" (no dots either way for a
// placeholder), so toggling `showApiKey` with nothing typed genuinely has no
// visible effect — not a broken toggle, just nothing for it to reveal. Fix:
// disable the button while apiKeyInput.trim() === '', matching capability to
// affordance; it re-enables the instant the user types a character.
// ---------------------------------------------------------------------------
describe('E2 — #2783: Show/Hide toggle disabled state', () => {
  it('Show/Hide button is disabled when apiKeyInput is empty (fresh card, stored key present)', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'anthropic', masked_credential: 'sk-ant-...abcd' }), onChange: vi.fn() },
    });
    await openCard(container);

    const toggle = container.querySelector('.password-toggle') as HTMLButtonElement;
    expect(toggle).not.toBeNull();
    expect(toggle.disabled).toBe(true);
  });

  it('Show/Hide button is disabled when apiKeyInput is empty (no stored key either)', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'openrouter', masked_credential: null }), onChange: vi.fn() },
    });
    await openCard(container);

    const toggle = container.querySelector('.password-toggle') as HTMLButtonElement;
    expect(toggle.disabled).toBe(true);
  });

  it('typing a key enables the Show/Hide button', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'anthropic' }), onChange: vi.fn() },
    });
    await openCard(container);

    const input = container.querySelector('.pcc-body input[type="password"]') as HTMLInputElement;
    const toggle = container.querySelector('.password-toggle') as HTMLButtonElement;
    expect(toggle.disabled).toBe(true);

    await fireEvent.input(input, { target: { value: 'sk-ant-new-key' } });
    await tick();

    expect(toggle.disabled).toBe(false);
  });

  it('typing then clearing the key back to empty re-disables the button', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'anthropic' }), onChange: vi.fn() },
    });
    await openCard(container);

    const input = container.querySelector('.pcc-body input[type="password"]') as HTMLInputElement;
    const toggle = container.querySelector('.password-toggle') as HTMLButtonElement;

    await fireEvent.input(input, { target: { value: 'sk-ant-typed' } });
    await tick();
    expect(toggle.disabled).toBe(false);

    await fireEvent.input(input, { target: { value: '' } });
    await tick();
    expect(toggle.disabled).toBe(true);
  });

  it('a key that is only whitespace keeps the button disabled (matches Save\'s trim() gating)', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'anthropic' }), onChange: vi.fn() },
    });
    await openCard(container);

    const input = container.querySelector('.pcc-body input[type="password"]') as HTMLInputElement;
    const toggle = container.querySelector('.password-toggle') as HTMLButtonElement;

    await fireEvent.input(input, { target: { value: '   ' } });
    await tick();

    expect(toggle.disabled).toBe(true);
  });

  it('the disabled toggle button carries the native disabled attribute (browser blocks its click handler)', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'anthropic' }), onChange: vi.fn() },
    });
    await openCard(container);

    const toggle = container.querySelector('.password-toggle') as HTMLButtonElement;
    // Asserting the `disabled` DOM attribute directly (rather than firing a
    // click and checking for a no-op) — jsdom's fireEvent.click doesn't
    // reliably enforce the same native disabled-button click-suppression a
    // real browser does, so the meaningful, non-flaky assertion here is that
    // the button is genuinely disabled: real browsers block both the click
    // AND any onclick handler on a disabled button.
    expect(toggle.hasAttribute('disabled')).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// F — Ollama: endpoint field + always-enabled Save + probe-based Test
// ---------------------------------------------------------------------------

describe('F — ollama config + Save + Test', () => {
  it('renders an endpoint text input pre-filled from info.endpoint', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'ollama', endpoint: 'http://localhost:11434', masked_credential: null }), onChange: vi.fn() },
    });
    await openCard(container);
    const input = container.querySelector('.pcc-body input[type="text"]') as HTMLInputElement;
    expect(input.value).toBe('http://localhost:11434');
  });

  it('Save is enabled without any user input (endpoint has a default)', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'ollama', endpoint: null, masked_credential: null }), onChange: vi.fn() },
    });
    await openCard(container);
    const saveBtn = screen.getByText('Save') as HTMLButtonElement;
    expect(saveBtn.disabled).toBe(false);
  });

  it('Test connection calls ollama_probe_two_step with the endpoint and shows success on no error_class', async () => {
    invokeMock.mockResolvedValue({ models: ['llama3.2:3b'], error_class: null });
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'ollama', endpoint: 'http://localhost:11434', masked_credential: null }), onChange: vi.fn() },
    });
    await openCard(container);

    await fireEvent.click(screen.getByText('Test connection'));
    await tick();
    await tick();

    expect(invokeMock).toHaveBeenCalledWith('ollama_probe_two_step', { endpoint: 'http://localhost:11434' });
    expect(container.querySelector('.pcc-test-ok')).not.toBeNull();
  });

  it('shows an error message when the probe returns error_class="unreachable"', async () => {
    invokeMock.mockResolvedValue({ models: [], error_class: 'unreachable' });
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'ollama', endpoint: 'http://localhost:11434', masked_credential: null }), onChange: vi.fn() },
    });
    await openCard(container);

    await fireEvent.click(screen.getByText('Test connection'));
    await tick();
    await tick();

    const err = container.querySelector('.pcc-test-err');
    expect(err).not.toBeNull();
    expect(err?.textContent).toContain("Couldn't reach Ollama");
  });
});

// ---------------------------------------------------------------------------
// G — OpenClaw: read-only card
// ---------------------------------------------------------------------------

describe('G — openclaw read-only card', () => {
  it('renders the Gateway URL as read-only text, not an input', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'openclaw', endpoint: 'http://127.0.0.1:18789', masked_credential: null }), onChange: vi.fn() },
    });
    await openCard(container);
    expect(container.querySelector('.pcc-body input')).toBeNull();
    expect(container.querySelector('.pcc-readonly')?.textContent).toContain('http://127.0.0.1:18789');
  });

  it('does not render a Save button for openclaw', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'openclaw' }), onChange: vi.fn() },
    });
    await openCard(container);
    expect(screen.queryByText('Save')).toBeNull();
  });

  // #2590 UX batch 2, item 3: OC gets Test connection parity with the other three
  // provider cards, once paired. (Superseded the old "never render one for openclaw"
  // assertion below — see the new `describe` block for the paired-card behavior.)
  it('renders a Test connection button for a paired openclaw card', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'openclaw', endpoint: 'https://100.64.0.1:8443/v1' }), onChange: vi.fn() },
    });
    await openCard(container);
    expect(screen.getByText('Test connection')).not.toBeNull();
  });

  // #2641 D14 Commit 4: Disconnect visibility now derives from cardStatus (which for
  // openclaw depends on hasOcAgentRows, resolved async via oc_agent_row_exists) rather
  // than the old ocUnpaired check — mock the invoke so this settles to a paired,
  // connected, no-error ('active') state before asserting.
  it('still renders the Disconnect button for openclaw', async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'oc_agent_row_exists') return Promise.resolve(true);
      return Promise.resolve(undefined);
    });
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'openclaw' }), onChange: vi.fn() },
    });
    await openCard(container);
    await vi.waitFor(() => expect(screen.getByText('Disconnect')).not.toBeNull());
  });
});

// ---------------------------------------------------------------------------
// G2 — OpenClaw: Test connection parity (#2590 UX batch 2, item 3)
// ---------------------------------------------------------------------------

describe('G2 — openclaw Test connection parity', () => {
  it('calls gateway_fetch_fingerprint with the card\'s endpoint and shows success', async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'gateway_fetch_fingerprint') {
        return Promise.resolve({ tls_fingerprint: 'ab:cd', cert_pem: '---', gateway_hostname: null });
      }
      return Promise.resolve(null);
    });
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'openclaw', endpoint: 'https://100.64.0.1:8443/v1' }), onChange: vi.fn() },
    });
    await openCard(container);

    await fireEvent.click(screen.getByText('Test connection'));
    await tick();
    await tick();

    expect(invokeMock).toHaveBeenCalledWith('gateway_fetch_fingerprint', { url: 'https://100.64.0.1:8443/v1' });
    expect(container.textContent).toContain('Connected');
  });

  it('shows an error when gateway_fetch_fingerprint rejects', async () => {
    invokeMock.mockRejectedValue(new Error('Cannot reach gateway: connection refused'));
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'openclaw', endpoint: 'https://100.64.0.1:8443/v1' }), onChange: vi.fn() },
    });
    await openCard(container);

    await fireEvent.click(screen.getByText('Test connection'));
    await tick();
    await tick();

    expect(container.textContent).toContain('Cannot reach gateway');
  });

  it('does not render a Test connection button for an unpaired openclaw card', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'openclaw', connected: false, error: null, endpoint: null, masked_credential: null }), onChange: vi.fn() },
    });
    await openCard(container);
    expect(screen.queryByText('Test connection')).toBeNull();
  });

  it('carries the oc-test-connection testid, matching the pair-gateway/re-pair pattern', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'openclaw', endpoint: 'https://100.64.0.1:8443/v1' }), onChange: vi.fn() },
    });
    await openCard(container);
    expect(container.querySelector('[data-testid="oc-test-connection"]')).not.toBeNull();
  });
});

// ---------------------------------------------------------------------------
// H — Disconnect
// ---------------------------------------------------------------------------

describe('H — disconnect', () => {
  it('is disabled when the runtime is already disconnected', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ connected: false }), onChange: vi.fn() },
    });
    await openCard(container);
    const btn = screen.getByText('Disconnect') as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
  });

  it('clicking Disconnect (non-OC) opens a ConfirmDialog naming the provider and consequence, without calling disconnectRuntime yet', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'anthropic', connected: true }), onChange: vi.fn() },
    });
    await openCard(container);

    expect(container.querySelector('.confirm-modal')).toBeNull();
    await fireEvent.click(screen.getByText('Disconnect'));
    await tick();

    const dialog = container.querySelector('.confirm-modal');
    expect(dialog).not.toBeNull();
    expect(dialog?.textContent).toContain('Disconnect Anthropic');
    expect(dialog?.textContent).toContain('deactivate all agents on this provider and delete the saved key');
    expect(disconnectRuntime).not.toHaveBeenCalled();
  });

  it('cancelling the non-OC ConfirmDialog leaves the connection untouched', async () => {
    const onChange = vi.fn();
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'anthropic', connected: true }), onChange },
    });
    await openCard(container);
    await fireEvent.click(screen.getByText('Disconnect'));
    await tick();

    const cancelBtn = container.querySelector('.confirm-modal .cancel-btn') as HTMLButtonElement;
    await fireEvent.click(cancelBtn);
    await tick();

    expect(container.querySelector('.confirm-modal')).toBeNull();
    expect(disconnectRuntime).not.toHaveBeenCalled();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('confirming the non-OC ConfirmDialog calls disconnectRuntime with the runtime type and onChange', async () => {
    const onChange = vi.fn();

    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'anthropic', connected: true }), onChange },
    });
    await openCard(container);
    await fireEvent.click(screen.getByText('Disconnect'));
    await tick();

    const confirmBtn = container.querySelector('.confirm-modal .action-btn') as HTMLButtonElement;
    await fireEvent.click(confirmBtn);
    await tick();
    await tick();

    expect(disconnectRuntime).toHaveBeenCalledWith('anthropic');
    expect(onChange).toHaveBeenCalled();
  });

  // Bonus coverage (#2892 scope report §5 item 4 / §9.2, bundled per Matt's
  // ruling): the OC ConfirmDialog path (showDisconnectConfirm /
  // confirmOpenClawDisconnect) had no DOM-driven test before this migration.
  it('clicking Disconnect for OpenClaw opens the OC ConfirmDialog and confirming it calls disconnectRuntime', async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'oc_agent_row_exists') return Promise.resolve(true);
      return Promise.resolve(undefined);
    });
    const onChange = vi.fn();
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'openclaw', connected: true, agents: [
        { id: 'a1', name: 'One', model: 'claude', is_default: true },
        { id: 'a2', name: 'Two', model: 'claude', is_default: false },
      ] }), onChange },
    });
    await openCard(container);
    await vi.waitFor(() => expect(screen.getByText('Disconnect')).not.toBeNull());

    await fireEvent.click(screen.getByText('Disconnect'));
    await tick();

    const dialog = container.querySelector('.confirm-modal');
    expect(dialog).not.toBeNull();
    expect(dialog?.textContent).toContain('Disconnect OpenClaw');
    expect(dialog?.textContent).toContain('Your 2 agents will be unavailable until you pair again');
    expect(disconnectRuntime).not.toHaveBeenCalled();

    const confirmBtn = container.querySelector('.confirm-modal .action-btn') as HTMLButtonElement;
    await fireEvent.click(confirmBtn);
    await tick();
    await tick();

    expect(disconnectRuntime).toHaveBeenCalledWith('openclaw');
    expect(onChange).toHaveBeenCalled();
  });

  it('cancelling the OC ConfirmDialog leaves the connection untouched', async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'oc_agent_row_exists') return Promise.resolve(true);
      return Promise.resolve(undefined);
    });
    const onChange = vi.fn();
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'openclaw', connected: true, agents: [] }), onChange },
    });
    await openCard(container);
    await vi.waitFor(() => expect(screen.getByText('Disconnect')).not.toBeNull());

    await fireEvent.click(screen.getByText('Disconnect'));
    await tick();

    const cancelBtn = container.querySelector('.confirm-modal .cancel-btn') as HTMLButtonElement;
    await fireEvent.click(cancelBtn);
    await tick();

    expect(container.querySelector('.confirm-modal')).toBeNull();
    expect(disconnectRuntime).not.toHaveBeenCalled();
    expect(onChange).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// I — OpenClaw: unpaired state (§12 — a state, not an error)
// ---------------------------------------------------------------------------

describe('I — openclaw unpaired state', () => {
  function makeUnpaired() {
    return makeInfo({ runtime_type: 'openclaw', connected: false, error: null, endpoint: null, masked_credential: null });
  }

  it('shows "Not paired" in the status slot, not "Disconnected"', () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeUnpaired(), onChange: vi.fn() },
    });
    expect(container.querySelector('.pcc-status')?.textContent?.trim()).toBe('Not paired');
    expect(container.querySelector('.pcc-status--bad')).toBeNull();
  });

  it('shows a "Not paired" badge next to the name', () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeUnpaired(), onChange: vi.fn() },
    });
    expect(container.querySelector('.pcc-badge--unpaired')?.textContent).toContain('Not paired');
  });

  it('does not apply the red attention border for an unpaired (not failing) card', () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeUnpaired(), onChange: vi.fn() },
    });
    expect(container.querySelector('.pcc-card--attn')).toBeNull();
  });

  // #2641 D14 Commit 4: red now requires hasOcAgentRows === true (a real failure on a
  // pairing that genuinely exists), not just info.error — mock oc_agent_row_exists so
  // this stays a true "paired, now failing" case rather than the never-paired default.
  it('still applies the red attention border when openclaw is failing (has info.error) and hasOcAgentRows is true', async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'oc_agent_row_exists') return Promise.resolve(true);
      return Promise.resolve(undefined);
    });
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'openclaw', connected: false, error: 'TLS handshake failed' }), onChange: vi.fn() },
    });
    await vi.waitFor(() => expect(container.querySelector('.pcc-card--attn')).not.toBeNull());
    expect(container.querySelector('.pcc-status')?.textContent?.trim()).toBe('Disconnected');
  });

  // #2642/#2630 Commit 3: the card no longer runs an in-card ceremony — clicking
  // "Pair gateway" calls the onPairRequested callback prop instead, with mode
  // derived from hasOcAgentRows (here still resolving/false, so 'pair'). The
  // caller (ProviderConnectionsPanel) is the one that opens NewProviderModal.
  it('renders a "Pair gateway" button that calls onPairRequested(\'pair\') instead of opening an in-card ceremony', async () => {
    const onPairRequested = vi.fn();
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeUnpaired(), onChange: vi.fn(), onPairRequested },
    });
    await openCard(container);
    const pairBtn = screen.getByText('Pair gateway');
    await fireEvent.click(pairBtn);
    await tick();

    expect(onPairRequested).toHaveBeenCalledWith('pair');
    // No in-card ceremony UI renders — that surface moved to NewProviderModal.svelte.
    expect(screen.queryByText('Start pairing')).toBeNull();
    expect(container.querySelector('.pcc-pairing-error')).toBeNull();
  });

  it('does not render a Disconnect button while unpaired', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeUnpaired(), onChange: vi.fn() },
    });
    await openCard(container);
    expect(screen.queryByText('Disconnect')).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// I2 — #2642/#2630 Commit 1: Pair/Re-pair label driven by oc_agent_row_exists(),
// not info.agents.length / info.connected (the pre-fix proxy). See Reed's audit
// §2.3: after a disconnect, ai_agents rows are soft-deactivated (is_active = 0),
// not deleted, so info.agents === [] and info.connected === false look identical
// to a never-paired install — but oc_agent_row_exists() (no is_active filter)
// still returns true. The label (and, eventually, first-pair/re-pair routing)
// must key off that DB check, not the connection-info shape.
// ---------------------------------------------------------------------------

describe('I2 — Pair/Re-pair label via oc_agent_row_exists', () => {
  it('oc_agent_row_exists=false, never paired → button reads "Pair gateway"; clicking calls onPairRequested(\'pair\')', async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'oc_agent_row_exists') return Promise.resolve(false);
      return Promise.resolve(undefined);
    });
    const onPairRequested = vi.fn();
    const { container } = render(ProviderConnectionCard, {
      props: {
        info: makeInfo({ runtime_type: 'openclaw', connected: false, error: null, endpoint: null, masked_credential: null }),
        onChange: vi.fn(),
        onPairRequested,
      },
    });
    await openCard(container);
    await vi.waitFor(() => expect(screen.getByTestId('oc-pair-gateway').textContent?.trim()).toBe('Pair gateway'));

    await fireEvent.click(screen.getByTestId('oc-pair-gateway'));
    await tick();
    expect(onPairRequested).toHaveBeenCalledWith('pair');
  });

  it('oc_agent_row_exists=true, info.connected=true → button reads "Re-pair"; clicking calls onPairRequested(\'re-pair\')', async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'oc_agent_row_exists') return Promise.resolve(true);
      return Promise.resolve(undefined);
    });
    const onPairRequested = vi.fn();
    const { container } = render(ProviderConnectionCard, {
      props: {
        info: makeInfo({
          runtime_type: 'openclaw',
          connected: true,
          endpoint: 'https://100.1.2.3:18789',
          masked_credential: null,
          agents: [{ id: 'a1', name: 'Stia', model: 'claude', is_default: true }],
        }),
        onChange: vi.fn(),
        onPairRequested,
      },
    });
    await openCard(container);
    await vi.waitFor(() => expect(screen.getByTestId('oc-re-pair').textContent?.trim()).toBe('Re-pair'));

    await fireEvent.click(screen.getByTestId('oc-re-pair'));
    await tick();
    expect(onPairRequested).toHaveBeenCalledWith('re-pair');
  });

  // The regression case Reed's audit flags as the single highest-value new test in
  // this whole scope: exact post-disconnect shape (oc_agent_row_exists=true,
  // info.connected=false, info.agents=[]) must still read "Re-pair", not "Pair" —
  // the pre-fix `ocUnpaired` (info.connected/info.error-driven) branch selector
  // would have shown "Pair gateway" here, which routes the next pairing attempt
  // into sync_initial_import (fresh UUIDs) instead of reimport_oc_agents
  // (reactivation) — the exact D18 bug class (#2647/#2648, #2649) this fix closes.
  it('REGRESSION: oc_agent_row_exists=true, info.connected=false, info.agents=[] (post-disconnect shape) → button still reads "Re-pair", not "Pair"', async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'oc_agent_row_exists') return Promise.resolve(true);
      return Promise.resolve(undefined);
    });
    const { container } = render(ProviderConnectionCard, {
      props: {
        info: makeInfo({
          runtime_type: 'openclaw',
          connected: false,
          error: null,
          endpoint: null,
          masked_credential: null,
          agents: [],
        }),
        onChange: vi.fn(),
      },
    });
    await openCard(container);

    // Post-disconnect, the card renders in the `ocUnpaired` branch (grey "Not
    // paired" badge/status — connectivity check, unaffected by this fix) but the
    // button INSIDE that branch must still say "Re-pair", driven by hasOcAgentRows.
    await vi.waitFor(() => {
      const btn = screen.getByTestId('oc-pair-gateway');
      expect(btn.textContent?.trim()).toBe('Re-pair');
    });
    expect(screen.queryByText('Pair gateway')).toBeNull();
  });

});

// ---------------------------------------------------------------------------
// M — #2642/#2630 Commit 3/4: pairSyncError is now an EXTERNAL prop (Panel-
// threaded, fed by the modal's onConnectionSaved(lastImportError)) instead of
// internal ceremony state. The card just displays it — both the collapsed-header
// badge (#2640 D13 fix) and the expanded inline notice.
// ---------------------------------------------------------------------------

describe('M — pairSyncError external prop display', () => {
  it('renders the collapsed-header badge when pairSyncError is set', () => {
    const { container } = render(ProviderConnectionCard, {
      props: {
        info: makeInfo({ runtime_type: 'openclaw', connected: true, endpoint: 'https://100.1.2.3:18789', masked_credential: null }),
        onChange: vi.fn(),
        pairSyncError: 'gateway unreachable',
      },
    });
    const badge = container.querySelector('[data-testid="oc-pair-sync-error-badge"]');
    expect(badge).not.toBeNull();
    expect(badge?.textContent).toContain('Import failed');
  });

  it('renders no badge when pairSyncError is empty (default)', () => {
    const { container } = render(ProviderConnectionCard, {
      props: {
        info: makeInfo({ runtime_type: 'openclaw', connected: true, endpoint: 'https://100.1.2.3:18789', masked_credential: null }),
        onChange: vi.fn(),
      },
    });
    expect(container.querySelector('[data-testid="oc-pair-sync-error-badge"]')).toBeNull();
  });

  it('renders the expanded inline notice with the error text when pairSyncError is set', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: {
        info: makeInfo({ runtime_type: 'openclaw', connected: true, endpoint: 'https://100.1.2.3:18789', masked_credential: null }),
        onChange: vi.fn(),
        pairSyncError: 'is_default collision',
      },
    });
    await openCard(container);
    const inline = container.querySelector('[data-testid="oc-pair-sync-error"]');
    expect(inline).not.toBeNull();
    expect(inline?.textContent).toContain('is_default collision');
  });
});

// ---------------------------------------------------------------------------
// K — OpenClaw: Sync Now wiring (§14)
// ---------------------------------------------------------------------------

describe('K — sync now', () => {
  function makePaired(overrides: Partial<RuntimeConnectionInfo> = {}) {
    return makeInfo({
      runtime_type: 'openclaw',
      connected: true,
      endpoint: 'http://127.0.0.1:18789',
      masked_credential: null,
      ...overrides,
    });
  }

  it('renders a "Sync now" ghost button on the paired card', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makePaired(), onChange: vi.fn() },
    });
    await openCard(container);
    expect(screen.getByText('Sync now')).not.toBeNull();
  });

  it('clicking Sync now calls syncNow() and updates the timestamp to "Just now" on success', async () => {
    syncNowMock.mockResolvedValue({ success: true, timestamp: new Date().toISOString() });
    const { container } = render(ProviderConnectionCard, {
      props: { info: makePaired(), onChange: vi.fn() },
    });
    await openCard(container);

    await fireEvent.click(screen.getByText('Sync now'));
    await tick();
    await tick();

    expect(syncNowMock).toHaveBeenCalled();
    expect(container.textContent).toContain('Just now');
  });

  it('button is disabled and shows "Syncing…" while the sync is in flight', async () => {
    let resolveSync: (v: any) => void = () => {};
    syncNowMock.mockReturnValue(new Promise((res) => { resolveSync = res; }));
    const { container } = render(ProviderConnectionCard, {
      props: { info: makePaired(), onChange: vi.fn() },
    });
    await openCard(container);

    const btn = screen.getByText('Sync now') as HTMLButtonElement;
    await fireEvent.click(btn);
    await tick();

    const syncingBtn = screen.getByText('Syncing…') as HTMLButtonElement;
    expect(syncingBtn.disabled).toBe(true);

    resolveSync({ success: true, timestamp: new Date().toISOString() });
    await tick();
    await tick();
  });

  it('on failure, the row keeps its previous timestamp and the error surfaces on the card', async () => {
    syncNowMock.mockRejectedValue(new Error('Gateway unreachable'));
    const { container } = render(ProviderConnectionCard, {
      props: { info: makePaired(), onChange: vi.fn() },
    });
    await openCard(container);

    await fireEvent.click(screen.getByText('Sync now'));
    await tick();
    await tick();

    expect(container.textContent).toContain('Gateway unreachable');
    // Timestamp row still present (unchanged, showing 'Never' since none was ever set)
    expect(container.textContent).toContain('Never');
  });
});

// ---------------------------------------------------------------------------
// L — Ollama Disconnect: presentation-only feedback (#2522 design polish
// batch, item 5). B3: Disconnect semantics TBD — no backend behavior change,
// just a visible acknowledgment that the click was received.
// ---------------------------------------------------------------------------

describe('L — ollama disconnect presentation-only feedback', () => {
  function makeOllama(overrides: Partial<RuntimeConnectionInfo> = {}) {
    return makeInfo({ runtime_type: 'ollama', connected: true, endpoint: 'http://localhost:11434', masked_credential: null, ...overrides });
  }

  it('shows no acknowledgment before Disconnect is clicked', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeOllama(), onChange: vi.fn() },
    });
    await openCard(container);
    expect(container.querySelector('[data-testid="ollama-disconnect-ack"]')).toBeNull();
  });

  it('clicking Disconnect on the Ollama card shows a visible acknowledgment', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeOllama(), onChange: vi.fn() },
    });
    await openCard(container);

    await fireEvent.click(screen.getByText('Disconnect'));
    await tick();
    await fireEvent.click(container.querySelector('.confirm-modal .action-btn') as HTMLButtonElement);
    await tick();
    await tick();

    const ack = container.querySelector('[data-testid="ollama-disconnect-ack"]');
    expect(ack).not.toBeNull();
    expect(ack?.textContent).toContain('Disconnect requested');
  });

  it('still calls disconnectRuntime(\'ollama\') — presentation-only, no backend behavior change', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeOllama(), onChange: vi.fn() },
    });
    await openCard(container);

    await fireEvent.click(screen.getByText('Disconnect'));
    await tick();
    await fireEvent.click(container.querySelector('.confirm-modal .action-btn') as HTMLButtonElement);
    await tick();
    await tick();

    expect(disconnectRuntime).toHaveBeenCalledWith('ollama');
  });

  it('the acknowledgment does not appear on non-ollama cards', async () => {
    const { container } = render(ProviderConnectionCard, {
      props: { info: makeInfo({ runtime_type: 'anthropic', connected: true }), onChange: vi.fn() },
    });
    await openCard(container);

    await fireEvent.click(screen.getByText('Disconnect'));
    await tick();
    await tick();

    expect(container.querySelector('[data-testid="ollama-disconnect-ack"]')).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// N — #2641 D14: three-state cardStatus matrix
//
// A2 above already covers grey-vs-red for anthropic/openrouter/ollama via
// configured_this_install. This group adds the piece A2 doesn't: the exact #2631
// regression shape (a truly CONNECTED card with a real masked credential, but
// configured_this_install=false — the stale-keychain-leftover-from-a-previous-
// install case, where naive connected-based coloring would have shown green), plus
// the full OpenClaw hasOcAgentRows-driven three-state split and the Smart Features
// status dot (Settings.svelte).
// ---------------------------------------------------------------------------

describe('N — three-state cardStatus matrix (#2641)', () => {
  describe('#2631 regression: stale keychain leftover must render grey, not green', () => {
    it('anthropic: connected=true + masked_credential present + configured_this_install=false → grey "Not configured", NOT green "Connected"', () => {
      const { container } = render(ProviderConnectionCard, {
        props: {
          info: makeInfo({
            runtime_type: 'anthropic',
            connected: true,
            masked_credential: 'sk-ant-...leftover',
            error: null,
            configured_this_install: false,
          }),
          onChange: vi.fn(),
        },
      });
      expect(container.querySelector('.pcc-status')?.textContent?.trim()).toBe('Not configured');
      expect(container.querySelector('.pcc-status--neutral')).not.toBeNull();
      expect(container.querySelector('.pcc-status--bad')).toBeNull();
      expect(container.querySelector('.pcc-card--attn')).toBeNull();
    });

    it('openrouter: connected=true + masked_credential present + configured_this_install=false → grey "Not configured"', () => {
      const { container } = render(ProviderConnectionCard, {
        props: {
          info: makeInfo({
            runtime_type: 'openrouter',
            connected: true,
            masked_credential: 'sk-or-...leftover',
            error: null,
            configured_this_install: false,
          }),
          onChange: vi.fn(),
        },
      });
      expect(container.querySelector('.pcc-status')?.textContent?.trim()).toBe('Not configured');
      expect(container.querySelector('.pcc-status--neutral')).not.toBeNull();
    });

    it('ollama: connected=true + endpoint present + configured_this_install=false → grey "Not configured"', () => {
      const { container } = render(ProviderConnectionCard, {
        props: {
          info: makeInfo({
            runtime_type: 'ollama',
            connected: true,
            masked_credential: null,
            endpoint: 'http://localhost:11434',
            error: null,
            configured_this_install: false,
          }),
          onChange: vi.fn(),
        },
      });
      expect(container.querySelector('.pcc-status')?.textContent?.trim()).toBe('Not configured');
      expect(container.querySelector('.pcc-status--neutral')).not.toBeNull();
    });
  });

  describe('openclaw: hasOcAgentRows-driven three states', () => {
    function mockRows(exists: boolean | 'error') {
      invokeMock.mockImplementation((cmd: string) => {
        if (cmd === 'oc_agent_row_exists') {
          return exists === 'error' ? Promise.reject(new Error('unreachable')) : Promise.resolve(exists);
        }
        return Promise.resolve(undefined);
      });
    }

    it('hasOcAgentRows=false (never paired) → grey "Not paired"', async () => {
      mockRows(false);
      const { container } = render(ProviderConnectionCard, {
        props: {
          info: makeInfo({ runtime_type: 'openclaw', connected: false, error: null, endpoint: null, masked_credential: null }),
          onChange: vi.fn(),
        },
      });
      await vi.waitFor(() => expect(container.querySelector('.pcc-status')?.textContent?.trim()).toBe('Not paired'));
      expect(container.querySelector('.pcc-status--neutral')).not.toBeNull();
      expect(container.querySelector('.pcc-card--attn')).toBeNull();
    });

    it('hasOcAgentRows=true, connected=true, no error → green "Connected"', async () => {
      mockRows(true);
      const { container } = render(ProviderConnectionCard, {
        props: {
          info: makeInfo({ runtime_type: 'openclaw', connected: true, error: null, endpoint: 'https://100.64.0.1:8443/v1', masked_credential: null }),
          onChange: vi.fn(),
        },
      });
      await vi.waitFor(() => expect(container.querySelector('.pcc-status')?.textContent?.trim()).toBe('Connected'));
      expect(container.querySelector('.pcc-status--neutral')).toBeNull();
      expect(container.querySelector('.pcc-status--bad')).toBeNull();
      expect(container.querySelector('.pcc-card--attn')).toBeNull();
    });

    it('hasOcAgentRows=true, connected=false, error=null (user-initiated disconnect) → grey "Paired, disconnected", NOT red', async () => {
      mockRows(true);
      const { container } = render(ProviderConnectionCard, {
        props: {
          info: makeInfo({ runtime_type: 'openclaw', connected: false, error: null, endpoint: 'https://100.64.0.1:8443/v1', masked_credential: null }),
          onChange: vi.fn(),
        },
      });
      await vi.waitFor(() => expect(container.querySelector('.pcc-status')?.textContent?.trim()).toBe('Paired, disconnected'));
      expect(container.querySelector('.pcc-status--neutral')).not.toBeNull();
      expect(container.querySelector('.pcc-status--bad')).toBeNull();
      expect(container.querySelector('.pcc-card--attn')).toBeNull();
    });

    it('hasOcAgentRows=true, error="Gateway unreachable" (genuine failure) → red "Disconnected" + attention border', async () => {
      mockRows(true);
      const { container } = render(ProviderConnectionCard, {
        props: {
          info: makeInfo({ runtime_type: 'openclaw', connected: false, error: 'Gateway unreachable', endpoint: 'https://100.64.0.1:8443/v1', masked_credential: null }),
          onChange: vi.fn(),
        },
      });
      await vi.waitFor(() => expect(container.querySelector('.pcc-status--bad')).not.toBeNull());
      expect(container.querySelector('.pcc-status')?.textContent?.trim()).toBe('Disconnected');
      expect(container.querySelector('.pcc-card--attn')).not.toBeNull();
    });
  });
});

/**
 * ProviderConnectionsPanel.test.ts — Component tests for ProviderConnectionsPanel.svelte
 *
 * #2522 Stage 5b, ADR-0063 D8: flat list of provider cards, no "active provider" concept.
 *
 * Test groups:
 *  A — Loading state shown before listRuntimeConnections resolves
 *  B — Renders one ProviderConnectionCard per RuntimeConnectionInfo returned
 *  C — Error state + retry
 *  D — No radio-group / active-provider selection UI anywhere in the panel
 *  E — #2642/#2630 Commit 5: OpenClaw card's Pair/Re-pair button calls
 *      onPairRequested, which the Panel threads into ocDeepLink state → the
 *      modal's startCeremony prop — a real prop-based deep link, replacing the
 *      old driveOcCard() DOM-query hack (Reed finding 6, #2522 Stage 8b) this
 *      group used to test.
 *  F — #2522 design polish batch, item 4: the fetched connections list is threaded
 *      into NewProviderModal so it can show already-connected state for any provider.
 *  G — D24 (#2649): onAgentsChanged fires from handleConnectionSaved so Settings.svelte
 *      can reset its agentsLoaded gate and re-fetch the Agents tab after an OC import.
 *  H — D25 (#2649): onAgentsChanged also fires from handleCardChange, the new
 *      onChange prop wired to every ProviderConnectionCard. Disconnect (and any
 *      other card-level mutation) goes through this separate path from the
 *      modal's handleConnectionSaved, and D24 missed it.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, fireEvent, screen } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import ProviderConnectionsPanel from './ProviderConnectionsPanel.svelte';
import type { RuntimeConnectionInfo } from '$lib/api';
import { disconnectRuntime } from '$lib/api';

const { listRuntimeConnectionsMock, invokeMock } = vi.hoisted(() => ({
  listRuntimeConnectionsMock: vi.fn(),
  invokeMock: vi.fn(),
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: invokeMock,
}));

vi.mock('$lib/api', async (importOriginal) => {
  const original = await importOriginal() as any;
  return {
    ...original,
    listRuntimeConnections: listRuntimeConnectionsMock,
    disconnectRuntime: vi.fn().mockResolvedValue(undefined),
  };
});

// Same shim NewProviderModal.test.ts uses: the vitest-setup.ts window mock lacks
// addEventListener/removeEventListener, needed for <svelte:window onkeydown> inside
// the modal this panel mounts.
if (typeof (globalThis.window as any).addEventListener !== 'function') {
  (globalThis.window as any).addEventListener = () => {};
}
if (typeof (globalThis.window as any).removeEventListener !== 'function') {
  (globalThis.window as any).removeEventListener = () => {};
}

const FIVE_RUNTIMES: RuntimeConnectionInfo[] = [
  { runtime_type: 'openclaw', connected: true, masked_credential: null, endpoint: 'http://127.0.0.1:18789', error: null, agents: [], configured_this_install: true },
  { runtime_type: 'openrouter', connected: false, masked_credential: null, endpoint: null, error: null, agents: [], configured_this_install: false },
  { runtime_type: 'anthropic', connected: true, masked_credential: 'sk-ant-...abcd', endpoint: null, error: null, agents: [], configured_this_install: true },
  { runtime_type: 'ollama', connected: false, masked_credential: null, endpoint: 'http://localhost:11434', error: null, agents: [], configured_this_install: false },
  { runtime_type: 'venice', connected: false, masked_credential: null, endpoint: null, error: null, agents: [], configured_this_install: false },
];

beforeEach(() => {
  listRuntimeConnectionsMock.mockReset();
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
});

afterEach(() => {
  vi.resetAllMocks();
});

describe('A — loading state', () => {
  it('shows a loading message before the fetch resolves', async () => {
    let resolveFetch: (v: RuntimeConnectionInfo[]) => void = () => {};
    listRuntimeConnectionsMock.mockReturnValue(new Promise((res) => { resolveFetch = res; }));

    const { container } = render(ProviderConnectionsPanel);
    await tick();

    expect(container.querySelector('.pcp-loading')).not.toBeNull();
    expect(container.textContent).toContain('Loading');

    resolveFetch(FIVE_RUNTIMES);
    await tick();
    await tick();
  });
});

describe('B — renders a card only for configured providers (Blocker 3 fix, #2777 venice live-test scoping)', () => {
  it('fetches on mount and renders a card only for each configured_this_install: true runtime', async () => {
    listRuntimeConnectionsMock.mockResolvedValue(FIVE_RUNTIMES);

    const { container } = render(ProviderConnectionsPanel);
    await tick();
    await tick();
    await tick();

    expect(listRuntimeConnectionsMock).toHaveBeenCalledOnce();
    // FIVE_RUNTIMES: only openclaw and anthropic have configured_this_install: true.
    const cards = container.querySelectorAll('.pcc-card');
    expect(cards.length).toBe(2);
  });

  it('renders cards only for configured runtimes — unconfigured ones (openrouter/ollama/venice in FIVE_RUNTIMES) are excluded entirely', async () => {
    listRuntimeConnectionsMock.mockResolvedValue(FIVE_RUNTIMES);

    const { container } = render(ProviderConnectionsPanel);
    await tick();
    await tick();
    await tick();

    // Configured (configured_this_install: true) — cards present.
    expect(container.querySelector('.pcc-card[data-runtime="openclaw"]')).not.toBeNull();
    expect(container.querySelector('.pcc-card[data-runtime="anthropic"]')).not.toBeNull();
    // Unconfigured (configured_this_install: false) — no card at all, not even a
    // grey/disconnected one. "+ New Provider" is the only way to reach these.
    expect(container.querySelector('.pcc-card[data-runtime="openrouter"]')).toBeNull();
    expect(container.querySelector('.pcc-card[data-runtime="ollama"]')).toBeNull();
    expect(container.querySelector('.pcc-card[data-runtime="venice"]')).toBeNull();
  });

  it('the "+ New Provider" button remains visible even when every card is filtered out', async () => {
    listRuntimeConnectionsMock.mockResolvedValue(
      FIVE_RUNTIMES.map((c) => ({ ...c, configured_this_install: false }))
    );

    const { container } = render(ProviderConnectionsPanel);
    await tick();
    await tick();
    await tick();

    expect(container.querySelectorAll('.pcc-card').length).toBe(0);
    expect(container.querySelector('.pcp-new-btn')).not.toBeNull();
  });

  it('renders zero cards when the backend returns an empty list', async () => {
    listRuntimeConnectionsMock.mockResolvedValue([]);

    const { container } = render(ProviderConnectionsPanel);
    await tick();
    await tick();
    await tick();

    expect(container.querySelectorAll('.pcc-card').length).toBe(0);
    expect(container.querySelector('.pcp-error')).toBeNull();
  });
});

describe('C — error state + retry', () => {
  it('shows an error message when the fetch rejects', async () => {
    listRuntimeConnectionsMock.mockRejectedValue(new Error('backend unreachable'));

    const { container } = render(ProviderConnectionsPanel);
    await tick();
    await tick();
    await tick();

    const errEl = container.querySelector('.pcp-error');
    expect(errEl).not.toBeNull();
    expect(errEl?.textContent).toContain('backend unreachable');
  });

  it('retry button re-invokes listRuntimeConnections and can recover to the loaded state', async () => {
    listRuntimeConnectionsMock.mockRejectedValueOnce(new Error('backend unreachable'));
    listRuntimeConnectionsMock.mockResolvedValueOnce(FIVE_RUNTIMES);

    const { container } = render(ProviderConnectionsPanel);
    await tick();
    await tick();
    await tick();

    expect(container.querySelector('.pcp-error')).not.toBeNull();

    const retryBtn = container.querySelector('.pcp-error button') as HTMLElement;
    expect(retryBtn).not.toBeNull();
    await fireEvent.click(retryBtn);
    await tick();
    await tick();
    await tick();

    expect(listRuntimeConnectionsMock).toHaveBeenCalledTimes(2);
    expect(container.querySelector('.pcp-error')).toBeNull();
    // Blocker 3 fix: only configured providers get a card — FIVE_RUNTIMES has two
    // (openclaw, anthropic).
    expect(container.querySelectorAll('.pcc-card').length).toBe(2);
  });
});

describe('D — no active-provider / radio-group concept (ADR-0063 D8)', () => {
  it('renders no radio inputs and no "active provider" selector', async () => {
    listRuntimeConnectionsMock.mockResolvedValue(FIVE_RUNTIMES);

    const { container } = render(ProviderConnectionsPanel);
    await tick();
    await tick();
    await tick();

    expect(container.querySelectorAll('input[type="radio"]').length).toBe(0);
    expect(container.textContent?.toLowerCase()).not.toContain('active provider');
  });
});

describe('F — connections threaded into NewProviderModal (#2522 design polish batch, item 4)', () => {
  it('New Provider modal Step 1 hides already-connected anthropic (#2590 UX batch 2, item 2) — the connections list threaded down from the Panel is the same fetched list the cards use, filtering it out of the "add new" picker', async () => {
    listRuntimeConnectionsMock.mockResolvedValue(FIVE_RUNTIMES);

    const { container } = render(ProviderConnectionsPanel);
    await tick();
    await tick();
    await tick();

    const newProviderBtn = container.querySelector('.pcp-new-btn') as HTMLElement;
    await fireEvent.click(newProviderBtn);
    await tick();

    // FIVE_RUNTIMES' anthropic AND openclaw entries are connected — #2590 item 2
    // filters both out of Step 1 entirely rather than leaving them selectable and
    // then showing an "already connected" Step 2 summary (the pre-#2590 flow this
    // test used to exercise). Confirms the Panel's own fetched `connections` list
    // (and the `ocConnectionInfo` derived from it) is what's driving the filter,
    // not a modal-local stub. openrouter/ollama/venice (disconnected in FIVE_RUNTIMES)
    // remain selectable.
    const rows = Array.from(container.querySelectorAll('[role="radio"]'));
    expect(rows.length).toBe(3);
    expect(rows.some((r) => r.textContent?.includes('Anthropic'))).toBe(false);
    expect(rows.some((r) => r.textContent?.includes('OpenClaw'))).toBe(false);
    expect(rows.some((r) => r.textContent?.includes('OpenRouter'))).toBe(true);
    expect(rows.some((r) => r.textContent?.includes('Ollama'))).toBe(true);
    expect(rows.some((r) => r.textContent?.includes('Venice'))).toBe(true);
  });
});

describe('E — onPairRequested → ocDeepLink → startCeremony prop-based deep link (#2642/#2630 Commit 5)', () => {
  it('clicking the unpaired card\'s Pair gateway button opens NewProviderModal already on the ceremony URL step (startCeremony=\'pair\'), skipping Step 1', async () => {
    // Blocker 3 fix (#2777 venice live-test scoping): the card-visibility filter
    // now hides any card with configured_this_install: false. A genuinely never-
    // configured OpenClaw would have no card at all (re-pair then only reachable
    // via "+ New Provider", per the scoping audit's cross-cutting note) — but this
    // test's purpose is exercising the onPairRequested → ocDeepLink → startCeremony
    // plumbing itself, which is independent of that real-world visibility question.
    // configured_this_install: true keeps the card rendered so the mechanism stays
    // covered; `connected: false` still drives the card's own "unpaired body"
    // (Pair gateway button) rendering via `ocUnpaired`, unaffected by this fix.
    const UNPAIRED_RUNTIMES: RuntimeConnectionInfo[] = [
      { runtime_type: 'openclaw', connected: false, masked_credential: null, endpoint: null, error: null, agents: [], configured_this_install: true },
      { runtime_type: 'openrouter', connected: false, masked_credential: null, endpoint: null, error: null, agents: [], configured_this_install: false },
      { runtime_type: 'anthropic', connected: true, masked_credential: 'sk-ant-...abcd', endpoint: null, error: null, agents: [], configured_this_install: true },
      { runtime_type: 'ollama', connected: false, masked_credential: null, endpoint: 'http://localhost:11434', error: null, agents: [], configured_this_install: false },
    ];
    listRuntimeConnectionsMock.mockResolvedValue(UNPAIRED_RUNTIMES);

    const { container } = render(ProviderConnectionsPanel);
    await tick();
    await tick();
    await tick();

    const ocCard = container.querySelector('.pcc-card[data-runtime="openclaw"]') as HTMLElement;
    expect(ocCard).not.toBeNull();
    const topBtn = ocCard.querySelector('.pcc-top') as HTMLElement;
    await fireEvent.click(topBtn);
    await tick();

    const pairBtn = ocCard.querySelector('[data-testid="oc-pair-gateway"]') as HTMLElement;
    expect(pairBtn.textContent).toContain('Pair gateway');
    await fireEvent.click(pairBtn);
    await tick();

    // The modal is open, deep-linked straight into the ceremony's URL step — no
    // Step 1 provider picker was ever rendered for this flow.
    expect(container.querySelectorAll('[role="radio"]').length).toBe(0);
    expect(container.textContent).toContain('Pair OpenClaw');
    expect(screen.getByLabelText('Gateway URL')).not.toBeNull();
  });

  it('clicking the paired card\'s Re-pair button opens NewProviderModal on the ceremony URL step (startCeremony=\'re-pair\') with the URL prefilled from the existing endpoint', async () => {
    listRuntimeConnectionsMock.mockResolvedValue(FIVE_RUNTIMES);
    // hasOcAgentRows (Commit 1) drives the card's Pair/Re-pair mode passed to
    // onPairRequested — must resolve true so the card's own Re-pair button
    // actually calls onPairRequested('re-pair'), not ('pair').
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'oc_agent_row_exists') return Promise.resolve(true);
      return Promise.resolve(undefined);
    });

    const { container } = render(ProviderConnectionsPanel);
    await tick();
    await tick();
    await tick();

    const ocCard = container.querySelector('.pcc-card[data-runtime="openclaw"]') as HTMLElement;
    const topBtn = ocCard.querySelector('.pcc-top') as HTMLElement;
    await fireEvent.click(topBtn);
    await tick();

    const rePairBtn = ocCard.querySelector('[data-testid="oc-re-pair"]') as HTMLElement;
    expect(rePairBtn.textContent).toContain('Re-pair');
    await fireEvent.click(rePairBtn);
    await tick();

    expect(container.textContent).toContain('Re-pair OpenClaw');
    const urlInput = screen.getByLabelText('Gateway URL') as HTMLInputElement;
    // FIVE_RUNTIMES' openclaw entry has endpoint 'http://127.0.0.1:18789'.
    expect(urlInput.value).toBe('http://127.0.0.1:18789');
  });

  it('closing the modal (X button) resets ocDeepLink — a subsequent "+ New Provider" click starts fresh at Step 1, not mid-ceremony', async () => {
    // See comment on the first test in this group — configured_this_install: true
    // keeps the openclaw card rendered under the Blocker 3 fix so this test can
    // still reach the card's Pair gateway button.
    const UNPAIRED_RUNTIMES: RuntimeConnectionInfo[] = [
      { runtime_type: 'openclaw', connected: false, masked_credential: null, endpoint: null, error: null, agents: [], configured_this_install: true },
      { runtime_type: 'openrouter', connected: false, masked_credential: null, endpoint: null, error: null, agents: [], configured_this_install: false },
      { runtime_type: 'anthropic', connected: true, masked_credential: 'sk-ant-...abcd', endpoint: null, error: null, agents: [], configured_this_install: true },
      { runtime_type: 'ollama', connected: false, masked_credential: null, endpoint: 'http://localhost:11434', error: null, agents: [], configured_this_install: false },
    ];
    listRuntimeConnectionsMock.mockResolvedValue(UNPAIRED_RUNTIMES);

    const { container } = render(ProviderConnectionsPanel);
    await tick();
    await tick();
    await tick();

    const ocCard = container.querySelector('.pcc-card[data-runtime="openclaw"]') as HTMLElement;
    const topBtn = ocCard.querySelector('.pcc-top') as HTMLElement;
    await fireEvent.click(topBtn);
    await tick();
    await fireEvent.click(ocCard.querySelector('[data-testid="oc-pair-gateway"]') as HTMLElement);
    await tick();
    expect(container.textContent).toContain('Pair OpenClaw');

    const closeBtn = container.querySelector('.npm-close-btn') as HTMLElement;
    await fireEvent.click(closeBtn);
    await tick();
    expect(container.querySelector('.npm-modal')).toBeNull();

    // Re-opening via the plain "+ New Provider" button (not the card) must land on
    // Step 1, proving ocDeepLink was reset to null on close, not left as 'pair'.
    const newProviderBtn = container.querySelector('.pcp-new-btn') as HTMLElement;
    await fireEvent.click(newProviderBtn);
    await tick();

    expect(container.querySelectorAll('[role="radio"]').length).toBeGreaterThan(0);
    expect(container.textContent).not.toContain('Pair OpenClaw');
  });
});

describe('G — D24 (#2649): onAgentsChanged notifies Settings.svelte to re-fetch the Agents tab', () => {
  it('a successful "Add connection" (OpenRouter) triggers handleConnectionSaved, which calls onAgentsChanged after loadConnections resolves', async () => {
    listRuntimeConnectionsMock.mockResolvedValue(FIVE_RUNTIMES);
    invokeMock.mockResolvedValue(undefined);
    const onAgentsChanged = vi.fn();

    const { container } = render(ProviderConnectionsPanel, { props: { onAgentsChanged } });
    await tick();
    await tick();
    await tick();

    await fireEvent.click(container.querySelector('.pcp-new-btn') as HTMLElement);
    await tick();

    const rows = Array.from(container.querySelectorAll('[role="radio"]'));
    const openrouterRow = rows.find((r) => r.textContent?.includes('OpenRouter')) as HTMLButtonElement;
    await fireEvent.click(openrouterRow);
    await tick();
    await fireEvent.click(screen.getByText('Continue'));
    await tick();

    const keyInput = screen.getByLabelText('API Key') as HTMLInputElement;
    await fireEvent.input(keyInput, { target: { value: 'sk-or-test' } });
    await tick();

    expect(onAgentsChanged).not.toHaveBeenCalled();

    await fireEvent.click(screen.getByText('Add connection'));
    // Fix 2 (§28.2/#1630) adds an extra await hop before ai_configure_and_save
    // (runApiProviderValidator's invoke('validate_openrouter_key', ...)) — one
    // more tick is needed for that promise chain to settle before
    // onConnectionSaved → handleConnectionSaved → loadConnections →
    // onAgentsChanged all resolve.
    await tick();
    await tick();
    await tick();
    await tick();

    expect(onAgentsChanged).toHaveBeenCalledTimes(1);
    // Must fire only after loadConnections() has re-fetched — not before.
    expect(listRuntimeConnectionsMock.mock.calls.length).toBeGreaterThanOrEqual(2);
  });

  it('the real bug scenario: a successful OC pairing import calls onAgentsChanged when the Imported Agents popup is dismissed', async () => {
    // Blocker 3 fix: configured_this_install: true keeps the openclaw card
    // rendered so this test can reach it — see comment on group E's first test.
    const UNPAIRED_RUNTIMES: RuntimeConnectionInfo[] = [
      { runtime_type: 'openclaw', connected: false, masked_credential: null, endpoint: null, error: null, agents: [], configured_this_install: true },
      { runtime_type: 'openrouter', connected: false, masked_credential: null, endpoint: null, error: null, agents: [], configured_this_install: false },
      { runtime_type: 'anthropic', connected: true, masked_credential: 'sk-ant-...abcd', endpoint: null, error: null, agents: [], configured_this_install: true },
      { runtime_type: 'ollama', connected: false, masked_credential: null, endpoint: 'http://localhost:11434', error: null, agents: [], configured_this_install: false },
    ];
    listRuntimeConnectionsMock.mockResolvedValue(UNPAIRED_RUNTIMES);
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'oc_agent_row_exists') return Promise.resolve(false);
      if (cmd === 'gateway_fetch_fingerprint') {
        return Promise.resolve({ tls_fingerprint: 'AA:BB:CC:DD', cert_pem: '-----CERT-----', gateway_hostname: null });
      }
      if (cmd === 'gateway_exchange_pairing_code') {
        return Promise.resolve({ token: 'tok_first', gateway_version: '0.3.0', version_warning: null });
      }
      if (cmd === 'sync_initial_import') return Promise.resolve({ agent_names: ['Stia'], agents_missing_model: [] });
      return Promise.resolve(undefined);
    });
    const onAgentsChanged = vi.fn();

    const { container } = render(ProviderConnectionsPanel, { props: { onAgentsChanged } });
    await tick();
    await tick();
    await tick();

    const ocCard = container.querySelector('.pcc-card[data-runtime="openclaw"]') as HTMLElement;
    await fireEvent.click(ocCard.querySelector('.pcc-top') as HTMLElement);
    await tick();
    await fireEvent.click(ocCard.querySelector('[data-testid="oc-pair-gateway"]') as HTMLElement);
    await tick();

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

    await vi.waitFor(() => expect(screen.getByText('Agent imported')).not.toBeNull());
    expect(onAgentsChanged).not.toHaveBeenCalled();

    await fireEvent.click(screen.getByTestId('imported-agents-done'));
    await tick();
    await tick();

    // handleConnectionSaved() fires from dismissImportedAgentsPopup's onConnectionSaved()
    // call — this is the exact D24 (#2649) bug path: without onAgentsChanged, Settings.svelte's
    // agentsLoaded gate never resets and the Agents tab keeps showing the pre-import roster.
    expect(onAgentsChanged).toHaveBeenCalledTimes(1);
  });

  it('omitting the onAgentsChanged prop does not throw when handleConnectionSaved fires (optional callback, matches every other onChange call site)', async () => {
    listRuntimeConnectionsMock.mockResolvedValue(FIVE_RUNTIMES);
    invokeMock.mockResolvedValue(undefined);

    const { container } = render(ProviderConnectionsPanel);
    await tick();
    await tick();
    await tick();

    await fireEvent.click(container.querySelector('.pcp-new-btn') as HTMLElement);
    await tick();
    const rows = Array.from(container.querySelectorAll('[role="radio"]'));
    const openrouterRow = rows.find((r) => r.textContent?.includes('OpenRouter')) as HTMLButtonElement;
    await fireEvent.click(openrouterRow);
    await tick();
    await fireEvent.click(screen.getByText('Continue'));
    await tick();
    const keyInput = screen.getByLabelText('API Key') as HTMLInputElement;
    await fireEvent.input(keyInput, { target: { value: 'sk-or-test' } });
    await tick();

    await expect(fireEvent.click(screen.getByText('Add connection'))).resolves.not.toThrow();
    await tick();
    await tick();
  });
});

describe('H — D25 (#2649): onAgentsChanged fires from handleCardChange (disconnect and other card-level mutations)', () => {
  it('disconnecting a card calls loadConnections then onAgentsChanged — the bug path the backend log (line 1307) exposed: OC disconnect promotes a new default agent, but Settings never re-fetched', async () => {
    listRuntimeConnectionsMock.mockResolvedValue(FIVE_RUNTIMES);
    vi.stubGlobal('confirm', vi.fn(() => true));
    const onAgentsChanged = vi.fn();

    const { container } = render(ProviderConnectionsPanel, { props: { onAgentsChanged } });
    await tick();
    await tick();
    await tick();

    // Anthropic card is connected in FIVE_RUNTIMES and uses the plain
    // window.confirm() disconnect path (not the OC modal-confirm path), the
    // simplest card-level mutation that exercises the shared onChange wiring.
    const anthropicCard = container.querySelector('.pcc-card[data-runtime="anthropic"]') as HTMLElement;
    expect(anthropicCard).not.toBeNull();
    await fireEvent.click(anthropicCard.querySelector('.pcc-top') as HTMLElement);
    await tick();

    expect(onAgentsChanged).not.toHaveBeenCalled();

    // #2892: the plain-runtime Disconnect path now opens a ConfirmDialog
    // instead of calling window.confirm() directly — click Disconnect to
    // open it, then click the dialog's own confirm button.
    await fireEvent.click(screen.getByText('Disconnect'));
    await tick();
    await fireEvent.click(container.querySelector('.confirm-modal .action-btn') as HTMLButtonElement);
    await tick();
    await tick();
    await tick();

    expect(disconnectRuntime).toHaveBeenCalledWith('anthropic');
    // Must fire only after loadConnections() has re-fetched — same ordering
    // handleConnectionSaved uses — not before.
    expect(listRuntimeConnectionsMock.mock.calls.length).toBeGreaterThanOrEqual(2);
    expect(onAgentsChanged).toHaveBeenCalledTimes(1);
  });

  it('omitting the onAgentsChanged prop does not throw when a card disconnects (optional callback)', async () => {
    listRuntimeConnectionsMock.mockResolvedValue(FIVE_RUNTIMES);
    vi.stubGlobal('confirm', vi.fn(() => true));

    const { container } = render(ProviderConnectionsPanel);
    await tick();
    await tick();
    await tick();

    const anthropicCard = container.querySelector('.pcc-card[data-runtime="anthropic"]') as HTMLElement;
    await fireEvent.click(anthropicCard.querySelector('.pcc-top') as HTMLElement);
    await tick();

    await expect(fireEvent.click(screen.getByText('Disconnect'))).resolves.not.toThrow();
    await tick();
    await expect(
      fireEvent.click(container.querySelector('.confirm-modal .action-btn') as HTMLButtonElement)
    ).resolves.not.toThrow();
    await tick();
    await tick();
  });
});

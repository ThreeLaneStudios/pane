/**
 * D24 (#2649) — Agents tab refresh after OC import
 *
 * Bug: After OC pairing imports new agents, the Agents tab in Settings didn't
 * show them until the user navigated away and back. Root cause:
 * `handleConnectionSaved` in ProviderConnectionsPanel.svelte called
 * `loadConnections()` (refreshes provider cards only) but never signaled
 * Settings to re-fetch agents. Settings.svelte's `loadAgents()` is gated on
 * `agentsLoaded === false` (the "Load agents when switching to agents tab"
 * `$effect`), which is set `true` on first load and never reset — so once the
 * Agents tab had been opened once, it never refreshed again on its own.
 *
 * Matt's ruling: one line — `handleConnectionSaved` resets `agentsLoaded` (or
 * calls `loadAgents` directly) so the Agents tab refreshes after import.
 *
 * Fix (Option 1, the cleaner of the two Matt offered): `ProviderConnectionsPanel`
 * gains an `onAgentsChanged?: () => void` callback prop, called from
 * `handleConnectionSaved` right after `loadConnections()`. `Settings.svelte`
 * passes `onAgentsChanged={() => { agentsLoaded = false; }}`, which the
 * existing `$effect` (gated on `activeTab === 'agents' && !agentsLoaded &&
 * !loadingAgents`) picks up and re-fetches from.
 *
 * Test groups:
 *  A — Source-level pins on Settings.svelte: onAgentsChanged prop wired to
 *      `agentsLoaded = false`, and the pre-existing $effect gate that consumes it.
 *  B — Source-level pin on ProviderConnectionsPanel.svelte: the callback prop
 *      exists and `handleConnectionSaved` calls it after `loadConnections()`.
 *  C — Component-level integration: rendering ProviderConnectionsPanel with a
 *      Settings-shaped harness (mirroring the real `agentsLoaded` $effect gate)
 *      proves a post-import `handleConnectionSaved` call causes a genuine
 *      re-fetch — the exact "new agents appear without a component remount"
 *      behavior this bug broke.
 *
 * Settings.svelte requires the full Tauri window context and cannot be
 * DOM-rendered directly (same constraint documented in Settings.test.ts /
 * Settings.oc-create-gate.test.ts / Settings.agents-tab-active-filter.test.ts).
 * Group A follows that established source-level pattern. Group C exercises the
 * real ProviderConnectionsPanel.svelte component (no extraction/re-implementation)
 * against a minimal harness that reproduces Settings.svelte's actual gate logic,
 * so a revert of either half of the fix fails this suite.
 *
 * Harness: Vitest (jsdom) — matches existing pattern in src/lib/components/*.test.ts.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { render, fireEvent, screen } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import ProviderConnectionsPanel from './ProviderConnectionsPanel.svelte';
import type { RuntimeConnectionInfo } from '$lib/api';

// ---------------------------------------------------------------------------
// Group A/B — source-level pins
// ---------------------------------------------------------------------------

describe('Settings.svelte — source-level: onAgentsChanged resets agentsLoaded (D24, #2649)', () => {
  const source = readFileSync(resolve(__dirname, 'Settings.svelte'), 'utf-8');

  it('passes onAgentsChanged to ProviderConnectionsPanel that resets agentsLoaded to false', () => {
    expect(source).toMatch(/<ProviderConnectionsPanel[^>]*onAgentsChanged=\{\(\) => \{ agentsLoaded = false; \}\}/);
  });

  it('still binds pairingFingerprint alongside the new prop (existing wiring untouched)', () => {
    expect(source).toMatch(/<ProviderConnectionsPanel bind:pairingFingerprint onAgentsChanged=/);
  });

  it('the pre-existing Agents-tab $effect gate that consumes agentsLoaded is unchanged', () => {
    expect(source).toMatch(
      /\$effect\(\(\) => \{\s*if \(activeTab === 'agents' && !agentsLoaded && !loadingAgents\) \{\s*loadAgents\(\);\s*\}\s*\}\);/
    );
  });
});

describe('ProviderConnectionsPanel.svelte — source-level: handleConnectionSaved calls onAgentsChanged (D24, #2649)', () => {
  const source = readFileSync(resolve(__dirname, 'ProviderConnectionsPanel.svelte'), 'utf-8');

  it('declares an onAgentsChanged callback prop', () => {
    expect(source).toMatch(/onAgentsChanged\?: \(\) => void;/);
  });

  it('handleConnectionSaved calls onAgentsChanged after loadConnections resolves, not before', () => {
    const match = source.match(
      /async function handleConnectionSaved\(lastImportError\?: string\) \{[\s\S]*?\n  \}/
    );
    expect(match).not.toBeNull();
    const body = match![0];
    const loadIdx = body.indexOf('await loadConnections();');
    const notifyIdx = body.indexOf('onAgentsChanged?.();');
    expect(loadIdx).toBeGreaterThan(-1);
    expect(notifyIdx).toBeGreaterThan(-1);
    expect(notifyIdx).toBeGreaterThan(loadIdx);
  });
});

// ---------------------------------------------------------------------------
// Group C — component-level integration against a Settings-shaped harness
// ---------------------------------------------------------------------------

const { listRuntimeConnectionsMock, invokeMock } = vi.hoisted(() => ({
  listRuntimeConnectionsMock: vi.fn(),
  invokeMock: vi.fn(),
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: invokeMock,
}));

vi.mock('$lib/api', async (importOriginal) => {
  const original = (await importOriginal()) as any;
  return {
    ...original,
    listRuntimeConnections: listRuntimeConnectionsMock,
    disconnectRuntime: vi.fn().mockResolvedValue(undefined),
  };
});

if (typeof (globalThis.window as any).addEventListener !== 'function') {
  (globalThis.window as any).addEventListener = () => {};
}
if (typeof (globalThis.window as any).removeEventListener !== 'function') {
  (globalThis.window as any).removeEventListener = () => {};
}

// Blocker 3 fix (#2777 venice live-test scoping): cards now only render when
// configured_this_install is true. openclaw's own card body still renders its
// unpaired state (Pair gateway button) based on `connected: false` —
// unaffected by this fix — but the card itself needs configured_this_install:
// true to exist at all so this test can reach it, same as
// ProviderConnectionsPanel.test.ts's group E fixtures.
const INITIAL_RUNTIMES: RuntimeConnectionInfo[] = [
  { runtime_type: 'openclaw', connected: false, masked_credential: null, endpoint: null, error: null, agents: [], configured_this_install: true },
  { runtime_type: 'openrouter', connected: false, masked_credential: null, endpoint: null, error: null, agents: [], configured_this_install: false },
  { runtime_type: 'anthropic', connected: true, masked_credential: 'sk-ant-...abcd', endpoint: null, error: null, agents: [], configured_this_install: true },
  { runtime_type: 'ollama', connected: false, masked_credential: null, endpoint: 'http://localhost:11434', error: null, agents: [], configured_this_install: false },
];

/** Minimal fake "Agents tab" of a Fake Agent type — just enough shape to prove
 *  a re-fetch happened and produced a new roster, without pulling in the full
 *  ~4900-line Settings.svelte (Tauri window context constraint, see file header). */
interface FakeAgent {
  id: string;
  name: string;
}

/**
 * Reproduces Settings.svelte's real state machine for this bug: `agentsLoaded`,
 * `loadingAgents`, `loadAgents()`, and the `$effect` gate
 * (`activeTab === 'agents' && !agentsLoaded && !loadingAgents`) that calls it —
 * driven here by directly toggling `agentsLoaded` the same way the real $effect
 * reacts to it, since Vitest/jsdom can't run Settings.svelte's own $effect
 * without the full component mounted.
 */
function makeAgentsTabHarness(fetchAgents: () => Promise<FakeAgent[]>) {
  let agentsLoaded = false;
  let loadingAgents = false;
  let agentsList: FakeAgent[] = [];

  async function loadAgents() {
    loadingAgents = true;
    try {
      agentsList = await fetchAgents();
      agentsLoaded = true;
    } finally {
      loadingAgents = false;
    }
  }

  // Mirrors the $effect: called whenever the Agents tab is "active" and a
  // dependency (agentsLoaded) changes.
  async function onAgentsTabActiveOrDepsChanged() {
    if (!agentsLoaded && !loadingAgents) {
      await loadAgents();
    }
  }

  function resetAgentsLoaded() {
    agentsLoaded = false;
  }

  return {
    state: () => ({ agentsLoaded, loadingAgents, agentsList }),
    onAgentsTabActiveOrDepsChanged,
    resetAgentsLoaded,
  };
}

beforeEach(() => {
  listRuntimeConnectionsMock.mockReset();
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
});

describe('D24 (#2649) — integration: post-import onAgentsChanged drives a real Agents-tab re-fetch', () => {
  it('agentsList reflects the newly-imported agent after handleConnectionSaved fires onAgentsChanged, matching the real agentsLoaded-gate $effect — no component remount required', async () => {
    listRuntimeConnectionsMock.mockResolvedValue(INITIAL_RUNTIMES);

    const fetchAgents = vi
      .fn()
      .mockResolvedValueOnce([{ id: 'a1', name: 'Brit Freed' }])
      .mockResolvedValueOnce([
        { id: 'a1', name: 'Brit Freed' },
        { id: 'a2', name: 'Stia' }, // newly imported via OC pairing
      ]);

    const harness = makeAgentsTabHarness(fetchAgents);

    // 1. Seed initial agents — mirrors "the Agents tab was already opened once".
    await harness.onAgentsTabActiveOrDepsChanged();
    expect(harness.state().agentsLoaded).toBe(true);
    expect(harness.state().agentsList.map((a) => a.id)).toEqual(['a1']);
    expect(fetchAgents).toHaveBeenCalledTimes(1);

    // 2. Render the real ProviderConnectionsPanel wired to the harness's reset,
    //    exactly as Settings.svelte wires it: onAgentsChanged={() => { agentsLoaded = false; }}
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'oc_agent_row_exists') return Promise.resolve(false);
      if (cmd === 'gateway_fetch_fingerprint') {
        return Promise.resolve({ tls_fingerprint: 'AA:BB:CC:DD', cert_pem: '-----CERT-----', gateway_hostname: null });
      }
      if (cmd === 'gateway_exchange_pairing_code') {
        return Promise.resolve({ token: 'tok_import', gateway_version: '0.3.0', version_warning: null });
      }
      if (cmd === 'sync_initial_import') {
        return Promise.resolve({ agent_names: ['Stia'], agents_missing_model: [] });
      }
      return Promise.resolve(undefined);
    });

    const { container } = render(ProviderConnectionsPanel, {
      props: { onAgentsChanged: harness.resetAgentsLoaded },
    });
    await tick();
    await tick();
    await tick();

    // 3. Trigger handleConnectionSaved (simulating post-import) via the real
    //    OC pairing flow end-to-end through the OC card + modal — the same path
    //    the bug report describes ("after OC pairing imports new agents").
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

    // agentsLoaded must still be true (unchanged) until the popup is dismissed —
    // handleConnectionSaved only fires from dismissImportedAgentsPopup.
    expect(harness.state().agentsLoaded).toBe(true);

    await fireEvent.click(screen.getByTestId('imported-agents-done'));
    await tick();
    await tick();

    // 4. onAgentsChanged fired → harness's agentsLoaded reset to false. Assert
    //    the *gate*, not just the callback — this is the actual behavior the
    //    bug broke (Settings.svelte's $effect never re-ran loadAgents()).
    expect(harness.state().agentsLoaded).toBe(false);

    // 5. Re-run the same $effect Settings.svelte declares — proves the gate now
    //    lets a real re-fetch through, without remounting anything.
    await harness.onAgentsTabActiveOrDepsChanged();

    expect(fetchAgents).toHaveBeenCalledTimes(2);
    expect(harness.state().agentsLoaded).toBe(true);
    expect(harness.state().agentsList.map((a) => a.id)).toEqual(['a1', 'a2']);
    expect(harness.state().agentsList.some((a) => a.name === 'Stia')).toBe(true);
  });

  it('regression guard: without onAgentsChanged wired (prop omitted), agentsLoaded is never reset and the stale roster persists — this is the exact pre-fix bug', async () => {
    listRuntimeConnectionsMock.mockResolvedValue(INITIAL_RUNTIMES);
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'oc_agent_row_exists') return Promise.resolve(false);
      if (cmd === 'gateway_fetch_fingerprint') {
        return Promise.resolve({ tls_fingerprint: 'AA:BB:CC:DD', cert_pem: '-----CERT-----', gateway_hostname: null });
      }
      if (cmd === 'gateway_exchange_pairing_code') {
        return Promise.resolve({ token: 'tok_stale', gateway_version: '0.3.0', version_warning: null });
      }
      if (cmd === 'sync_initial_import') {
        return Promise.resolve({ agent_names: ['Stia'], agents_missing_model: [] });
      }
      return Promise.resolve(undefined);
    });

    const fetchAgents = vi.fn().mockResolvedValue([{ id: 'a1', name: 'Brit Freed' }]);
    const harness = makeAgentsTabHarness(fetchAgents);
    await harness.onAgentsTabActiveOrDepsChanged();
    expect(harness.state().agentsLoaded).toBe(true);

    // No onAgentsChanged prop passed — the bug's pre-fix state.
    const { container } = render(ProviderConnectionsPanel);
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
    await fireEvent.click(screen.getByTestId('imported-agents-done'));
    await tick();
    await tick();

    // agentsLoaded stays true — harness never got reset — proving the gate
    // would silently no-op if the callback weren't wired.
    expect(harness.state().agentsLoaded).toBe(true);
    await harness.onAgentsTabActiveOrDepsChanged();
    expect(fetchAgents).toHaveBeenCalledTimes(1); // no re-fetch happened
  });
});

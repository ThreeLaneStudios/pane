/**
 * NewSessionSheet.test.ts — §36.17.4 (#3050 PR 10, Captain & Crew UI).
 *
 * Replaces AgentPickerPopover.svelte at both call sites (Ruling 1). Covers:
 *   - DEFAULT agent pre-selected as captain; Start always enabled
 *   - Captain-only vs. captain+crew vs. OpenClaw-captain crew-section swap
 *   - Writes crew_agent_ids exactly as chosen (0/1/2/4+ crew)
 *   - Footer summary text (works alone / + N crew / own crew)
 *   - Ruling 2: the DEFAULT CAPTAIN constant renders on the captain chip
 *   - Ruling 3: search narrows BOTH the captain list and the crew chips,
 *     no threshold
 */

import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, fireEvent, within } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import NewSessionSheet from './NewSessionSheet.svelte';
import type { Agent } from '$lib/types';
import { DEFAULT_CAPTAIN_LABEL } from '$lib/constants';

if (typeof (globalThis.window as any).addEventListener !== 'function') {
  (globalThis.window as any).addEventListener = () => {};
}
if (typeof (globalThis.window as any).removeEventListener !== 'function') {
  (globalThis.window as any).removeEventListener = () => {};
}

function mkAgent(overrides: Partial<Agent> = {}): Agent {
  return {
    id: 'a1',
    name: 'Research',
    slug: 'research',
    role: 'sub',
    model: 'claude-sonnet-4-5',
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

afterEach(() => {
  vi.restoreAllMocks();
});

describe('NewSessionSheet — captain pre-selection and Start', () => {
  it('pre-selects the DEFAULT agent as captain, not just the first agent', async () => {
    const agents = [
      mkAgent({ id: 'a1', name: 'Astro', is_default: false }),
      mkAgent({ id: 'a2', name: 'Valencia', is_default: true }),
    ];
    const onConfirm = vi.fn();
    const { container } = render(NewSessionSheet, { agents, onConfirm, onCancel: vi.fn() });

    const selectedRow = container.querySelector('.ns-ag--on');
    expect(selectedRow?.textContent).toContain('Valencia');
  });

  // Mutation: dropped the `?? agents[0]` fallback AND the `is_default` find,
  // replaced with a hardcoded `''`. Result: red — no row carries `.ns-ag--on`
  // at all, and `querySelector('.ns-ag--on')` returns null, failing the
  // assertion above immediately (not just a wrong-agent mismatch). Reverted.

  it('Start session has no disabled attribute at all — always enabled', async () => {
    const agents = [mkAgent({ id: 'a1', name: 'Astro', is_default: true })];
    const { container } = render(NewSessionSheet, { agents, onConfirm: vi.fn(), onCancel: vi.fn() });
    const startBtn = container.querySelector('[data-testid="ns-start-btn"]') as HTMLButtonElement;
    expect(startBtn.disabled).toBe(false);
    expect(startBtn.hasAttribute('disabled')).toBe(false);
  });

  it('Start session stays enabled even with zero crew selected and zero search text', async () => {
    const agents = [mkAgent({ id: 'a1', name: 'Astro', is_default: true }), mkAgent({ id: 'a2', name: 'Vanessa' })];
    const { container } = render(NewSessionSheet, { agents, onConfirm: vi.fn(), onCancel: vi.fn() });
    const startBtn = container.querySelector('[data-testid="ns-start-btn"]') as HTMLButtonElement;
    expect(startBtn.hasAttribute('disabled')).toBe(false);
  });
});

describe('NewSessionSheet — OpenClaw captain swaps the crew section', () => {
  it('crew section is ABSENT from the DOM (not just hidden) when captain is OpenClaw, and the write payload carries an empty crew array', async () => {
    const ocAgent = mkAgent({ id: 'oc-1', name: 'Brit Freed', runtime_ref: 'oc-1', runtime_type: 'openclaw', is_default: true });
    const apiAgent = mkAgent({ id: 'a1', name: 'Vanessa' });
    const onConfirm = vi.fn();
    const { container } = render(NewSessionSheet, { agents: [ocAgent, apiAgent], onConfirm, onCancel: vi.fn() });

    // DOM-presence assertion, not visibility — a display:none crew section
    // would NOT be caught by a toBeVisible()-only check (Reed's scope
    // audit's own caution); querySelector returning null is what proves
    // the section is actually absent.
    expect(container.querySelector('[data-testid="ns-crew-section"]')).toBeNull();
    expect(container.querySelector('[data-testid="ns-crew-section-oc"]')).not.toBeNull();

    const startBtn = container.querySelector('[data-testid="ns-start-btn"]') as HTMLButtonElement;
    await fireEvent.click(startBtn);

    expect(onConfirm).toHaveBeenCalledWith('oc-1', []);
  });

  it('switching captain back to an API agent restores the crew section', async () => {
    const ocAgent = mkAgent({ id: 'oc-1', name: 'Brit Freed', runtime_ref: 'oc-1', runtime_type: 'openclaw', is_default: true });
    const apiAgent = mkAgent({ id: 'a1', name: 'Vanessa' });
    const { container } = render(NewSessionSheet, { agents: [ocAgent, apiAgent], onConfirm: vi.fn(), onCancel: vi.fn() });

    expect(container.querySelector('[data-testid="ns-crew-section"]')).toBeNull();

    const apiRow = Array.from(container.querySelectorAll('.ns-ag')).find(r => r.textContent?.includes('Vanessa')) as HTMLButtonElement;
    await fireEvent.click(apiRow);
    await tick();

    expect(container.querySelector('[data-testid="ns-crew-section"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="ns-crew-section-oc"]')).toBeNull();
  });
});

describe('NewSessionSheet — writes crew_agent_ids exactly as chosen (0/1/2/4+)', () => {
  function setup(crewCount: number) {
    const captain = mkAgent({ id: 'cap-1', name: 'Astro', is_default: true });
    const crewAgents = Array.from({ length: crewCount }, (_, i) => mkAgent({ id: `c${i}`, name: `Crew${i}` }));
    const onConfirm = vi.fn();
    const { container } = render(NewSessionSheet, { agents: [captain, ...crewAgents], onConfirm, onCancel: vi.fn() });
    return { container, onConfirm, crewAgents };
  }

  it('0 crew selected: Start fires with an empty array', async () => {
    const { container, onConfirm } = setup(2);
    const startBtn = container.querySelector('[data-testid="ns-start-btn"]') as HTMLButtonElement;
    await fireEvent.click(startBtn);
    expect(onConfirm).toHaveBeenCalledWith('cap-1', []);
  });

  it('1 crew selected: Start fires with exactly that one id', async () => {
    const { container, onConfirm, crewAgents } = setup(1);
    const chip = Array.from(container.querySelectorAll('.ns-cc')).find(c => c.textContent?.includes(crewAgents[0].name)) as HTMLButtonElement;
    await fireEvent.click(chip);
    await tick();
    const startBtn = container.querySelector('[data-testid="ns-start-btn"]') as HTMLButtonElement;
    await fireEvent.click(startBtn);
    expect(onConfirm).toHaveBeenCalledWith('cap-1', ['c0']);
  });

  it('2 crew selected: Start fires with both ids', async () => {
    const { container, onConfirm, crewAgents } = setup(2);
    for (const a of crewAgents) {
      const chip = Array.from(container.querySelectorAll('.ns-cc')).find(c => c.textContent?.includes(a.name)) as HTMLButtonElement;
      await fireEvent.click(chip);
      await tick();
    }
    const startBtn = container.querySelector('[data-testid="ns-start-btn"]') as HTMLButtonElement;
    await fireEvent.click(startBtn);
    expect(onConfirm).toHaveBeenCalledWith('cap-1', ['c0', 'c1']);
  });

  it('4+ crew selected (selecting all 4): Start fires with all 4 ids — no silent drop past the 3-row visual cap', async () => {
    const { container, onConfirm, crewAgents } = setup(4);
    for (const a of crewAgents) {
      const chip = Array.from(container.querySelectorAll('.ns-cc')).find(c => c.textContent?.includes(a.name)) as HTMLButtonElement;
      await fireEvent.click(chip);
      await tick();
    }
    const startBtn = container.querySelector('[data-testid="ns-start-btn"]') as HTMLButtonElement;
    await fireEvent.click(startBtn);
    expect(onConfirm).toHaveBeenCalledWith('cap-1', ['c0', 'c1', 'c2', 'c3']);
  });
});

describe('NewSessionSheet — footer summary (three exact strings, not a generic template)', () => {
  it('"<Captain> · works alone" when crew is empty', async () => {
    const agents = [mkAgent({ id: 'a1', name: 'Astro', is_default: true })];
    const { container } = render(NewSessionSheet, { agents, onConfirm: vi.fn(), onCancel: vi.fn() });
    expect(container.querySelector('[data-testid="ns-footer-text"]')?.textContent).toBe('Astro · works alone');
  });

  it('"<Captain> + N crew" when N crew selected', async () => {
    const agents = [mkAgent({ id: 'a1', name: 'Astro', is_default: true }), mkAgent({ id: 'c1', name: 'Vanessa' }), mkAgent({ id: 'c2', name: 'Vernon' })];
    const { container } = render(NewSessionSheet, { agents, onConfirm: vi.fn(), onCancel: vi.fn() });
    for (const name of ['Vanessa', 'Vernon']) {
      const chip = Array.from(container.querySelectorAll('.ns-cc')).find(c => c.textContent?.includes(name)) as HTMLButtonElement;
      await fireEvent.click(chip);
      await tick();
    }
    expect(container.querySelector('[data-testid="ns-footer-text"]')?.textContent).toBe('Astro + 2 crew');
  });

  it('"<Captain> · own crew" for an OpenClaw captain', async () => {
    const agents = [mkAgent({ id: 'oc-1', name: 'Brit Freed', runtime_ref: 'oc-1', runtime_type: 'openclaw', is_default: true })];
    const { container } = render(NewSessionSheet, { agents, onConfirm: vi.fn(), onCancel: vi.fn() });
    expect(container.querySelector('[data-testid="ns-footer-text"]')?.textContent).toBe('Brit Freed · own crew');
  });

  // Mutation: replaced the three-branch footerText derivation with a
  // generic template `${name} + ${selectedCrew.size} crew` unconditionally.
  // Result: red — the "works alone" test above failed:
  //   expected 'Astro + 0 crew' to be 'Astro · works alone'
  // Reverted.
});

describe('NewSessionSheet — Ruling 2: DEFAULT CAPTAIN constant on the captain chip', () => {
  it('renders the DEFAULT_CAPTAIN_LABEL constant text next to the default agent, not a hardcoded "Default" string', async () => {
    const agents = [mkAgent({ id: 'a1', name: 'Astro', is_default: true })];
    const { container } = render(NewSessionSheet, { agents, onConfirm: vi.fn(), onCancel: vi.fn() });
    const dfltChip = container.querySelector('.ns-dflt');
    expect(dfltChip?.textContent).toBe(DEFAULT_CAPTAIN_LABEL);
    expect(DEFAULT_CAPTAIN_LABEL).toBe('DEFAULT CAPTAIN');
  });
});

describe('NewSessionSheet — Ruling 3: search narrows BOTH the captain list and the crew chips, no threshold', () => {
  it('typing narrows the captain list', async () => {
    const agents = [
      mkAgent({ id: 'a1', name: 'Astro', is_default: true }),
      mkAgent({ id: 'a2', name: 'Vanessa' }),
    ];
    const { container } = render(NewSessionSheet, { agents, onConfirm: vi.fn(), onCancel: vi.fn() });
    const search = container.querySelector('[data-testid="ns-search"]') as HTMLInputElement;
    await fireEvent.input(search, { target: { value: 'Van' } });
    await tick();

    const aglist = container.querySelector('.ns-aglist') as HTMLElement;
    expect(within(aglist).queryByText('Vanessa')).not.toBeNull();
    expect(within(aglist).queryByText('Astro')).toBeNull();
  });

  it('typing ALSO narrows the crew chip pool — no 12-agent threshold gating it', async () => {
    // Only 3 agents total (well under any "~12" prose number) — proves the
    // filter applies unconditionally, not only past some agent-count gate.
    const captain = mkAgent({ id: 'cap-1', name: 'Astro', is_default: true });
    const crew1 = mkAgent({ id: 'c1', name: 'Vanessa' });
    const crew2 = mkAgent({ id: 'c2', name: 'Vernon' });
    const { container } = render(NewSessionSheet, { agents: [captain, crew1, crew2], onConfirm: vi.fn(), onCancel: vi.fn() });

    const search = container.querySelector('[data-testid="ns-search"]') as HTMLInputElement;
    await fireEvent.input(search, { target: { value: 'Van' } });
    await tick();

    const ccs = container.querySelector('.ns-ccs') as HTMLElement;
    expect(within(ccs).queryByText('Vanessa')).not.toBeNull();
    expect(within(ccs).queryByText('Vernon')).toBeNull();
  });

  // Mutation: gated the crew-pool filter behind `if (agents.length > 12)`
  // (the "real threshold" reading Reed's scope audit flagged as one of two
  // plausible-but-untestable-without-asking interpretations — Matt's
  // Ruling 3 resolved it as "always filters, no threshold"). Result: red —
  // the second test above failed (Vernon still rendered in the 3-agent
  // fixture, since 3 < 12 never triggers the mutated gate). Reverted.
});

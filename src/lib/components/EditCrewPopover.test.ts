/**
 * EditCrewPopover.test.ts — §36.17.4 (#3050 PR 10, Captain & Crew UI).
 *
 * Captain is locked (no toggle affordance at all — proven via a spy, not
 * just visual styling). Save crew calls onSave with the exact toggled set,
 * additions AND removals in the same edit session. "Applies to the next
 * job <Captain> hands off. Running jobs keep their agent." copy is exact.
 *
 * The cross-layer "running job keeps its agent" guarantee itself is proven
 * at the backend (`ai/chat.rs`'s `crew_rw_tests_3050_pr10::
 * test_update_session_crew_does_not_touch_agent_jobs`) — per Reed's scope
 * audit: "no frontend Vitest can prove this on its own," since the real
 * guarantee is the structural absence of any `UPDATE agent_jobs SET
 * agent_id` anywhere in the codebase. This file only proves the FRONTEND
 * half: Save crew calls the right command with the right payload.
 */

import { describe, it, expect, vi } from 'vitest';
import { render, fireEvent } from '@testing-library/svelte/svelte5';
import EditCrewPopover from './EditCrewPopover.svelte';
import type { Agent } from '$lib/types';

if (typeof (globalThis.window as any).addEventListener !== 'function') {
  (globalThis.window as any).addEventListener = () => {};
}
if (typeof (globalThis.window as any).removeEventListener !== 'function') {
  (globalThis.window as any).removeEventListener = () => {};
}

function mkAgent(overrides: Partial<Agent> = {}): Agent {
  return {
    id: 'a1',
    name: 'Agent',
    slug: 'agent',
    role: 'sub',
    model: 'claude-sonnet-4-5',
    is_default: false,
    is_active: true,
    workspace_path: '/tmp/a',
    config_json: '{}',
    runtime_ref: null,
    runtime_type: 'anthropic',
    source: 'local',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    ...overrides,
  };
}

describe('EditCrewPopover — captain is locked', () => {
  it('captain row has no interactive affordance — a click on it never fires any toggle callback', async () => {
    const captain = mkAgent({ id: 'cap-1', name: 'Astro' });
    const crewPool = [mkAgent({ id: 'c1', name: 'Vanessa' })];
    const onSave = vi.fn();
    const { container } = render(EditCrewPopover, {
      captain, crew: [], crewPool, top: 0, left: 0, onSave, onCancel: vi.fn(),
    });

    const capRow = container.querySelector('[data-testid="ecp-captain-row"]') as HTMLElement;
    expect(capRow).not.toBeNull();
    // The captain row is not itself a <button> — proven structurally — AND
    // clicking it must never reach the same toggle/save path a crew chip
    // would. Spy via onSave: click the captain row, then Save, and confirm
    // onSave's payload never includes the captain's own id (it couldn't
    // have been toggled in).
    await fireEvent.click(capRow);
    const saveBtn = container.querySelector('[data-testid="ecp-save-btn"]') as HTMLButtonElement;
    await fireEvent.click(saveBtn);
    expect(onSave).toHaveBeenCalledWith([]);
  });

  it('shows "Captain · can\'t be changed" text', () => {
    const captain = mkAgent({ id: 'cap-1', name: 'Astro' });
    const { container } = render(EditCrewPopover, {
      captain, crew: [], crewPool: [], top: 0, left: 0, onSave: vi.fn(), onCancel: vi.fn(),
    });
    expect(container.querySelector('[data-testid="ecp-captain-row"]')?.textContent).toContain("Captain · can't be changed");
  });
});

describe('EditCrewPopover — Save crew with the exact toggled set', () => {
  it('additions AND removals in the same edit session both reach onSave', async () => {
    const captain = mkAgent({ id: 'cap-1', name: 'Astro' });
    const existingCrew = mkAgent({ id: 'c1', name: 'Vanessa' });
    const newCrew = mkAgent({ id: 'c2', name: 'Vernon' });
    const onSave = vi.fn();
    const { container } = render(EditCrewPopover, {
      captain,
      crew: [existingCrew], // c1 currently selected
      crewPool: [existingCrew, newCrew],
      top: 0, left: 0, onSave, onCancel: vi.fn(),
    });

    // Remove c1 (currently on), add c2 (currently off).
    const chips = Array.from(container.querySelectorAll('.ecp-cc'));
    const c1Chip = chips.find(c => c.textContent?.includes('Vanessa')) as HTMLButtonElement;
    const c2Chip = chips.find(c => c.textContent?.includes('Vernon')) as HTMLButtonElement;
    await fireEvent.click(c1Chip); // remove
    await fireEvent.click(c2Chip); // add

    const saveBtn = container.querySelector('[data-testid="ecp-save-btn"]') as HTMLButtonElement;
    await fireEvent.click(saveBtn);

    expect(onSave).toHaveBeenCalledWith(['c2']);
  });

  // Mutation: Save unioned the original `crew` prop back into the
  // outgoing payload (`Array.from(new Set([...selected, ...crew.map(c =>
  // c.id)]))`), so a removal (toggling an originally-present member OFF)
  // never actually reached onSave. Result: red — the test above expected
  // `['c2']` but got `['c2', 'c1']` (c1's removal silently lost). Reverted.

  it('pre-checks the chips matching the current crew prop', () => {
    const captain = mkAgent({ id: 'cap-1', name: 'Astro' });
    const existingCrew = mkAgent({ id: 'c1', name: 'Vanessa' });
    const { container } = render(EditCrewPopover, {
      captain, crew: [existingCrew], crewPool: [existingCrew], top: 0, left: 0, onSave: vi.fn(), onCancel: vi.fn(),
    });
    const chip = container.querySelector('.ecp-cc') as HTMLElement;
    expect(chip.classList.contains('ecp-cc--on')).toBe(true);
  });
});

describe('EditCrewPopover — exact copy', () => {
  it('shows the exact "Applies to the next job <Captain> hands off. Running jobs keep their agent." text', () => {
    const captain = mkAgent({ id: 'cap-1', name: 'Astro' });
    const { container } = render(EditCrewPopover, {
      captain, crew: [], crewPool: [], top: 0, left: 0, onSave: vi.fn(), onCancel: vi.fn(),
    });
    const note = container.querySelector('[data-testid="ecp-note"]')?.textContent?.replace(/\s+/g, ' ').trim();
    expect(note).toBe('Applies to the next job Astro hands off. Running jobs keep their agent.');
  });
});

describe('EditCrewPopover — Cancel', () => {
  it('Cancel calls onCancel without calling onSave', async () => {
    const captain = mkAgent({ id: 'cap-1', name: 'Astro' });
    const onSave = vi.fn();
    const onCancel = vi.fn();
    const { container } = render(EditCrewPopover, {
      captain, crew: [], crewPool: [], top: 0, left: 0, onSave, onCancel,
    });
    const cancelBtn = Array.from(container.querySelectorAll('button')).find(b => b.textContent === 'Cancel') as HTMLButtonElement;
    await fireEvent.click(cancelBtn);
    expect(onCancel).toHaveBeenCalled();
    expect(onSave).not.toHaveBeenCalled();
  });

  it('clicking the backdrop calls onCancel', async () => {
    const captain = mkAgent({ id: 'cap-1', name: 'Astro' });
    const onCancel = vi.fn();
    const { container } = render(EditCrewPopover, {
      captain, crew: [], crewPool: [], top: 0, left: 0, onSave: vi.fn(), onCancel,
    });
    const backdrop = container.querySelector('[data-testid="ecp-backdrop"]') as HTMLElement;
    await fireEvent.click(backdrop);
    expect(onCancel).toHaveBeenCalled();
  });
});

/**
 * ImportedAgentsModal.test.ts — Component tests for ImportedAgentsModal.svelte
 *
 * #2635 (D11 fix): shown after ProviderConnectionCard.svelte's completePairing()
 * calls sync_initial_import and gets back a non-empty agent list. See the
 * component's module comment for why this exists (Settings-context equivalent of
 * Onboarding.svelte's full-screen "Your Agents" review step).
 *
 * Test groups:
 *  A — Renders title/subtitle and every agent name passed in
 *  B — Singular vs plural title wording
 *  C — Done button and scrim click both call onDismiss
 *
 * Note: Escape-to-close (<svelte:window onkeydown>) is not covered here — this
 * suite's vitest-setup.ts window shim stubs addEventListener as a no-op (needed
 * just to let mount/unmount succeed), so no keydown listener is ever actually
 * registered against a dispatchable target in this harness. No other modal test
 * file in this codebase (DefaultAgentPickerModal.test.ts, AgentPickerModal — no
 * test file at all) exercises Escape for the same reason; scrim-click and Done
 * button cover the two dismiss paths that this harness *can* verify.
 */

import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte/svelte5';
import ImportedAgentsModal from './ImportedAgentsModal.svelte';

// vitest-setup.ts replaces `globalThis.window` with a plain mock object (Tauri
// internals shim) that lacks addEventListener/removeEventListener. This modal uses
// <svelte:window onkeydown={...}> for Escape-to-close — same patch as
// DefaultAgentPickerModal.test.ts / ProviderConnectionCard.test.ts.
if (typeof (globalThis.window as any).addEventListener !== 'function') {
  (globalThis.window as any).addEventListener = () => {};
}
if (typeof (globalThis.window as any).removeEventListener !== 'function') {
  (globalThis.window as any).removeEventListener = () => {};
}

afterEach(() => {
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// A — Renders title/subtitle and agent names
// ---------------------------------------------------------------------------

describe('A — renders agent list', () => {
  it('renders every agent name passed in', () => {
    const { container } = render(ImportedAgentsModal, {
      props: { agentNames: ['Stia', 'Coder'], onDismiss: vi.fn() },
    });
    expect(screen.getByText('Stia')).not.toBeNull();
    expect(screen.getByText('Coder')).not.toBeNull();
    expect(container.querySelectorAll('.iam-row').length).toBe(2);
  });

  it('renders as a dialog with aria-modal', () => {
    const { container } = render(ImportedAgentsModal, {
      props: { agentNames: ['Stia'], onDismiss: vi.fn() },
    });
    const dialog = container.querySelector('[role="dialog"]');
    expect(dialog).not.toBeNull();
    expect(dialog?.getAttribute('aria-modal')).toBe('true');
  });
});

// ---------------------------------------------------------------------------
// B — Singular vs plural title wording
// ---------------------------------------------------------------------------

describe('B — title wording', () => {
  it('uses singular wording for exactly one agent', () => {
    render(ImportedAgentsModal, { props: { agentNames: ['Stia'], onDismiss: vi.fn() } });
    expect(screen.getByText('Agent imported')).not.toBeNull();
  });

  it('uses plural wording with a count for more than one agent', () => {
    render(ImportedAgentsModal, { props: { agentNames: ['Stia', 'Coder', 'Reed'], onDismiss: vi.fn() } });
    expect(screen.getByText('3 agents imported')).not.toBeNull();
  });
});

// ---------------------------------------------------------------------------
// C — Dismissal
// ---------------------------------------------------------------------------

describe('C — dismissal', () => {
  it('Done button calls onDismiss', async () => {
    const onDismiss = vi.fn();
    render(ImportedAgentsModal, { props: { agentNames: ['Stia'], onDismiss } });
    await fireEvent.click(screen.getByTestId('imported-agents-done'));
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });

  it('clicking the scrim (outside the modal) calls onDismiss', async () => {
    const onDismiss = vi.fn();
    const { container } = render(ImportedAgentsModal, { props: { agentNames: ['Stia'], onDismiss } });
    const scrim = container.querySelector('.iam-scrim') as HTMLElement;
    await fireEvent.click(scrim);
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });

  it('clicking inside the modal body does not call onDismiss', async () => {
    const onDismiss = vi.fn();
    const { container } = render(ImportedAgentsModal, { props: { agentNames: ['Stia'], onDismiss } });
    const modal = container.querySelector('.iam-modal') as HTMLElement;
    await fireEvent.click(modal);
    expect(onDismiss).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// D — #2647 D17 fix: model soft-fail warning
// ---------------------------------------------------------------------------

describe('D — agentsMissingModel soft-fail warning', () => {
  it('does not render a warning when agentsMissingModel is omitted (default [])', () => {
    const { container } = render(ImportedAgentsModal, {
      props: { agentNames: ['Stia'], onDismiss: vi.fn() },
    });
    expect(container.querySelector('[data-testid="imported-agents-missing-model-warning"]')).toBeNull();
  });

  it('does not render a warning when agentsMissingModel is an empty array', () => {
    const { container } = render(ImportedAgentsModal, {
      props: { agentNames: ['Stia'], agentsMissingModel: [], onDismiss: vi.fn() },
    });
    expect(container.querySelector('[data-testid="imported-agents-missing-model-warning"]')).toBeNull();
  });

  it('renders a singular, name-bearing warning for exactly one agent missing a model', () => {
    render(ImportedAgentsModal, {
      props: { agentNames: ['Stia'], agentsMissingModel: ['Stia'], onDismiss: vi.fn() },
    });
    const warning = screen.getByTestId('imported-agents-missing-model-warning');
    expect(warning.textContent).toContain('Agent "Stia" imported without a model');
    expect(warning.textContent).toContain('Settings');
  });

  it('renders a plural, counted warning for more than one agent missing a model', () => {
    render(ImportedAgentsModal, {
      props: { agentNames: ['Stia', 'Coder'], agentsMissingModel: ['Stia', 'Coder'], onDismiss: vi.fn() },
    });
    const warning = screen.getByTestId('imported-agents-missing-model-warning');
    expect(warning.textContent).toContain('2 agents imported without a model');
  });
});

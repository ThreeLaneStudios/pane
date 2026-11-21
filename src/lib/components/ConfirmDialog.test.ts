/**
 * ConfirmDialog.test.ts — Component tests for the shared confirmation modal
 * (#2892 drive-by; ConfirmDialog itself pre-dates this migration, #542).
 *
 * ConfirmDialog replaces native `window.confirm()` calls, which resolve
 * `false` in Tauri's WebView (Apple's documented default for an
 * unimplemented WKUIDelegate method). Same shape as PromptDialog.test.ts,
 * minus the text-input cases (ConfirmDialog has no input, just title/message
 * + confirm/cancel).
 */

import { describe, it, expect, vi } from 'vitest';
import { render, fireEvent, screen } from '@testing-library/svelte/svelte5';
import ConfirmDialog from './ConfirmDialog.svelte';

// vitest-setup.ts replaces `globalThis.window` with a plain mock object (Tauri
// internals shim) that lacks a real addEventListener/removeEventListener pub-sub
// implementation. ConfirmDialog uses <svelte:window onkeydown={...}> for
// Escape-to-cancel, so the DOM listener API needs to exist on that mock window
// for mount/unmount to succeed (same patch pattern as PromptDialog.test.ts).
// Since the stub is a no-op rather than a real listener registry, we
// additionally capture registered keydown handlers so the Escape test below
// can invoke them directly.
const windowKeydownHandlers = new Set<(e: KeyboardEvent) => void>();
(globalThis.window as any).addEventListener = (type: string, handler: any) => {
  if (type === 'keydown') windowKeydownHandlers.add(handler);
};
(globalThis.window as any).removeEventListener = (type: string, handler: any) => {
  if (type === 'keydown') windowKeydownHandlers.delete(handler);
};

describe('ConfirmDialog', () => {
  it('renders title and message', () => {
    render(ConfirmDialog, {
      props: {
        title: 'Delete Project',
        message: 'This will permanently delete the project and all its notes.',
        onConfirm: vi.fn(),
        onCancel: vi.fn(),
      },
    });

    expect(screen.getByText('Delete Project')).toBeTruthy();
    expect(
      screen.getByText('This will permanently delete the project and all its notes.')
    ).toBeTruthy();
  });

  it('renders default confirmLabel "Delete" and default confirmColor "#EF4444" when not provided', () => {
    const { container } = render(ConfirmDialog, {
      props: {
        title: 'Delete Project',
        message: 'Are you sure?',
        onConfirm: vi.fn(),
        onCancel: vi.fn(),
      },
    });

    const actionBtn = container.querySelector('.action-btn') as HTMLButtonElement;
    expect(actionBtn.textContent?.trim()).toBe('Delete');
    expect(actionBtn.style.background).toBe('rgb(239, 68, 68)');
  });

  it('renders custom confirmLabel and confirmColor when provided', () => {
    const { container } = render(ConfirmDialog, {
      props: {
        title: 'Disconnect Anthropic',
        message: 'This will deactivate all agents on this provider.',
        confirmLabel: 'Disconnect',
        confirmColor: '#EF4444',
        onConfirm: vi.fn(),
        onCancel: vi.fn(),
      },
    });

    const actionBtn = container.querySelector('.action-btn') as HTMLButtonElement;
    expect(actionBtn.textContent?.trim()).toBe('Disconnect');
    expect(actionBtn.style.background).toBe('rgb(239, 68, 68)');
  });

  it('clicking the confirm button calls onConfirm', async () => {
    const onConfirm = vi.fn();
    const { container } = render(ConfirmDialog, {
      props: {
        title: 'Delete Project',
        message: 'Are you sure?',
        onConfirm,
        onCancel: vi.fn(),
      },
    });

    const actionBtn = container.querySelector('.action-btn') as HTMLButtonElement;
    await fireEvent.click(actionBtn);

    expect(onConfirm).toHaveBeenCalled();
  });

  it('clicking the cancel button calls onCancel, not onConfirm', async () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    const { container } = render(ConfirmDialog, {
      props: {
        title: 'Delete Project',
        message: 'Are you sure?',
        onConfirm,
        onCancel,
      },
    });

    const cancelBtn = container.querySelector('.cancel-btn') as HTMLButtonElement;
    await fireEvent.click(cancelBtn);

    expect(onCancel).toHaveBeenCalled();
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it('overlay click calls onCancel', async () => {
    const onCancel = vi.fn();
    const { container } = render(ConfirmDialog, {
      props: {
        title: 'Delete Project',
        message: 'Are you sure?',
        onConfirm: vi.fn(),
        onCancel,
      },
    });

    const overlay = container.querySelector('.confirm-overlay') as HTMLElement;
    await fireEvent.click(overlay);

    expect(onCancel).toHaveBeenCalled();
  });

  it('Escape key calls onCancel', async () => {
    const onCancel = vi.fn();
    windowKeydownHandlers.clear();
    render(ConfirmDialog, {
      props: {
        title: 'Delete Project',
        message: 'Are you sure?',
        onConfirm: vi.fn(),
        onCancel,
      },
    });

    expect(windowKeydownHandlers.size).toBeGreaterThan(0);
    // Svelte's internal event dispatcher (handle_event_propagation) walks
    // event.composedPath()/event.target before invoking the user handler, so
    // the synthetic event needs those fields present (even though nothing in
    // this mocked window is a real DOM node to propagate through).
    const fakeEscapeEvent = {
      key: 'Escape',
      target: null,
      composedPath: () => [],
      cancelBubble: false,
    } as unknown as KeyboardEvent;
    for (const handler of windowKeydownHandlers) {
      handler(fakeEscapeEvent);
    }
    expect(onCancel).toHaveBeenCalled();
  });

  // #2874 item 65 ruling 2: folder-delete errors must be visible in the UI,
  // not console-only. `error` is an additive, optional prop — omitted it
  // renders nothing extra (every pre-existing caller is unaffected).
  it('renders no error text when `error` is omitted', () => {
    const { container } = render(ConfirmDialog, {
      props: {
        title: 'Delete Project',
        message: 'Are you sure?',
        onConfirm: vi.fn(),
        onCancel: vi.fn(),
      },
    });

    expect(container.querySelector('.confirm-error')).toBeNull();
  });

  it('renders the error message visibly when `error` is set', () => {
    render(ConfirmDialog, {
      props: {
        title: 'Delete "Inbox"?',
        message: 'This will delete the folder.',
        error: 'Cannot delete the default folder',
        onConfirm: vi.fn(),
        onCancel: vi.fn(),
      },
    });

    expect(screen.getByText('Cannot delete the default folder')).toBeTruthy();
  });
});

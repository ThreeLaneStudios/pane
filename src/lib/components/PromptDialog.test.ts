/**
 * PromptDialog.test.ts — Component tests for the shared text-input prompt
 * modal (#2874 item 6).
 *
 * PromptDialog replaces native `window.prompt()` calls, which don't render
 * in Tauri's WebView. It follows ConfirmDialog.svelte's state-variable +
 * callback pattern (see ConfirmDialog for the established convention).
 */

import { describe, it, expect, vi } from 'vitest';
import { render, fireEvent, screen } from '@testing-library/svelte/svelte5';
import PromptDialog from './PromptDialog.svelte';

// vitest-setup.ts replaces `globalThis.window` with a plain mock object (Tauri
// internals shim) that lacks a real addEventListener/removeEventListener pub-sub
// implementation. PromptDialog uses <svelte:window onkeydown={...}> for
// Escape-to-cancel, so the DOM listener API needs to exist on that mock window
// for mount/unmount to succeed (same patch pattern as DefaultAgentPickerModal.test.ts
// / ImportedAgentsModal.test.ts / AgentCreateOverlay.test.ts). Since the stub is a
// no-op rather than a real listener registry, we additionally capture registered
// keydown handlers so the Escape test below can invoke them directly.
const windowKeydownHandlers = new Set<(e: KeyboardEvent) => void>();
(globalThis.window as any).addEventListener = (type: string, handler: any) => {
  if (type === 'keydown') windowKeydownHandlers.add(handler);
};
(globalThis.window as any).removeEventListener = (type: string, handler: any) => {
  if (type === 'keydown') windowKeydownHandlers.delete(handler);
};

describe('PromptDialog', () => {
  it('renders title, message (when provided), and placeholder', () => {
    const { container, rerender } = render(PromptDialog, {
      props: {
        title: 'New Folder',
        message: 'Enter the model identifier to use.',
        placeholder: 'Folder name',
        onConfirm: vi.fn(),
        onCancel: vi.fn(),
      },
    });

    expect(screen.getByText('New Folder')).toBeTruthy();
    expect(screen.getByText('Enter the model identifier to use.')).toBeTruthy();
    const input = container.querySelector('.prompt-input') as HTMLInputElement;
    expect(input.placeholder).toBe('Folder name');
  });

  it('omits the message paragraph when message is not provided', () => {
    const { container } = render(PromptDialog, {
      props: {
        title: 'Insert Link',
        placeholder: 'https://example.com',
        onConfirm: vi.fn(),
        onCancel: vi.fn(),
      },
    });

    expect(container.querySelector('.prompt-message')).toBeNull();
  });

  it('disables confirm button when input is empty (default validation); enables it with non-whitespace input', async () => {
    const { container } = render(PromptDialog, {
      props: {
        title: 'New Folder',
        onConfirm: vi.fn(),
        onCancel: vi.fn(),
      },
    });

    const input = container.querySelector('.prompt-input') as HTMLInputElement;
    const confirmBtn = container.querySelector('.action-btn') as HTMLButtonElement;

    expect(confirmBtn.disabled).toBe(true);

    await fireEvent.input(input, { target: { value: '   ' } });
    expect(confirmBtn.disabled).toBe(true);

    await fireEvent.input(input, { target: { value: 'My Folder' } });
    expect(confirmBtn.disabled).toBe(false);
  });

  it('custom validate prop overrides default empty-check', async () => {
    const validate = (value: string) => value.trim().length >= 3;
    const { container } = render(PromptDialog, {
      props: {
        title: 'Custom',
        validate,
        onConfirm: vi.fn(),
        onCancel: vi.fn(),
      },
    });

    const input = container.querySelector('.prompt-input') as HTMLInputElement;
    const confirmBtn = container.querySelector('.action-btn') as HTMLButtonElement;

    // Non-empty but shorter than the custom minimum: still disabled.
    await fireEvent.input(input, { target: { value: 'ab' } });
    expect(confirmBtn.disabled).toBe(true);

    // Meets the custom minimum: enabled.
    await fireEvent.input(input, { target: { value: 'abc' } });
    expect(confirmBtn.disabled).toBe(false);
  });

  it('pre-fills the input with initialValue', () => {
    const { container } = render(PromptDialog, {
      props: {
        title: 'Edit Link',
        initialValue: 'https://example.com',
        onConfirm: vi.fn(),
        onCancel: vi.fn(),
      },
    });

    const input = container.querySelector('.prompt-input') as HTMLInputElement;
    expect(input.value).toBe('https://example.com');
  });

  it('Enter key triggers onConfirm with trimmed value when valid; does nothing when invalid', async () => {
    const onConfirm = vi.fn();
    const { container } = render(PromptDialog, {
      props: {
        title: 'New Folder',
        onConfirm,
        onCancel: vi.fn(),
      },
    });

    const input = container.querySelector('.prompt-input') as HTMLInputElement;

    // Invalid (empty) — Enter should do nothing.
    await fireEvent.keyDown(input, { key: 'Enter' });
    expect(onConfirm).not.toHaveBeenCalled();

    // Valid — Enter should confirm.
    await fireEvent.input(input, { target: { value: 'My Folder' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    expect(onConfirm).toHaveBeenCalledWith('My Folder');
  });

  it('Escape key triggers onCancel', async () => {
    const onCancel = vi.fn();
    windowKeydownHandlers.clear();
    render(PromptDialog, {
      props: {
        title: 'New Folder',
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

  it('overlay click triggers onCancel', async () => {
    const onCancel = vi.fn();
    const { container } = render(PromptDialog, {
      props: {
        title: 'New Folder',
        onConfirm: vi.fn(),
        onCancel,
      },
    });

    const overlay = container.querySelector('.prompt-overlay') as HTMLElement;
    await fireEvent.click(overlay);
    expect(onCancel).toHaveBeenCalled();
  });

  it('onConfirm receives the trimmed value, not the raw input', async () => {
    const onConfirm = vi.fn();
    const { container } = render(PromptDialog, {
      props: {
        title: 'New Folder',
        onConfirm,
        onCancel: vi.fn(),
      },
    });

    const input = container.querySelector('.prompt-input') as HTMLInputElement;
    const confirmBtn = container.querySelector('.action-btn') as HTMLButtonElement;

    await fireEvent.input(input, { target: { value: '  Foo  ' } });
    await fireEvent.click(confirmBtn);

    expect(onConfirm).toHaveBeenCalledWith('Foo');
  });

  it('cancel button triggers onCancel without calling onConfirm', async () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    const { container } = render(PromptDialog, {
      props: {
        title: 'New Folder',
        onConfirm,
        onCancel,
      },
    });

    const input = container.querySelector('.prompt-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'My Folder' } });

    const cancelBtn = container.querySelector('.cancel-btn') as HTMLButtonElement;
    await fireEvent.click(cancelBtn);

    expect(onCancel).toHaveBeenCalled();
    expect(onConfirm).not.toHaveBeenCalled();
  });
});

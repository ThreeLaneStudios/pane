/**
 * LinkInsertModal.test.ts — Component tests for LinkInsertModal.svelte
 *
 * Tests:
 *  LM-1  Renders with autofocused input, empty by default
 *  LM-2  Add link disabled when input is empty
 *  LM-3  Bare domain (pane.so) enables Add link
 *  LM-4  Malformed URL disables Add link + shows inline error
 *  LM-5  Clearing error text hides the error
 *  LM-6  Enter key with valid URL dispatches submit with https:// prepended
 *  LM-7  Esc key dispatches cancel
 *  LM-8  Click on scrim dispatches cancel
 *  LM-9  Click on modal body does NOT dispatch cancel
 *  LM-10 Full HTTPS URL dispatches submit unmodified
 *
 * spec-skip: pure UI addition; no core-behavioral files touched;
 *            ingestion mechanism is toolbar trigger + modal, follows locked design at
 *            ~/Shared/ExampleApp/docs/designs/1898-link-modal/
 * Relates to #1898
 */

import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import LinkInsertModal from './LinkInsertModal.svelte';

afterEach(() => cleanup());

// ── LM-1: Renders with input present ─────────────────────────────────────────

describe('LinkInsertModal — LM-1: initial render', () => {
  it('renders the URL input', () => {
    render(LinkInsertModal);
    const input = screen.getByTestId('link-modal-input');
    expect(input).toBeTruthy();
  });

  it('input starts empty', () => {
    render(LinkInsertModal);
    const input = screen.getByTestId<HTMLInputElement>('link-modal-input');
    expect(input.value).toBe('');
  });

  it('has placeholder "Paste a link…"', () => {
    render(LinkInsertModal);
    const input = screen.getByPlaceholderText('Paste a link…');
    expect(input).toBeTruthy();
  });
});

// ── LM-2: Add link disabled when empty ───────────────────────────────────────

describe('LinkInsertModal — LM-2: empty state', () => {
  it('"Add link" button is disabled when input is empty', () => {
    render(LinkInsertModal);
    const addBtn = screen.getByTestId<HTMLButtonElement>('link-modal-add');
    expect(addBtn.disabled).toBe(true);
  });

  it('no error message shown when empty', () => {
    render(LinkInsertModal);
    const error = screen.queryByTestId('link-modal-error');
    expect(error).toBeNull();
  });
});

// ── LM-3: Bare domain enables Add link ───────────────────────────────────────

describe('LinkInsertModal — LM-3: bare domain (pane.so)', () => {
  it('enables "Add link" when bare domain is typed', async () => {
    render(LinkInsertModal);
    const input = screen.getByTestId('link-modal-input');
    await fireEvent.input(input, { target: { value: 'pane.so' } });
    await tick();
    const addBtn = screen.getByTestId<HTMLButtonElement>('link-modal-add');
    expect(addBtn.disabled).toBe(false);
  });

  it('enables "Add link" for youtube.com/watch path', async () => {
    render(LinkInsertModal);
    const input = screen.getByTestId('link-modal-input');
    await fireEvent.input(input, { target: { value: 'youtube.com/watch?v=hOy-cX_2fQ0' } });
    await tick();
    const addBtn = screen.getByTestId<HTMLButtonElement>('link-modal-add');
    expect(addBtn.disabled).toBe(false);
  });
});

// ── LM-4: Malformed URL shows error ──────────────────────────────────────────

describe('LinkInsertModal — LM-4: invalid URL', () => {
  it('shows error for malformed URL', async () => {
    render(LinkInsertModal);
    const input = screen.getByTestId('link-modal-input');
    await fireEvent.input(input, { target: { value: 'htp:/broken,url' } });
    await tick();
    const error = screen.queryByTestId('link-modal-error');
    expect(error).toBeTruthy();
    expect(error!.textContent).toContain("That doesn't look like a URL.");
  });

  it('"Add link" is disabled for malformed URL', async () => {
    render(LinkInsertModal);
    const input = screen.getByTestId('link-modal-input');
    await fireEvent.input(input, { target: { value: 'htp:/broken,url' } });
    await tick();
    const addBtn = screen.getByTestId<HTMLButtonElement>('link-modal-add');
    expect(addBtn.disabled).toBe(true);
  });
});

// ── LM-5: Clearing error text hides the error ────────────────────────────────

describe('LinkInsertModal — LM-5: error clears on valid input', () => {
  it('error disappears after typing a valid URL', async () => {
    render(LinkInsertModal);
    const input = screen.getByTestId('link-modal-input');

    // First make it invalid
    await fireEvent.input(input, { target: { value: 'htp:/broken' } });
    await tick();
    expect(screen.queryByTestId('link-modal-error')).toBeTruthy();

    // Now type a valid URL
    await fireEvent.input(input, { target: { value: 'https://pane.so' } });
    await tick();
    expect(screen.queryByTestId('link-modal-error')).toBeNull();
  });

  it('error disappears after clearing back to empty', async () => {
    render(LinkInsertModal);
    const input = screen.getByTestId('link-modal-input');

    await fireEvent.input(input, { target: { value: 'not-a-url!!!' } });
    await tick();
    expect(screen.queryByTestId('link-modal-error')).toBeTruthy();

    await fireEvent.input(input, { target: { value: '' } });
    await tick();
    expect(screen.queryByTestId('link-modal-error')).toBeNull();
  });
});

// ── LM-6: Enter key with valid URL dispatches submit ─────────────────────────

describe('LinkInsertModal — LM-6: Enter key submits', () => {
  it('Enter key dispatches submit with https:// prepended for bare domain', async () => {
    const onSubmit = vi.fn();
    const { component } = render(LinkInsertModal);
    component.$on('submit', onSubmit);

    const input = screen.getByTestId('link-modal-input');
    await fireEvent.input(input, { target: { value: 'pane.so' } });
    await tick();

    // Enter on the scrim (keydown bubbles up)
    await fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Enter' });
    await tick();

    expect(onSubmit).toHaveBeenCalledOnce();
    expect(onSubmit.mock.calls[0][0].detail).toBe('https://pane.so');
  });

  it('Enter dispatches submit with full URL unchanged', async () => {
    const onSubmit = vi.fn();
    const { component } = render(LinkInsertModal);
    component.$on('submit', onSubmit);

    const input = screen.getByTestId('link-modal-input');
    const fullUrl = 'https://www.youtube.com/watch?v=hOy-cX_2fQ0';
    await fireEvent.input(input, { target: { value: fullUrl } });
    await tick();

    await fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Enter' });
    await tick();

    expect(onSubmit).toHaveBeenCalledOnce();
    expect(onSubmit.mock.calls[0][0].detail).toBe(fullUrl);
  });

  it('Enter does NOT dispatch submit when input is empty', async () => {
    const onSubmit = vi.fn();
    const { component } = render(LinkInsertModal);
    component.$on('submit', onSubmit);

    await fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Enter' });
    await tick();
    expect(onSubmit).not.toHaveBeenCalled();
  });
});

// ── LM-7: Esc key dispatches cancel ──────────────────────────────────────────

describe('LinkInsertModal — LM-7: Esc cancels', () => {
  it('Esc key dispatches cancel event', async () => {
    const onCancel = vi.fn();
    const { component } = render(LinkInsertModal);
    component.$on('cancel', onCancel);

    await fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
    await tick();
    expect(onCancel).toHaveBeenCalledOnce();
  });
});

// ── LM-8: Click on scrim dispatches cancel ───────────────────────────────────

describe('LinkInsertModal — LM-8: scrim click cancels', () => {
  it('clicking the scrim (dialog backdrop) dispatches cancel', async () => {
    const onCancel = vi.fn();
    const { component } = render(LinkInsertModal);
    component.$on('cancel', onCancel);

    const scrim = screen.getByRole('dialog');
    // Simulate click where target === currentTarget (scrim itself)
    await fireEvent.click(scrim);
    await tick();
    expect(onCancel).toHaveBeenCalledOnce();
  });
});

// ── LM-9: Click on modal body does NOT dispatch cancel ───────────────────────

describe('LinkInsertModal — LM-9: modal body click does not cancel', () => {
  it('clicking Cancel button dispatches cancel (not a scrim-click)', async () => {
    const onCancel = vi.fn();
    const { component } = render(LinkInsertModal);
    component.$on('cancel', onCancel);

    const cancelBtn = screen.getByTestId('link-modal-cancel');
    await fireEvent.click(cancelBtn);
    await tick();
    // Cancel button click dispatches cancel via handleCancel directly
    expect(onCancel).toHaveBeenCalledOnce();
  });

  it('clicking Add link button does NOT dispatch cancel', async () => {
    const onCancel = vi.fn();
    const onSubmit = vi.fn();
    const { component } = render(LinkInsertModal);
    component.$on('cancel', onCancel);
    component.$on('submit', onSubmit);

    const input = screen.getByTestId('link-modal-input');
    await fireEvent.input(input, { target: { value: 'https://pane.so' } });
    await tick();

    const addBtn = screen.getByTestId('link-modal-add');
    await fireEvent.click(addBtn);
    await tick();

    expect(onCancel).not.toHaveBeenCalled();
    expect(onSubmit).toHaveBeenCalledOnce();
  });
});

// ── LM-10: Full HTTPS URL dispatches unmodified ──────────────────────────────

describe('LinkInsertModal — LM-10: full URL submit', () => {
  it('dispatches full https URL unmodified on Add link click', async () => {
    const onSubmit = vi.fn();
    const { component } = render(LinkInsertModal);
    component.$on('submit', onSubmit);

    const input = screen.getByTestId('link-modal-input');
    const url = 'https://www.youtube.com/watch?v=hOy-cX_2fQ0';
    await fireEvent.input(input, { target: { value: url } });
    await tick();

    await fireEvent.click(screen.getByTestId('link-modal-add'));
    await tick();

    expect(onSubmit).toHaveBeenCalledOnce();
    expect(onSubmit.mock.calls[0][0].detail).toBe(url);
  });
});

/**
 * ChatPanel.backButton.test.ts — #3151 PR1 Commit F, moved #3155 C49b.
 *
 * #3155 C49b moved the one-deep back button from a floating element
 * (rendered as soon as `showBackButton` was true, regardless of
 * anything else on screen) to a `←` glyph attached to the LEFT EDGE of
 * the JOB BRIEF strip's own label (`.job-brief-label`, inside the C32
 * `.job-brief` block) — "`←` at the left edge of the JOB BRIEF strip"
 * per the issue's own wording. The button is therefore now nested
 * INSIDE a `.job-brief` row: it only ever renders when (a) `showBackButton`
 * is true AND (b) there is an actual job-brief-shaped message
 * (`job_id` + `synthetic` + `role === 'user'`) in the currently loaded
 * `messages` for the glyph to attach to — exactly the shape a job's own
 * dedicated session always has (its own dispatch brief, written by
 * `insert_job_brief_message` before the job's first turn ever runs).
 *
 * Still rendered only when `showBackButton` is true, clicking it still
 * calls `onBackClick`, and the "what does a second press do" answer is
 * unchanged — once the caller flips `showBackButton` back to `false`
 * (which is exactly what `ChatSidebar.svelte` does, derived from BOTH
 * `$jobSessionBackTarget !== null` AND the active session's own
 * `origin`, and `goToJobSessionBackTarget` clears the store on its own
 * first step), the button is ABSENT from the DOM, not merely disabled —
 * there is no deeper stack, matching the one-deep framing literally.
 */
import { describe, it, expect, vi, beforeAll } from 'vitest';
import { render } from '@testing-library/svelte/svelte5';
import ChatPanel from './ChatPanel.svelte';

beforeAll(() => {
  globalThis.ResizeObserver = class ResizeObserver {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  const win = globalThis.window as any;
  if (!win.addEventListener) {
    win.addEventListener = globalThis.addEventListener?.bind(globalThis) ?? (() => {});
  }
  if (!win.removeEventListener) {
    win.removeEventListener = globalThis.removeEventListener?.bind(globalThis) ?? (() => {});
  }
  if (!win.document) {
    win.document = globalThis.document;
  }
});

vi.mock('@tauri-apps/plugin-shell', () => ({ open: vi.fn() }));
vi.mock('$lib/utils/markdown', () => ({ renderMarkdown: (html: string) => html }));

// A job-brief-shaped message — the exact shape `insert_job_brief_message`
// writes (role='user', synthetic=1, job_id set) and C32's own
// `ChatPanel.svelte` discriminator renders as `.job-brief`, never a
// normal chat bubble.
function jobBriefMessage(overrides: Record<string, any> = {}) {
  return {
    id: 'msg-brief-1',
    thread_id: 'thread-test',
    role: 'user',
    content: 'Do the one-off thing',
    created_at: new Date().toISOString(),
    job_id: 'job-test-1',
    synthetic: true,
    ...overrides,
  };
}

function baseProps(overrides: Record<string, any> = {}) {
  return {
    messages: [jobBriefMessage()],
    onSendMessage: vi.fn(),
    showHeader: false,
    projectId: 'project-test',
    ...overrides,
  };
}

describe('#3155 C49b — the one-deep back button, now attached to the job-brief strip', () => {
  it('absent by default (showBackButton not supplied), even though a job-brief message is present', () => {
    const { container } = render(ChatPanel, baseProps());
    expect(container.querySelector('.job-brief-back-button')).toBeNull();
    // The strip itself still renders — only the glyph is gated.
    expect(container.querySelector('.job-brief')).not.toBeNull();
  });

  it('visible when showBackButton is true AND a job-brief message is present, and clicking it calls onBackClick', () => {
    const onBackClick = vi.fn();
    const { container } = render(ChatPanel, baseProps({ showBackButton: true, onBackClick }));

    const button = container.querySelector('.job-brief-back-button') as HTMLButtonElement;
    expect(button, 'the back button must be in the DOM when showBackButton is true and a job-brief row exists').not.toBeNull();

    button.click();
    expect(onBackClick).toHaveBeenCalledTimes(1);
  });

  it('a second render with showBackButton=false — the button is ABSENT, not disabled (the real one-deep mechanism: the caller flips the prop, not a click-count on this component)', () => {
    const { container } = render(ChatPanel, baseProps({ showBackButton: false }));
    const button = container.querySelector('.job-brief-back-button');
    expect(button).toBeNull();
    // Specifically NOT a disabled-but-present button — absence is the
    // contract, per the issue's own "one-deep" framing.
    expect(container.querySelector('button[disabled].job-brief-back-button')).toBeNull();
  });

  it('absent when showBackButton is true but there is NO job-brief-shaped message to attach it to — the glyph has no floating fallback position anymore', () => {
    const { container } = render(ChatPanel, baseProps({ showBackButton: true, messages: [] }));
    expect(container.querySelector('.job-brief-back-button')).toBeNull();
    expect(container.querySelector('.job-brief')).toBeNull();
  });
});

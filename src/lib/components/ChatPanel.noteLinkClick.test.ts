/**
 * ChatPanel.noteLinkClick.test.ts — #3151 PR1 Commit D
 *
 * `output_link_token` (`ai/tools.rs`) appends `[title](pane://note/{id})`
 * to a job turn's own-session reply and the origin receipt;
 * `markdown.ts`'s `renderMarkdown` renders it as a real `<a href="pane://
 * note/{id}">` anchor (proven separately, `markdown.test.ts`). This file
 * proves the THIRD leg: clicking that anchor inside a real, mounted
 * `ChatPanel` calls the new `onNoteLinkClick` callback with the note id —
 * and does NOT fall through to `handleLinkClick`'s existing http(s)-only
 * "open externally" branch, which is exactly the "a link that opens the
 * wrong [thing]" failure mode this sprint's own task warns is this
 * sprint's visible-and-wrong failure mode, not an invisible one.
 *
 * `renderMarkdown` is mocked to identity (same convention as
 * `ChatPanel.navError.test.ts`) — message content is supplied as
 * already-rendered HTML directly, since this test is about the CLICK
 * HANDLER, not the renderer (that's `markdown.test.ts`'s job).
 */
import { describe, it, expect, vi, beforeAll } from 'vitest';
import { render } from '@testing-library/svelte/svelte5';
import ChatPanel from './ChatPanel.svelte';
import { open as openMock } from '@tauri-apps/plugin-shell';

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

vi.mock('@tauri-apps/plugin-shell', () => ({
  open: vi.fn(),
}));

vi.mock('$lib/utils/markdown', () => ({
  renderMarkdown: (html: string) => html,
}));

function baseProps(overrides: Record<string, any> = {}) {
  return {
    messages: [
      {
        id: 'msg-note-link',
        role: 'assistant',
        content: '<a href="pane://note/note-abc-123">Research Note</a>',
        created_at: '2026-11-02 12:00:00',
      },
    ],
    onSendMessage: vi.fn(),
    showHeader: false,
    projectId: 'project-test',
    ...overrides,
  };
}

describe('#3151 PR1 Commit D — pane://note/ link click', () => {
  it('clicking the link calls onNoteLinkClick with the note id, and does NOT open externally', () => {
    const onNoteLinkClick = vi.fn();
    const { container } = render(ChatPanel, baseProps({ onNoteLinkClick }));

    const anchor = container.querySelector('a[href="pane://note/note-abc-123"]') as HTMLAnchorElement;
    expect(anchor, 'the pane://note/ anchor must actually be in the DOM').not.toBeNull();

    anchor.click();

    expect(onNoteLinkClick).toHaveBeenCalledTimes(1);
    expect(onNoteLinkClick).toHaveBeenCalledWith('note-abc-123');
    expect(openMock as unknown as ReturnType<typeof vi.fn>).not.toHaveBeenCalled();
  });

  it('with no onNoteLinkClick callback supplied, the click is a safe no-op — never falls through to open()', () => {
    const { container } = render(ChatPanel, baseProps());

    const anchor = container.querySelector('a[href="pane://note/note-abc-123"]') as HTMLAnchorElement;
    expect(anchor).not.toBeNull();

    anchor.click();

    expect(openMock as unknown as ReturnType<typeof vi.fn>).not.toHaveBeenCalled();
  });
});

/**
 * ChatPanel.navError.test.ts — #3036 PR2 item 4
 * (`docs/audits/2026-10-16-scope-agent-instructions.md` §6: "An unknown
 * view returns a visible error, not silence. Assert the error reaches
 * the UI, not just that a Result is Err.")
 *
 * `decideNavigationAction`'s own unit tests (`navigationDispatch.test.ts`)
 * prove the DECISION is correct (returns `{ kind: 'error', message }`
 * instead of doing nothing). This file proves the OTHER half: when
 * `ChatSidebar.svelte` sets its `navError` state from that decision, the
 * message actually renders as VISIBLE DOM text in the real, mounted
 * `ChatPanel.svelte` — not merely that some internal `Result` carries an
 * `Err` value nobody looks at. Mounts the real component (not a mock),
 * following the same `@testing-library/svelte` pattern already
 * established by `ChatPanel.probeDebugGate.test.ts`.
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

vi.mock('@tauri-apps/plugin-shell', () => ({
  open: vi.fn(),
}));

vi.mock('$lib/utils/markdown', () => ({
  renderMarkdown: (html: string) => html,
}));

function baseProps(overrides: Record<string, any> = {}) {
  return {
    messages: [],
    onSendMessage: vi.fn(),
    showHeader: false,
    projectId: 'project-test',
    ...overrides,
  };
}

describe('#3036 PR2 item 4 — navError banner is visibly rendered, not silent', () => {
  it('no banner at all when navError is null (the default/no-error state)', () => {
    const { container } = render(ChatPanel, baseProps());
    expect(container.querySelector('.connection-error')).toBeNull();
  });

  it('a navigation-dispatch error message is rendered as visible text in the DOM', () => {
    const message = 'Navigation failed: "bogus_view" is not a recognized view.';
    const { container } = render(ChatPanel, baseProps({ navError: message }));

    const banner = container.querySelector('.connection-error');
    expect(banner, 'the error banner element must exist in the DOM').not.toBeNull();
    expect(banner?.textContent).toContain(message);
  });

  it('the dismiss button calls onClearNavError (the banner is dismissible, not stuck)', async () => {
    const onClearNavError = vi.fn();
    const { container } = render(
      ChatPanel,
      baseProps({ navError: 'Could not navigate: project "proj-x" was not found.', onClearNavError }),
    );

    const banner = container.querySelector('.connection-error') as HTMLElement;
    expect(banner).not.toBeNull();
    const dismissBtn = banner.querySelector('button');
    expect(dismissBtn).not.toBeNull();
    (dismissBtn as HTMLButtonElement).click();

    expect(onClearNavError).toHaveBeenCalledTimes(1);
  });

  it('a navError banner does NOT show the Re-pair/Settings action connectionError shows (a navigation failure is not a connection problem)', () => {
    const { container } = render(ChatPanel, baseProps({ navError: 'Navigation failed: no target was provided.' }));
    const banner = container.querySelector('.connection-error') as HTMLElement;
    const buttons = Array.from(banner.querySelectorAll('button')).map((b) => b.textContent);
    expect(buttons).not.toContain('Settings');
    expect(buttons).not.toContain('Re-pair');
  });
});

/**
 * NotificationPanel.stripMarkdown.test.ts — #3178.
 *
 * `NotificationPanel.svelte` rendered `notif.body` raw
 * (`<p class="nt-preview">{notif.body}</p>`) — a notification built from
 * an agent reply containing an image embed showed the literal
 * `![alt](...)` markdown syntax in the bell row instead of readable
 * text. Fix wires in the SAME shared `$lib/utils/stripMarkdown` utility
 * `AllTasksView.svelte`'s own card preview already uses (zero
 * `stripMarkdown` references existed in this file before this commit).
 *
 * Mutation: remove the `stripMarkdown(...)` call around `notif.body` in
 * the template (revert to raw `{notif.body}`) — reddens: the literal
 * `![`/`](` substrings would survive into the rendered text.
 *
 * #3186 C63 — flipped back (the #3182 update above rationalized the
 * wrong target): the original case asserting `'lighthouse'` survives
 * was correct all along — the ask was alt text visible in the bell
 * preview, the same thing the chat bubble already shows for an image.
 * #3182's own fix (images running before links, so the whole
 * `![alt](path)` match is claimed before the links rule can touch it)
 * was right; what was wrong was that commit's `stripMarkdown.ts`
 * replacement value — `''`, discarding the capture — which dropped
 * alt text as a side effect of fixing the stray-`!` bug, not an
 * intended behavior. #3186 promotes the images rule's `.*?` to a
 * capture group and replaces with `'$1'`, so alt text now survives
 * exactly as it did before #3182 touched this file, with the stray-`!`
 * bug still fixed. Asserting on the RENDERED notification body below
 * (not the regex's return value directly) per this scope's own
 * instruction — the real own-session job-turn path can embed a
 * literal `\n\n` mid-string (stripMarkdown.test.ts has the matching
 * case), which only reads correctly once CSS `white-space: normal`
 * collapses it on render; a DOM-level `textContent` read sees the
 * rendered (collapsed) result, not the raw regex output.
 */
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, waitFor } from '@testing-library/svelte/svelte5';
import NotificationPanel from './NotificationPanel.svelte';

beforeAll(() => {
  globalThis.ResizeObserver = class ResizeObserver {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});

const invokeMock = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: any[]) => invokeMock(...args),
}));
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

beforeEach(() => {
  invokeMock.mockReset();
});

describe('NotificationPanel — bell preview strips markdown (#3178)', () => {
  it('a body containing an image embed renders its alt text, with no ![ / ]( / stray ! surviving (#3186 C63)', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'get_notifications') {
        return [
          {
            id: 'notif-3178',
            type: 'agent_message',
            title: 'Message from Nova',
            body: '![a lighthouse](Assets/44d12345-1234-5678-9abc-1234567890ab.png) · Aurum',
            source: 'system',
            read: 0,
            created_at: '2026-11-05 10:00:00',
            target_kind: 'session',
            target_id: 'sess-x',
          },
        ];
      }
      return null;
    });

    const { container } = render(NotificationPanel, { panelOpen: true, onRefreshBadge: vi.fn() });

    await waitFor(() => {
      const el = container.querySelector('.nt-preview');
      if (!el) throw new Error('preview not rendered yet');
      // #3186 C63: alt text is the ask — the images rule now keeps
      // it (`$1`), only the `![`/`](path)` markdown syntax is gone.
      expect(el.textContent).toContain('lighthouse');
    }, { container });

    const preview = container.querySelector('.nt-preview');
    // Asserting on the RENDERED notification body, not stripMarkdown's
    // return value directly — this case has no mid-string newline, so
    // the raw and rendered forms are identical here, but the next test
    // (the own-session job-turn shape) is where they diverge.
    expect(preview?.textContent).toBe('a lighthouse · Aurum');
    expect(preview?.textContent).not.toContain('![');
    expect(preview?.textContent).not.toContain('](');
    expect(preview?.textContent).not.toContain('!');
  });

  it('the own-session job-turn shape (embedded \\n\\n, alt text survives) renders clean — no stray space, no doubled separator (#3186 C63)', async () => {
    // Reed's gotcha, confirmed against the real source
    // (src-tauri/src/ai/scheduler.rs::own_session_notification_due
    // passes `result.final_text.trim()` — the FULL multi-line turn
    // text, not `.lines().next()` — into
    // `notifications.rs::emit_agent_message_notification`'s
    // `raw_body = format!("{} · {}", first_line, project_name)`). A
    // turn that narrates an image and appends the §39.4 receipt line
    // produces exactly this shape verbatim — this is the real body
    // behind Matt's "Generated 1 image(s) · Aurum" symptom, hand-
    // constructed the same way this file's own existing convention
    // already does (no job actually run).
    const body =
      '![A lighthouse overlooking the harbor at dusk](Assets/550e8400-e29b-41d4-a716-446655440000.png)\n\n' +
      'Generated 1 image(s) · Aurum';
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'get_notifications') {
        return [
          {
            id: 'notif-3186-own-session',
            type: 'agent_message',
            title: 'Message from Nova',
            body,
            source: 'system',
            read: 0,
            created_at: '2026-11-07 10:00:00',
            target_kind: 'session',
            target_id: 'sess-x',
          },
        ];
      }
      return null;
    });

    const { container } = render(NotificationPanel, { panelOpen: true, onRefreshBadge: vi.fn() });

    await waitFor(() => {
      const el = container.querySelector('.nt-preview');
      if (!el) throw new Error('preview not rendered yet');
      expect(el.textContent).toContain('lighthouse');
    }, { container });

    const preview = container.querySelector('.nt-preview');
    // stripMarkdown's own string output keeps the embedded `\n\n`
    // literally (nothing in the regex chain collapses an INTERNAL
    // double newline — only `.trim()` touches leading/trailing
    // whitespace). `.nt-preview` has no `white-space` override
    // (default `normal`), so a real browser collapses that
    // whitespace to a single space when it paints — jsdom's
    // `textContent` doesn't apply CSS, so this test normalizes
    // whitespace the same way rendering does, asserting on what the
    // bell actually reads rather than the raw unrendered string.
    const renderedBody = preview?.textContent?.replace(/\s+/g, ' ').trim();
    expect(renderedBody).toBe('A lighthouse overlooking the harbor at dusk Generated 1 image(s) · Aurum');
    expect(preview?.textContent).not.toContain('![');
    expect(preview?.textContent).not.toContain('](');
    expect(preview?.textContent).not.toContain('!');
  });

  it('an ordinary text-only body renders unchanged', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'get_notifications') {
        return [
          {
            id: 'notif-plain',
            type: 'agent_message',
            title: 'Message from Nova',
            body: 'All done, no images this time. · Aurum',
            source: 'system',
            read: 0,
            created_at: '2026-11-05 10:00:00',
            target_kind: 'session',
            target_id: 'sess-x',
          },
        ];
      }
      return null;
    });

    const { container, findByText } = render(NotificationPanel, { panelOpen: true, onRefreshBadge: vi.fn() });

    await waitFor(() => expect(invokeMock).toHaveBeenCalled(), { container });
    await findByText('All done, no images this time. · Aurum');
  });
});

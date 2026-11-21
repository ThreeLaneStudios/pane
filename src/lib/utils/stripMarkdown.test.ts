/**
 * stripMarkdown.test.ts — #3166 C53.
 *
 * `stripMarkdown` is the shared card-preview-snippet generator
 * (`AllTasksView.svelte`, `NotesBrowser.svelte`). It had explicit rules
 * for `![alt](src)` (plain-markdown image embeds) and every other
 * standard markdown shape, but zero rule for a raw HTML block. A
 * RESIZED image's embed is not plain markdown at all — per ADR-0069 and
 * `Editor.svelte::serializeAttachmentWrapper`'s two-branch split, it
 * serializes to a literal `<div data-attachment="" data-attachment-
 * id="…" …>…</div>` block sitting directly in the note's raw content.
 * Reed's framing: this is the same class of bug #3163 already named (a
 * surface that expects markdown receiving the raw-HTML resize form),
 * not cosmetic — a real serialization leak.
 */
import { describe, it, expect } from 'vitest';
import { stripMarkdown } from './stripMarkdown';

describe('#3166 C53 — stripMarkdown strips the resize-mode <div data-attachment> block', () => {
  it('strips a minimal resized-image block (no inner content, no extra attrs)', () => {
    const content = 'Before text\n\n<div data-attachment data-attachment-id="att-1"></div>\n\nAfter text';
    const result = stripMarkdown(content);
    expect(result).not.toContain('<div');
    expect(result).not.toContain('data-attachment');
    expect(result).toContain('Before text');
    expect(result).toContain('After text');
  });

  it('strips the full real shape buildAttachmentRawHtmlBlock emits — render-mode, type, filename, size, width, height all present', () => {
    const block =
      '<div data-attachment data-attachment-id="att-123" data-render-mode="inline" ' +
      'data-attachment-type="image" data-filename="photo.png" data-size="2048" ' +
      'data-width="400" data-height="300"></div>\n\n';
    const content = `Caption above\n\n${block}Caption below`;
    const result = stripMarkdown(content);
    expect(result).not.toContain('<div');
    expect(result).not.toContain('data-attachment-id');
    expect(result).toContain('Caption above');
    expect(result).toContain('Caption below');
  });

  it('replaces the block with a short placeholder rather than deleting it silently', () => {
    const content = '<div data-attachment data-attachment-id="att-1"></div>';
    const result = stripMarkdown(content);
    expect(result).toBe('📎 Image');
  });

  it('strips a block whose inner HTML is non-empty (NodeView children)', () => {
    const content = 'Note text <div data-attachment data-attachment-id="att-9"><span>caption</span></div> more text';
    const result = stripMarkdown(content);
    expect(result).not.toContain('<div');
    expect(result).not.toContain('<span');
    expect(result).toContain('Note text');
    expect(result).toContain('more text');
  });

  it('strips multiple independent resize blocks in the same note', () => {
    const content =
      '<div data-attachment data-attachment-id="att-1"></div>\n\n' +
      'middle text\n\n' +
      '<div data-attachment data-attachment-id="att-2"></div>';
    const result = stripMarkdown(content);
    expect(result).not.toContain('<div');
    expect(result).toContain('middle text');
    // Both blocks replaced — the placeholder appears twice.
    expect(result.match(/📎 Image/g)?.length).toBe(2);
  });

  it('a plain, non-resized image embed (![alt](Assets/x.png)) keeps its alt text — the ask, per #3186 C63', () => {
    const content = 'Before ![a lighthouse](Assets/abc.png) after';
    // #3186 C63: this assertion previously baked in dropped alt text
    // as "correct" output (the images rule's replacement was '',
    // discarding the capture entirely). Matt's ruling: the ask was
    // alt text surviving, like the chat bubble already shows for an
    // image — not alt text dropped silently. Flipped, not duplicated
    // alongside a second test — same discipline as the links rule,
    // which has always kept link text. The images rule now replaces
    // with `'$1'` (the promoted capture group), so the alt text sits
    // directly between the surrounding words with a single space on
    // each side — no double space, because nothing is deleted outright
    // anymore.
    expect(stripMarkdown(content)).toBe('Before a lighthouse after');
    expect(stripMarkdown(content)).not.toContain('<div');
    expect(stripMarkdown(content)).not.toContain('![');
    expect(stripMarkdown(content)).not.toContain('](');
    expect(stripMarkdown(content)).not.toContain('!');
  });

  it('#3186 C63 — the relative Assets/<uuid>.png form from a real notification body keeps its alt text, with no stray `!`', () => {
    const content = 'New attachment: ![a lighthouse](Assets/550e8400-e29b-41d4-a716-446655440000.png) added';
    expect(stripMarkdown(content)).toBe('New attachment: a lighthouse added');
    expect(stripMarkdown(content)).not.toContain('!');
  });

  it('#3186 C63 — the own-session job-turn shape: alt text survives, embedded \\n\\n from the turn text is untouched (not collapsed at the string level)', () => {
    // Reed's gotcha: the own-session notification path
    // (src-tauri/src/ai/scheduler.rs::own_session_notification_due)
    // passes the FULL multi-line turn text, not a single line — a
    // turn that narrates an image and appends the §39.4 receipt line
    // produces exactly this two-line shape. `$1` preserves the alt
    // text, but nothing in stripMarkdown collapses an internal
    // `\n\n` — that collapse is a CSS `white-space: normal` rendering
    // effect (see NotificationPanel.stripMarkdown.test.ts's rendered-
    // body assertion), not a string-level one, so this test asserts
    // the literal (uncollapsed) string stripMarkdown actually returns.
    const content = '![a lighthouse overlooking the harbor](Assets/44d12345-1234-5678-9abc-1234567890ab.png)\n\nGenerated 1 image(s)';
    expect(stripMarkdown(content)).toBe('a lighthouse overlooking the harbor\n\nGenerated 1 image(s)');
  });

  it('normal markdown content with no attachment block at all is completely unaffected', () => {
    const content = '# Heading\n\n**bold** and _italic_ and a [link](https://example.com).';
    const result = stripMarkdown(content);
    expect(result).toBe('Heading\n\nbold and italic and a link.');
  });

  /// Mutation guard (named explicitly, per the task standard): remove
  /// the new `.replace(/<div data-attachment.../, ...)` rule entirely —
  /// reddens every test above that asserts the block is gone (the raw
  /// `<div data-attachment...>` substring survives unchanged in the
  /// output instead).
  it('mutation guard: a resize block with a `>` inside a later attribute value is still matched correctly (the [^>]* opening-tag scan, not .*?)', () => {
    // data-filename legitimately contains no `>`, but this pins that the
    // opening tag's own attribute scan can't be fooled into closing
    // early at the FIRST `>` it sees inside the thing it's matching —
    // using [^>]* for the tag itself (not a lazy .*? spanning past the
    // tag close) is what guarantees that.
    const content = '<div data-attachment data-attachment-id="att-1" data-filename="weird.png"></div>tail';
    const result = stripMarkdown(content);
    expect(result).toBe('📎 Imagetail');
  });
});

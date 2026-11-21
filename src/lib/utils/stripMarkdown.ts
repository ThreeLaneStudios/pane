export function stripMarkdown(text: string): string {
  if (!text) return '';
  return text
    // #3166 C53 — a RESIZED image's embed is not plain markdown at all
    // (ADR-0069, Editor.svelte::serializeAttachmentWrapper's two-branch
    // split / attachmentSerializer.ts::buildAttachmentRawHtmlBlock): it
    // serializes to a literal `<div data-attachment="" data-attachment-
    // id="…" …>…</div>` HTML block sitting directly in the note's raw
    // content. None of the regex rules below have any concept of an HTML
    // tag, so that block passed through completely untouched and
    // rendered verbatim in any card preview — the same class of bug
    // #3163 already named (a surface that expects markdown receiving the
    // raw-HTML resize form), not cosmetic. Stripped BEFORE the `![alt]
    // (src)` plain-image rule below, since a resized attachment never
    // matches that rule's bracket/paren shape anyway and this one must
    // run first regardless. `[^>]*` (not `.*?`) for the opening tag's
    // attributes specifically so a `>` inside a later attribute value
    // can never prematurely close the match early.
    .replace(/<div data-attachment[^>]*>.*?<\/div>/gs, '📎 Image')
    .replace(/#{1,6}\s+/g, '')        // headers (including mid-string)
    .replace(/\*{1,3}([^*]+)\*{1,3}/g, '$1')  // bold/italic (more aggressive)
    .replace(/__(.+?)__/g, '$1')      // bold underscore
    .replace(/_(.+?)_/g, '$1')        // italic underscore
    .replace(/~~(.+?)~~/g, '$1')      // strikethrough
    .replace(/`{3}[\s\S]*?`{3}/g, '') // code blocks
    .replace(/`(.+?)`/g, '$1')        // inline code
    .replace(/^\s*[-*+]\s+/gm, '')    // unordered list items
    .replace(/^\s*\d+\.\s+/gm, '')    // ordered list items
    .replace(/^\s*>\s+/gm, '')        // blockquotes
    // #3182 C63 leftover: images must run BEFORE links. The links rule
    // `\[(.+?)\]\(.+?\)` has no `!`-awareness — it matches
    // `[alt](path)` on its own merits and eats it first, leaving the
    // leading `!` stranded with nothing left for the images rule to
    // match (no `[` immediately after it anymore). Swapping the order
    // lets the images rule claim `![alt](path)` whole, `!` included,
    // before the links rule ever sees it.
    //
    // #3186 C63: the ask was alt text surviving, like the chat bubble
    // already shows for an image — not alt text dropped silently. The
    // `.*?` here is promoted to a capture group and the replacement is
    // `'$1'` (not `''`) so the alt text itself is kept; only the
    // `![`/`](path)` markdown syntax around it is removed, same
    // discipline as the links rule immediately below (which has
    // always kept link text, never dropped it).
    .replace(/!\[(.*?)\]\(.+?\)/g, '$1')    // images
    .replace(/\[(.+?)\]\(.+?\)/g, '$1') // links
    .replace(/---+/g, '')              // horizontal rules
    .replace(/\n{3,}/g, '\n\n')       // collapse multiple newlines
    .trim();
}

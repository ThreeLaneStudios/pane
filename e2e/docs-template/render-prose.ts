// #470 docs-template PR: deterministic, dependency-free text rendering for
// the two kinds of manifest-sourced prose a generated page embeds --
// step/journey captions (plain text) and the prerequisite line (which
// arrives with a limited "**bold**" Markdown-ish convention, confirmed
// against every existing journey: `create-an-agent.journey.ts`'s
// PREREQUISITE constant is the only place this syntax is used anywhere in
// `e2e/journeys/`).
//
// Nothing here calls a model, reads a directory, generates an ID, stamps a
// timestamp, or uses a locale-aware string method -- the five variance
// sources named for this PR. Same input string -> same output string,
// every time, on every machine.

/**
 * Escapes the three characters that would otherwise be interpreted as HTML
 * markup if a caption ever contained them, AND the two characters ({ }) a
 * Svelte template would otherwise try to parse as an expression. Journey
 * captions are plain, hand-written prose today (confirmed: none of the six
 * captions in create-an-agent.journey.ts contain any of these five
 * characters), but a future journey's caption might, and the generated
 * `+page.svelte` must stay valid Svelte regardless -- `&#123;`/`&#125;`
 * render as literal `{`/`}` in a browser without re-opening a template
 * expression, which a raw `{`/`}` character would.
 */
export function escapeHtmlText(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/\{/g, '&#123;')
    .replace(/\}/g, '&#125;');
}

/** Same as `escapeHtmlText`, plus escaping `"` for use inside a double-quoted HTML/Svelte attribute. */
export function escapeHtmlAttr(s: string): string {
  return escapeHtmlText(s).replace(/"/g, '&quot;');
}

/**
 * Renders the limited "**bold**" convention used in journey prose (today:
 * only the prerequisite line, `**Before you start:** connect a provider in
 * Settings › AI.`) into HTML, escaping every text segment -- bold or not --
 * so no caption or prerequisite can inject arbitrary markup into the
 * generated page.
 *
 * Deliberately supports only this one convention, not general Markdown:
 * every caption and the prerequisite line in the one journey that exists
 * today were grepped for other Markdown syntax (links, lists, headings,
 * italics) and none appears -- adding support nobody has written would be
 * unexercised surface, not a feature. An odd number of "**" delimiters
 * (an unterminated bold run) falls back to a fully escaped, unformatted
 * render rather than guessing which half of the text was meant to be bold.
 */
export function renderInlineProse(text: string): string {
  const parts = text.split('**');
  if (parts.length % 2 === 0) {
    return escapeHtmlText(text);
  }
  return parts.map((part, i) => (i % 2 === 1 ? `<strong>${escapeHtmlText(part)}</strong>` : escapeHtmlText(part))).join('');
}

/**
 * Strips exactly one trailing "." from a step's `title` when building the
 * displayed `<h2>` heading -- render-time only, never touching the
 * journey source (`title` strings in `*.journey.ts` keep their trailing
 * period byte-identical; this function is never called on anything that
 * gets written back to a file, only on the string passed into the
 * heading line in `page.ts`).
 *
 * Rule, deliberately narrow: strip ONE trailing "." and nothing else.
 *   - `'Open Settings › Agents.'` -> `'Open Settings › Agents'` (the case
 *     this PR exists for).
 *   - A title ending in `?` or `!` is untouched -- this function only
 *     matches a literal trailing `.`, so `'Ready?'` and `'Go!'` pass
 *     through unchanged with no special-case code needed.
 *   - Does not generalize to "strip any trailing punctuation" (which
 *     would also eat a trailing `?`/`!`) and does not attempt to detect
 *     or protect an abbreviation (e.g. a hypothetical title ending
 *     "...etc.") -- none of today's six titles contain one (checked:
 *     `create-an-agent.journey.ts`'s six `title:` values end in
 *     "Agents.", "provider.", "model.", "agent.", "ready.", "chatting."
 *     -- no abbreviation among them), so there is nothing to protect
 *     against today. Flagging this as a known edge case for whoever
 *     writes a future journey whose title would end in an abbreviation:
 *     this strip will remove that period too (turning "etc." into "etc"
 *     in the heading only -- the caption and manifest `title` field keep
 *     the full stop), which is wrong for that one case. Caption text is
 *     never passed through this function at all (it goes through
 *     `renderInlineProse` directly in `page.ts`), so this can never
 *     mangle a caption.
 */
export function stripTrailingPeriod(title: string): string {
  return title.endsWith('.') ? title.slice(0, -1) : title;
}

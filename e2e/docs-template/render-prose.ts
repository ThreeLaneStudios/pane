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

// #470 docs-template PR: renders a journey manifest into the real
// `+page.svelte` shape paneapp-ai's docs routes already use. Traced
// against ALL 18 existing pages under
// ~/Projects/paneapp-ai/src/routes/docs/<slug>/+page.svelte (not a
// sample — every one of them was read or grepped for this PR's own
// audit; see this PR's description for the file:line citations backing
// every class emitted below), not invented here.
//
// #470 PR 1 (publish1) rewrite: the PREVIOUS version of this file (PR
// #483) invented three classes with zero CSS anywhere on the site --
// `.d-prereq`, `.d-fork`, `.d-fork-pending` -- and wrapped every step's
// caption in `.d-call` (a class that DOES exist globally, but is used
// that way by exactly ONE page on the whole site: the OLD
// `creating-an-agent` page this one replaces, not the other 17). Matt's
// ruling on #470: the generated page uses ONLY the site's existing
// classes, matching the COMMON shape across sibling pages rather than
// that one page's own quirk, and ships no new CSS at all. This rewrite:
//
//   - Step heading: `<h2>{index}. {title, trailing "." stripped}</h2>`,
//     no wrapper. Matches the numbered-<h2> convention used by 11 of the
//     18 docs pages (onboard-openrouter, getting-started, folders,
//     categories, backup-and-restore, memory-system, notes-and-tasks,
//     templates, smart-features, projects, onboard-ollama) -- confirmed
//     by reading/grepping every one of them.
//   - Step caption: plain `<p>{caption}</p>` right after the image, same
//     11-page majority (e.g. getting-started/+page.svelte:60-63) -- NOT
//     wrapped in `.d-call`, which no other page uses as a per-step
//     wrapper.
//   - Prerequisite line: plain `<p><strong>...</strong> ...</p>` -- the
//     exact shape install-pane/+page.svelte:112 already uses for its own
//     "Prerequisite:" line (no box, no special class -- none exists).
//     `renderInlineProse`'s existing `**bold**` convention already
//     produces this shape with zero new code.
//   - Fork line (unexercised today -- create-an-agent's six steps have
//     no fork): plain `<p>` with a normal in-prose `<a href>` (matches
//     how every other docs page links elsewhere, e.g.
//     getting-started/+page.svelte:97-104) or plain "(coming soon)" text
//     if the target isn't a known page yet. No `.d-fork` class -- it
//     doesn't exist anywhere on the site.
//   - ONE `<style>` block: `.d-shot` (used by every docs page for its
//     step screenshots) has no global CSS anywhere in
//     `src/lib/styles/docs.css` -- its only CSS source on the entire
//     site is an identical scoped `<style>` block duplicated at the
//     bottom of 17 of the 18 existing pages (the 18th,
//     system-files-creation, carries the same block plus extra unrelated
//     rules). Round 2 flagged this gap for Matt rather than inventing
//     around it; his ruling: the generated page carries that SAME block,
//     byte for byte -- not new CSS, a verbatim copy of what every
//     sibling page already ships. `D_SHOT_STYLE_BLOCK` below is copied
//     character-for-character from
//     install-pane/+page.svelte:130-139 (verified by diff against a
//     fresh clone of paneapp-ai@release -- see this PR's description for
//     the md5). No other class, no other rule -- this is the one and
//     only `<style>` block this generator emits.
//
// This PR generates for Matt's ruling: slug `docs/creating-an-agent`,
// images at `static/assets/tutorials/creating-an-agent` -- publishing
// (actually committing to paneapp-ai) is a later PR. This PR's own CI
// step builds the real paneapp-ai site with this file dropped in and
// screenshots the rendered result as proof, but never commits/pushes to
// that repo.
import { escapeHtmlAttr, escapeHtmlText, renderInlineProse, stripTrailingPeriod } from './render-prose.ts';
import type { JourneyManifest } from '../journeys/_runner.ts';

/**
 * Copied character-for-character from
 * install-pane/+page.svelte:130-139 (paneapp-ai@release, commit
 * a2ae2c5) -- the SAME scoped block 17 of the other 18 docs pages carry
 * (the 18th, system-files-creation, has this block plus extra unrelated
 * rules). Per Matt's ruling: this is the one `<style>` block the
 * generator ships, and it must stay byte-identical to its source -- do
 * not edit this constant without re-diffing against the live page.
 */
const D_SHOT_STYLE_BLOCK = [
  `<style>`,
  `  .d-shot {`,
  `    display: block;`,
  `    width: 100%;`,
  `    height: auto;`,
  `    margin: 0 0 18px;`,
  `    border-radius: var(--radius-card);`,
  `    border: 1px solid var(--bg-border);`,
  `  }`,
  `</style>`,
].join('\n');

/**
 * Renders one step's `fork` field (§25.13 rule 10's manifest mechanism)
 * as a plain paragraph after that step's caption -- no `.d-fork` class,
 * since none exists anywhere on the site (see this file's header
 * comment). `knownJourneys` is the set of journey names resolvable to a
 * real page *in this run* -- not a directory listing (this PR's own
 * determinism rule forbids `fs.readdir`), just the explicit set the
 * caller already knows about. When the fork's target isn't in that set,
 * this renders the label as plain text with "(coming soon)", never a
 * link to a route that would 404 -- Matt's ruling on #470.
 *
 * Not exercised by real fork data today: create-an-agent's six steps have
 * no fork (confirmed against every step in create-an-agent.journey.ts).
 */
function renderForkLine(fork: { journey: string; label: string }, knownJourneys: ReadonlySet<string>): string {
  if (knownJourneys.has(fork.journey)) {
    return `  <p>→ <a href="/docs/${escapeHtmlAttr(fork.journey)}">${escapeHtmlText(fork.label)}</a></p>`;
  }
  return `  <p>→ ${escapeHtmlText(fork.label)} (coming soon)</p>`;
}

/**
 * Renders `manifest` into a complete `+page.svelte` source string.
 * `slug` is the eventual publish slug (Matt's ruling: `creating-an-agent`
 * for this journey, which is NOT the journey's own `name`,
 * `create-an-agent` -- see the generator CLI's own header comment for why
 * those two differ). `knownJourneys` backs `fork` resolution above.
 *
 * Deterministic: a pure function of its three arguments. Same manifest,
 * same slug, same knownJourneys -> byte-identical output, every call.
 */
export function renderPageSvelte(manifest: JourneyManifest, slug: string, knownJourneys: ReadonlySet<string>): string {
  const base = `/assets/tutorials/${slug}`;
  const title = manifest.title;

  const lines: string[] = [];
  lines.push(`<script lang="ts">`);
  lines.push(`  import { page } from '$app/state';`);
  lines.push(`  import DocsPrevNext from '$lib/components/DocsPrevNext.svelte';`);
  lines.push('');
  lines.push(`  const base = '${base}';`);
  lines.push(`</script>`);
  lines.push('');
  lines.push(`<svelte:head>`);
  lines.push(`  <title>${escapeHtmlText(title)} — Pane Docs</title>`);
  lines.push(`  <meta`);
  lines.push(`    name="description"`);
  lines.push(`    content="${escapeHtmlAttr(title)} — step-by-step, with screenshots."`);
  lines.push(`  />`);
  lines.push(`</svelte:head>`);
  lines.push('');
  lines.push(`<article class="d-art">`);
  lines.push(`  <h1>${escapeHtmlText(title)}</h1>`);
  lines.push(`  <div class="meta">Tutorial · ${manifest.steps.length} steps</div>`);
  lines.push('');
  if (manifest.prerequisite !== undefined) {
    // Nearest existing construct: install-pane/+page.svelte:112's own
    // "Prerequisite:" line is a plain <p> with a bold lead-in -- no box,
    // no special class (none exists for this anywhere on the site).
    lines.push(`  <p>${renderInlineProse(manifest.prerequisite)}</p>`);
    lines.push('');
  }

  for (const step of manifest.steps) {
    const heading = `${step.index}. ${stripTrailingPeriod(step.title)}`;
    lines.push(`  <h2>${escapeHtmlText(heading)}</h2>`);
    lines.push(`  <img`);
    lines.push(`    class="d-shot"`);
    lines.push(`    src="{base}/${step.image}"`);
    lines.push(`    alt="${escapeHtmlAttr(step.title)}"`);
    lines.push(`    loading="lazy"`);
    lines.push(`  />`);
    lines.push(`  <p>${renderInlineProse(step.caption)}</p>`);
    if (step.fork !== undefined) {
      lines.push(renderForkLine(step.fork, knownJourneys));
    }
    lines.push('');
  }

  lines.push(`  <DocsPrevNext pathname={page.url.pathname} />`);
  lines.push(`</article>`);
  lines.push('');
  lines.push(D_SHOT_STYLE_BLOCK);
  lines.push('');

  return lines.join('\n');
}

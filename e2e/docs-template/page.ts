// #470 docs-template PR: renders a journey manifest into the real
// `+page.svelte` shape paneapp-ai's docs routes already use -- the format
// traced against a dozen existing pages under
// ~/Projects/paneapp-ai/src/routes/docs/<slug>/+page.svelte (quickstart,
// onboard-openrouter, system-files-creation, the existing
// creating-an-agent, etc.), not invented here. Every page shares:
//
//   <script lang="ts">
//     import { page } from '$app/state';
//     import DocsPrevNext from '$lib/components/DocsPrevNext.svelte';
//     const base = '/assets/tutorials/<slug>';
//   </script>
//   <svelte:head><title>...</title><meta name="description" ... /></svelte:head>
//   <article class="d-art">
//     <h1>...</h1>
//     <div class="meta">...</div>
//     ...per-step <h2>/<img class="d-shot">/<div class="d-call"><p>...</p></div>...
//     <DocsPrevNext pathname={page.url.pathname} />
//   </article>
//   <style> .d-shot { ... } </style>
//
// This PR generates for that shape and that slug/path (Matt's ruling:
// `docs/creating-an-agent`, images at `static/assets/tutorials/creating-an-agent`)
// but publishes nothing -- `paneapp-ai` is never touched, built, or
// written to. A later PR takes this file and actually opens a PR against
// that repo.
//
// Two rendering decisions this file makes that aren't dictated by the
// manifest shape alone, called out here (and in the PR description) per
// the task's "decide how to render and say what you chose":
//
//   1. The prerequisite line arrives as "**bold**" Markdown-ish text
//      (see render-prose.ts's own header comment) -- rendered via
//      `renderInlineProse`, which converts that one convention to
//      `<strong>` and HTML-escapes everything else.
//   2. Each step's `<h2>` is "`{index}. {step.title}`", matching the
//      numbered `<h2>` convention `onboard-openrouter/+page.svelte`
//      already uses. PR 483 round 2 added a hand-authored `title` to
//      every step (§25.13 rule 3, extended) -- before that, the manifest
//      carried only `name` (kebab-case) and `caption` (a full sentence,
//      too long for a heading), so this file mechanically Title Cased
//      `name` as a stand-in heading (`titleCaseFromKebab`, now deleted --
//      no caller left once a real hand-written heading existed). The
//      `<div class="meta">` line similarly can't borrow the site's
//      hand-picked "About N min" estimate (nothing in the manifest times
//      a read) -- it reports the mechanically true "Tutorial · N steps"
//      instead, derived from `manifest.steps.length`.
import { escapeHtmlAttr, escapeHtmlText, renderInlineProse } from './render-prose.ts';
import type { JourneyManifest } from '../journeys/_runner.ts';

/**
 * Renders one step's `fork` field (§25.13 rule 10's manifest mechanism,
 * approved as scoped on #470) as the final line inside that step's
 * `<div class="d-call">`. `knownJourneys` is the set of journey names
 * resolvable to a real page *in this run* -- not a directory listing (this
 * PR's own determinism rule forbids `fs.readdir`), just the explicit set
 * the caller already knows about. When the fork's target isn't in that
 * set, this renders the label as plain text with "(coming soon)", never a
 * link to a route that would 404 -- Matt's ruling on #470.
 *
 * Not exercised by real fork data today: create-an-agent's six steps have
 * no fork (confirmed against every step in create-an-agent.journey.ts).
 */
function renderForkLine(fork: { journey: string; label: string }, knownJourneys: ReadonlySet<string>): string {
  if (knownJourneys.has(fork.journey)) {
    return `    <p class="d-fork">→ <a href="/docs/${escapeHtmlAttr(fork.journey)}">${escapeHtmlText(fork.label)}</a></p>`;
  }
  return `    <p class="d-fork d-fork-pending">→ ${escapeHtmlText(fork.label)} (coming soon)</p>`;
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
    lines.push(`  <p class="d-prereq">${renderInlineProse(manifest.prerequisite)}</p>`);
    lines.push('');
  }

  for (const step of manifest.steps) {
    const heading = `${step.index}. ${step.title}`;
    lines.push(`  <h2>${escapeHtmlText(heading)}</h2>`);
    lines.push(`  <img`);
    lines.push(`    class="d-shot"`);
    lines.push(`    src="{base}/${step.image}"`);
    lines.push(`    alt="${escapeHtmlAttr(step.title)}"`);
    lines.push(`    loading="lazy"`);
    lines.push(`  />`);
    lines.push(`  <div class="d-call">`);
    lines.push(`    <p>${renderInlineProse(step.caption)}</p>`);
    if (step.fork !== undefined) {
      lines.push(renderForkLine(step.fork, knownJourneys));
    }
    lines.push(`  </div>`);
    lines.push('');
  }

  lines.push(`  <DocsPrevNext pathname={page.url.pathname} />`);
  lines.push(`</article>`);
  lines.push('');
  lines.push(`<style>`);
  lines.push(`  .d-shot {`);
  lines.push(`    display: block;`);
  lines.push(`    width: 100%;`);
  lines.push(`    height: auto;`);
  lines.push(`    margin: 0 0 18px;`);
  lines.push(`    border-radius: var(--radius-card);`);
  lines.push(`    border: 1px solid var(--bg-border);`);
  lines.push(`  }`);
  lines.push(`</style>`);
  lines.push('');

  return lines.join('\n');
}

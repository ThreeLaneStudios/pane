// #470 docs-template PR: deterministic kebab-case -> Title Case transform,
// used for each step's `<h2>` heading. There is no hand-authored per-step
// heading anywhere in the manifest today -- `ManifestStep` carries `name`
// (kebab-case, e.g. "open-agents-settings") and `caption` (a full sentence,
// too long for a heading), nothing in between. Rather than inventing
// heading prose (which this PR's determinism rule forbids -- "nothing at
// build time uses a model"), this mechanically title-cases the one
// machine identifier that already exists, matching the numbered-`<h2>`
// convention paneapp-ai's own `onboard-openrouter/+page.svelte` already
// uses ("1. Sign in", "2. Choose your AI provider", ...).
//
// Plain ASCII char-code operations only -- `.toUpperCase()` (not
// `.toLocaleUpperCase()`) is locale-independent per spec, but every
// `name` value is already guaranteed plain kebab-case ASCII by convention
// (the runner uses it verbatim in a screenshot filename), so there is no
// non-ASCII input to worry about either way.
export function titleCaseFromKebab(kebab: string): string {
  return kebab
    .split('-')
    .filter((word) => word.length > 0)
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
    .join(' ');
}

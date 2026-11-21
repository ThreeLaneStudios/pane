/**
 * searchMatch.ts — #3166 C51/C57.
 *
 * The one shared "does this query match this name" predicate —
 * `NewSessionSheet.svelte`'s own `matchesSearch` (§36.17.4, #3050 PR 10)
 * is the established pattern this mirrors: case-insensitive substring
 * match, empty/whitespace-only query matches everything. Extracted here
 * so the image-model picker (`Settings.svelte`) and the New job dialog's
 * Project/Agent fields (`ScheduledPanel.svelte`) wire the SAME helper
 * rather than each hand-rolling an independent copy of one substring
 * comparison — no new filter logic invented, per Matt's own scoping of
 * these as "wire the same matchesSearch helper onto these three fields."
 *
 * Matt's 2026-10-29 Ruling 3 (no agent-count threshold for
 * `NewSessionSheet`) extends here too: none of these three fields gate
 * the search input behind a minimum option count — it's always present.
 */

/** Case-insensitive substring match against a single name. Empty or
 *  whitespace-only query matches everything (the "no filter active"
 *  state). */
export function matchesSearchQuery(name: string, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return name.toLowerCase().includes(q);
}

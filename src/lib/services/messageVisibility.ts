/**
 * messageVisibility.ts — #3166 C55.
 *
 * Two independent loaders populate `ChatSidebar.svelte`'s single
 * `allMessages` cache and each hand-rolled its own copy of "should this
 * loaded row render in the thread":
 *
 *   - `navigation.ts`'s `navigateTo` STEP 4 (the sidebar project-click
 *     path) filtered system/compaction noise but had NO `synthetic`
 *     exclusion at all.
 *   - `ChatSidebar.svelte`'s `loadMessages` (its own doc comment: "the
 *     ONE session-switch path that bypasses `navigateTo` entirely" — the
 *     bell/card/dropdown session-switch path) filtered system noise AND
 *     a blanket `!m.synthetic`, intended for the §36.2.1 auto-continuation
 *     "Continue." row.
 *
 * `insert_job_brief_message` (scheduler.rs) ALSO sets `synthetic=1` on
 * the job/wake dispatch brief — `synthetic` is overloaded between "hide
 * this" (continuation marker) and "show this specially" (job brief,
 * rendered by `ChatPanel.svelte`'s own `message.job_id && message.synthetic
 * && message.role === 'user'` branch). `loadMessages`'s blanket exclusion
 * stripped the brief row out of `allMessages`; when a same-thread
 * `navigateTo` re-render later merged in "new" rows using ITS OWN filter
 * (no synthetic exclusion), the brief — now genuinely missing from
 * `allMessages` — was re-added for the first time, rendering raw markdown.
 *
 * One shared predicate now, so a third copy (or a fix to only one of the
 * two existing copies) can never diverge the same way these two did —
 * the same lesson #3155 C44 already taught about four independent copies
 * of one timestamp-parsing rule.
 *
 * `ChatPanel.svelte`'s own render-time filter (`visibleMessages`)
 * deliberately does NOT use `isVisibleLoadedMessage` below — it needs
 * synthetic job-brief rows to flow through so ITS OWN branch can render
 * them specially. It uses `isSystemNoiseMessage` alone.
 */

export interface MessageLikeForVisibility {
  content: string;
  role: string;
  synthetic?: number | boolean | null;
  job_id?: string | null;
}

/**
 * System/compaction/onboarding noise rows that must never render as a
 * chat bubble anywhere, regardless of thread or loader. Same six checks
 * every one of the (now five) call sites already applied independently.
 */
export function isSystemNoiseMessage(m: MessageLikeForVisibility): boolean {
  return (
    m.content.startsWith('[SYSTEM:') ||
    m.content.startsWith('## Session Summary') ||
    m.content.startsWith('URGENT:') ||
    m.content.includes('summarize the key decisions') ||
    m.content.includes('[TROVE APP CONTEXT]') ||
    m.content.includes('[END TROVE CONTEXT]') ||
    m.role === 'system'
  );
}

/**
 * The ONLY synthetic shape that should ever be hidden from the loaded
 * message list: a row with `synthetic` truthy and NO `job_id`. This is
 * §36.2.1/§36.2.3's auto-continuation "Continue." marker row. A job/wake
 * dispatch brief is ALSO `synthetic` but carries a `job_id` — it must
 * stay visible so `ChatPanel`'s own job-brief branch can render it.
 * This is the exact discriminator that branch already keys on
 * (`message.job_id && message.synthetic`); duplicated here as a
 * standalone predicate rather than referenced from ChatPanel, since
 * ChatPanel's version is a template condition, not an exported function.
 */
export function isHiddenSyntheticRow(m: MessageLikeForVisibility): boolean {
  return !!m.synthetic && !m.job_id;
}

/**
 * The full load-time visibility predicate `navigateTo` and
 * `ChatSidebar.loadMessages` (every reload site inside it, including the
 * pending-result reloads and `loadOlderMessages`) both need: not system
 * noise, not a hidden synthetic row.
 */
export function isVisibleLoadedMessage(m: MessageLikeForVisibility): boolean {
  return !isSystemNoiseMessage(m) && !isHiddenSyntheticRow(m);
}

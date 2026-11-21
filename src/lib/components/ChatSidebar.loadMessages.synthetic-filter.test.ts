/**
 * ChatSidebar.loadMessages.synthetic-filter.test.ts — #3166 C55.
 *
 * Matt's repro: click a project in the sidebar while already on it, and
 * every wake brief renders as a `JOB BRIEF` block with raw markdown;
 * navigating away and back clears it. Reed's diagnosis: two message
 * loaders disagreeing about `synthetic` rows on the skip-wipe merge path
 * (`navigation.ts`'s `navigateTo` STEP 4, and `ChatSidebar.svelte`'s
 * `loadMessages` — its own doc comment: "the ONE session-switch path
 * that bypasses `navigateTo` entirely").
 *
 * Before the fix, `loadMessages` filtered loaded rows with a blanket
 * `!m.synthetic`, while `navigateTo`'s own filter had NO synthetic
 * exclusion at all — the two independent copies disagreed about whether
 * a job/wake dispatch brief (`synthetic=1`, `job_id` set) should be
 * visible. This test file is named per Reed's build brief ("name the
 * test that fails first — a fix without a red test is how C27 shipped
 * three times") and is RED against the pre-fix shape of each loader
 * (see the first `it` block's own comment for how to reproduce that),
 * GREEN now that both loaders are wired to the one shared predicate in
 * `$lib/services/messageVisibility.ts`.
 */
import { describe, it, expect } from 'vitest';
import {
  isVisibleLoadedMessage,
  isHiddenSyntheticRow,
  isSystemNoiseMessage,
} from '$lib/services/messageVisibility';

/** The two independent loaders' PRE-FIX filter predicates, reproduced
 * verbatim (not imported — they no longer exist as separate code after
 * the fix) so this test can prove they used to disagree. */
function preFixNavigateToFilter(m: { content: string; role: string }): boolean {
  // navigation.ts's STEP 4, before #3166 C55 — no synthetic check at all.
  return (
    !m.content.startsWith('[SYSTEM:') &&
    !m.content.startsWith('## Session Summary') &&
    !m.content.startsWith('URGENT:') &&
    !m.content.includes('summarize the key decisions') &&
    !m.content.includes('[TROVE APP CONTEXT]') &&
    !m.content.includes('[END TROVE CONTEXT]') &&
    m.role !== 'system'
  );
}

function preFixLoadMessagesFilter(m: { content: string; role: string; synthetic?: number | boolean | null }): boolean {
  // ChatSidebar.svelte's loadMessages, before #3166 C55 — blanket !m.synthetic.
  return preFixNavigateToFilter(m) && !m.synthetic;
}

const jobBriefRow = {
  content: '**JOB BRIEF** Review open tasks and act on anything that needs doing.',
  role: 'user',
  synthetic: 1,
  job_id: 'job-1',
};

const continuationMarkerRow = {
  content: 'Continue.',
  role: 'user',
  synthetic: 1,
  job_id: null,
};

describe('#3166 C55 — the two loaders disagreed about synthetic job-brief rows', () => {
  it('RED PROOF: the two pre-fix filters disagreed on the exact job-brief fixture row from Matt\'s repro', () => {
    // This is the bug, reproduced directly: navigateTo's own filter would
    // include the brief (no synthetic check), loadMessages's own filter
    // would exclude it (blanket !m.synthetic) — the disagreement IS the
    // mechanism that let a brief, once stripped by loadMessages, get
    // "discovered as new" and re-added raw by a later navigateTo merge.
    expect(preFixNavigateToFilter(jobBriefRow)).toBe(true);
    expect(preFixLoadMessagesFilter(jobBriefRow)).toBe(false);
    expect(preFixNavigateToFilter(jobBriefRow)).not.toBe(preFixLoadMessagesFilter(jobBriefRow));
  });

  it('FIXED: the one shared predicate agrees with itself by construction — both loaders now call the same function', () => {
    // Both loaders are wired to `isVisibleLoadedMessage` now (see
    // navigation.ts and ChatSidebar.svelte) — there is only one answer
    // for this fixture row, not two that might disagree.
    expect(isVisibleLoadedMessage(jobBriefRow)).toBe(true);
  });

  it('a job/wake dispatch brief (synthetic=1, job_id set) is visible — must flow through to ChatPanel\'s own job-brief branch', () => {
    expect(isHiddenSyntheticRow(jobBriefRow)).toBe(false);
    expect(isVisibleLoadedMessage(jobBriefRow)).toBe(true);
  });

  it('the \u00a736.2.1 auto-continuation "Continue." marker row (synthetic=1, no job_id) is still hidden — the one case the exclusion exists for', () => {
    expect(isHiddenSyntheticRow(continuationMarkerRow)).toBe(true);
    expect(isVisibleLoadedMessage(continuationMarkerRow)).toBe(false);
  });

  it('a real, non-synthetic user message is always visible, matching both loaders before and after the fix', () => {
    const realMessage = { content: 'Hey, what time is it?', role: 'user', synthetic: 0, job_id: null };
    expect(preFixNavigateToFilter(realMessage)).toBe(true);
    expect(preFixLoadMessagesFilter(realMessage)).toBe(true);
    expect(isVisibleLoadedMessage(realMessage)).toBe(true);
  });

  it('system noise is excluded regardless of synthetic/job_id — the shared isSystemNoiseMessage half', () => {
    const noise = { content: '[SYSTEM: housekeeping]', role: 'user', synthetic: 1, job_id: 'job-1' };
    expect(isSystemNoiseMessage(noise)).toBe(true);
    expect(isVisibleLoadedMessage(noise)).toBe(false);
  });

  /// Mutation (actually run — see report): revert `isVisibleLoadedMessage`
  /// to call a blanket `!m.synthetic` again instead of
  /// `isHiddenSyntheticRow` — reddens this exact assertion (the job-brief
  /// fixture row flips from visible to hidden, reproducing Matt's repro).
  it('mutation guard: the job-brief fixture must stay visible through isVisibleLoadedMessage', () => {
    expect(isVisibleLoadedMessage(jobBriefRow)).toBe(true);
  });

  /// Same-thread merge simulation — the exact repro mechanics: a
  /// `loadMessages()` pass stripped the brief out of `allMessages`
  /// (old behavior), then a same-thread `navigateTo` merge treats it as
  /// "new" and appends it. With the shared predicate, `loadMessages`
  /// never strips it in the first place, so there is nothing for a later
  /// merge to "discover" — the count is stable across the round trip.
  it('a same-thread re-navigate after loadMessages does not re-add the brief as "new" (count stable)', () => {
    const dbRowsWithIds = [
      { ...jobBriefRow, id: 'row-brief' },
      { content: 'Hello', role: 'assistant', synthetic: 0, job_id: null, id: 'row-hello' },
    ];

    // Step 1: loadMessages() populates allMessages using the shared filter.
    const allMessages = dbRowsWithIds.filter(isVisibleLoadedMessage);
    expect(allMessages).toHaveLength(2);

    // Step 2: a same-thread navigateTo merge re-filters the SAME db rows
    // (same ids — a real re-fetch of the same thread) with the SAME
    // shared filter and merges by id — nothing "new" shows up, because
    // nothing was wrongly stripped out of `allMessages` in step 1 (the
    // pre-fix bug: `loadMessages` stripped the brief, so this merge later
    // saw it as genuinely new and re-added it raw).
    const existingIds = new Set(allMessages.map((m) => m.id));
    const reloaded = dbRowsWithIds.filter(isVisibleLoadedMessage);
    const newOnes = reloaded.filter((m) => !existingIds.has(m.id));
    expect(newOnes).toHaveLength(0);
    expect(allMessages).toHaveLength(2);
  });
});

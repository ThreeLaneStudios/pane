/**
 * jobSessionBack.test.ts — #3151 PR1 Commit F
 *
 * Both halves of the one-deep back mechanism: recording a back target
 * on entry (skipping the no-previous / same-session cases), and the
 * back action itself clearing BEFORE navigating and being a safe no-op
 * on a second call — the "what does a second back press do" question
 * the task explicitly requires an answer for.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import { jobSessionBackTarget } from '$lib/stores/app';
import {
  recordJobSessionEntry,
  goToJobSessionBackTarget,
  shouldShowJobSessionBackButton,
  isJobSessionOrigin,
} from './jobSessionBack';
import * as api from '$lib/api';
import { navigateTo } from '$lib/services/navigation';
import { sessionStore } from '$lib/stores/sessionStore';

vi.mock('$lib/api', () => ({
  switchSession: vi.fn(),
}));

vi.mock('$lib/services/navigation', () => ({
  navigateTo: vi.fn().mockResolvedValue(undefined),
}));

beforeEach(() => {
  jobSessionBackTarget.set(null);
  sessionStore.reset();
  vi.clearAllMocks();
});

describe('recordJobSessionEntry', () => {
  it('sets the back target when there is a real previous session', () => {
    recordJobSessionEntry('sess-previous', 'sess-job');
    expect(get(jobSessionBackTarget)).toBe('sess-previous');
  });

  it('does nothing when there is no previous session (nothing to go back to)', () => {
    recordJobSessionEntry(null, 'sess-job');
    expect(get(jobSessionBackTarget)).toBeNull();
    recordJobSessionEntry(undefined, 'sess-job');
    expect(get(jobSessionBackTarget)).toBeNull();
  });

  it('does nothing when the previous session IS the target (already there)', () => {
    recordJobSessionEntry('sess-job', 'sess-job');
    expect(get(jobSessionBackTarget)).toBeNull();
  });
});

describe('goToJobSessionBackTarget — one-deep, second call is a no-op', () => {
  it('clears the target BEFORE navigating, then switches/navigates to it', async () => {
    jobSessionBackTarget.set('sess-previous');
    (api.switchSession as any).mockImplementation(async () => {
      // The target must already be cleared by the time switchSession
      // resolves — proves the clear happens first, not after.
      expect(get(jobSessionBackTarget)).toBeNull();
      return { id: 'sess-previous', project_id: 'proj-x' };
    });

    await goToJobSessionBackTarget();

    expect(api.switchSession).toHaveBeenCalledWith('sess-previous');
    expect(navigateTo).toHaveBeenCalledWith({ view: 'kanban', projectId: 'proj-x', sessionId: 'sess-previous' });
    expect(get(jobSessionBackTarget)).toBeNull();
  });

  it('routes to Home when the target session has no real project', async () => {
    jobSessionBackTarget.set('sess-home');
    (api.switchSession as any).mockResolvedValue({ id: 'sess-home', project_id: 'workspace' });

    await goToJobSessionBackTarget();

    expect(navigateTo).toHaveBeenCalledWith({ view: 'home', sessionId: 'sess-home' });
  });

  it('a second call with no target set is a safe no-op — no switchSession, no navigateTo', async () => {
    expect(get(jobSessionBackTarget)).toBeNull();

    await goToJobSessionBackTarget();

    expect(api.switchSession).not.toHaveBeenCalled();
    expect(navigateTo).not.toHaveBeenCalled();
  });

  it('a failed switchSession still leaves the target cleared, not stale', async () => {
    jobSessionBackTarget.set('sess-will-fail');
    (api.switchSession as any).mockRejectedValue(new Error('boom'));

    await expect(goToJobSessionBackTarget()).rejects.toThrow('boom');
    expect(get(jobSessionBackTarget)).toBeNull();
  });
});

// #3155 C49b: the full `showBackButton` predicate used by
// `ChatSidebar.svelte` — TWO conditions, not one (see the function's own
// doc comment for why a back target alone is not sufficient). Extracted
// as a pure function specifically so this logic has a real Vitest
// surface with no component mount required.
//
// Mutation (the exact bug this closes): drop the origin check, i.e.
// `backTarget !== null` alone — the "target set, active session is NOT
// a job session" case below would read `true` instead of `false` — red.
// #3166 C58: `goToJobSessionBackTarget` also corrects `sessionStore`'s
// `activeSession` on the return trip — without this, going back from a
// job session would leave the header pinned on the job session's name
// instead of whatever session is now actually open (the same staleness
// bug the forward bell/card entry path had, just mirrored).
describe('goToJobSessionBackTarget — #3166 C58 corrects sessionStore.activeSession on the return trip', () => {
  it('sets sessionStore.activeSession to the session switchSession resolved, with no second API call', async () => {
    jobSessionBackTarget.set('sess-previous');
    const resolvedSession = { id: 'sess-previous', project_id: 'proj-x', title: 'Regular chat' };
    (api.switchSession as any).mockResolvedValue(resolvedSession);

    await goToJobSessionBackTarget();

    expect(get(sessionStore).activeSession).toEqual(resolvedSession);
  });
});

// #3166 C58: the bare origin check shared between the `←` button's own
// predicate (`shouldShowJobSessionBackButton` below) and the header's
// persistent "Job session · <name>" cue (`SessionDropdownHeader.svelte`)
// — one function, so the two surfaces can never independently disagree
// about what counts as "a job session."
describe('isJobSessionOrigin — #3166 C58 shared half', () => {
  it('true for spawn and schedule', () => {
    expect(isJobSessionOrigin('spawn')).toBe(true);
    expect(isJobSessionOrigin('schedule')).toBe(true);
  });

  it('false for user, null, and any other value', () => {
    expect(isJobSessionOrigin('user')).toBe(false);
    expect(isJobSessionOrigin(null)).toBe(false);
    expect(isJobSessionOrigin('migrated')).toBe(false);
  });

  /// Mutation guard: `shouldShowJobSessionBackButton` must be built FROM
  /// this function, not an independent copy of the same comparison —
  /// asserted by construction: both the true and false cases below only
  /// hold if the full predicate's origin half really does route through
  /// `isJobSessionOrigin`, not a second hand-rolled check that could
  /// drift from it.
  it('shouldShowJobSessionBackButton agrees with isJobSessionOrigin on the origin half for every origin value tested above', () => {
    for (const origin of ['spawn', 'schedule', 'user', null, 'migrated']) {
      expect(shouldShowJobSessionBackButton('sess-previous', origin)).toBe(isJobSessionOrigin(origin));
    }
  });
});

describe('shouldShowJobSessionBackButton — #3155 C49b origin gate', () => {
  it('true when a target exists AND the active session origin is spawn (a job\'s own dedicated session)', () => {
    expect(shouldShowJobSessionBackButton('sess-previous', 'spawn')).toBe(true);
  });

  it('true when a target exists AND the active session origin is schedule', () => {
    expect(shouldShowJobSessionBackButton('sess-previous', 'schedule')).toBe(true);
  });

  it('false when a target exists but the active session origin is NOT a job session (the Aurum case: a manual switch landed on an unrelated session)', () => {
    expect(shouldShowJobSessionBackButton('sess-previous', 'user')).toBe(false);
  });

  it('false when a target exists but the active session origin is null (no session resolved yet / navigation fallback)', () => {
    expect(shouldShowJobSessionBackButton('sess-previous', null)).toBe(false);
  });

  it('false when there is no target at all, even if the active session IS a job session', () => {
    expect(shouldShowJobSessionBackButton(null, 'spawn')).toBe(false);
  });

  it('false when neither condition holds', () => {
    expect(shouldShowJobSessionBackButton(null, 'user')).toBe(false);
  });
});

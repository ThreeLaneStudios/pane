/**
 * jobSessionBack.ts — #3151 PR1 Commit F
 *
 * The one-deep "back" mechanism for the two job-session entry points —
 * the Scheduled panel's "Open session" link (`scheduledPanel.ts`'s
 * `openScheduledJobSession`) and the notification bell
 * (`NotificationPanel.svelte`'s `target_kind === 'session'` case).
 * Pulled out as pure, dependency-free functions (own `get`/`navigateTo`/
 * `api` imports, no component state) so both call sites — AND the
 * `Cmd+[` keyboard shortcut (`+page.svelte`) AND the back button
 * (`ChatPanel.svelte`'s header, wired through `ChatSidebar.svelte`) —
 * share exactly one implementation of "what does going back actually
 * do," never three independently-written copies.
 *
 * One-deep, by construction: `jobSessionBackTarget` (`$lib/stores/app.ts`)
 * holds at most one id. `goToJobSessionBackTarget` clears it as its OWN
 * first step, before any navigation happens — so a second call (second
 * back-button press, or a second Cmd+[) reads `null` and is a plain,
 * safe no-op. There is no deeper stack to maintain; if the user then
 * navigates elsewhere and wants to get back to the job session, the
 * card/bell is the way in again, same as before this PR.
 */
import { get } from 'svelte/store';
import { jobSessionBackTarget } from '$lib/stores/app';
import { navigateTo } from '$lib/services/navigation';
import { sessionStore } from '$lib/stores/sessionStore';
import * as api from '$lib/api';

/**
 * Called by BOTH job-session entry points, right before they switch into
 * the target job session — `previousSessionId` is whatever the chat view
 * was showing a moment ago (`$activeChatThreadId`, read by the caller).
 * A no-op (leaves any existing back target untouched) when there is
 * nothing to go back to, or the "previous" session is the SAME one being
 * entered (e.g. clicking a job card for the session already on screen) —
 * neither case should produce a back button that returns to exactly
 * where the user already is.
 */
export function recordJobSessionEntry(previousSessionId: string | null | undefined, targetSessionId: string): void {
  if (previousSessionId && previousSessionId !== targetSessionId) {
    jobSessionBackTarget.set(previousSessionId);
  }
}

/**
 * #3155 C49b: the full `showBackButton` predicate, extracted as a pure,
 * dependency-free function (same rationale as this module's own header
 * comment — one implementation, not independently re-derived per call
 * site) so `ChatSidebar.svelte`'s `$derived` and this module's own test
 * file can both exercise the EXACT logic with no component mount
 * required.
 *
 * TWO conditions, not one. A non-null back target alone (the old, sole
 * condition) is "somewhere to go back TO" — it says nothing about where
 * the user IS right now. Reed's diagnosis: a back target set while
 * entering a job session, then surviving a manual sidebar/drawer switch
 * (`ChatSidebar.svelte`'s `handleSessionSwitch`, which now clears the
 * target — see its own doc comment — but didn't before this fix)
 * rendered the button on whatever unrelated session the user landed on
 * (Matt's repro: Aurum). `activeOrigin` is the SECOND, now-required
 * condition: the active session's own `origin` column must be
 * `'spawn'`/`'schedule'` — §36.11's existing "this is a job's own
 * dedicated session" convention, reused here, not reinvented. A wake
 * job's brief lives in its ORIGIN chat (whose `origin` is never
 * `'spawn'`/`'schedule'`), so this predicate also keeps the button off of
 * that case for free.
 */
export function shouldShowJobSessionBackButton(
  backTarget: string | null,
  activeSessionOrigin: string | null
): boolean {
  return backTarget !== null && isJobSessionOrigin(activeSessionOrigin);
}

/**
 * #3166 C58 — the bare origin half of `shouldShowJobSessionBackButton`,
 * extracted so the header's persistent "Job session · <name>" cue can
 * share the EXACT SAME check the `←` button's own predicate already
 * uses, rather than a second, independently-written `=== 'spawn' ||
 * === 'schedule'` comparison that could silently drift from this one.
 * The cue has no back-target condition (it must show even with nothing
 * to go back to — e.g. a deep link straight into a job session), so it
 * calls this half directly instead of the full predicate above.
 */
export function isJobSessionOrigin(activeSessionOrigin: string | null): boolean {
  return activeSessionOrigin === 'spawn' || activeSessionOrigin === 'schedule';
}

/**
 * The back action itself — one-deep, idempotent on a second call (see
 * this module's own doc comment). Mirrors the EXACT frontend path both
 * entry points already use to get INTO a job session
 * (`switchSession` + `navigateTo`, origin-guarded `demote_and_promote`
 * on the backend) — never a different mechanism for the return trip.
 */
export async function goToJobSessionBackTarget(): Promise<void> {
  const target = get(jobSessionBackTarget);
  if (!target) {
    return;
  }
  // Cleared FIRST: if `switchSession`/`navigateTo` below throws, the back
  // target is still gone — a failed return trip must not leave a stale
  // target a later, unrelated action could silently jump to.
  jobSessionBackTarget.set(null);

  const session = await api.switchSession(target);
  // #3166 C58: without this, going back would leave `sessionStore`'s
  // `activeSession` pinned on the job session being left, instead of
  // correcting to whatever is now actually open — the same staleness
  // bug the forward (bell/card) entry path had, in the other direction.
  sessionStore.setActiveSessionFromSwitch(session);
  const isHome = !session.project_id || session.project_id === 'workspace';
  if (isHome) {
    await navigateTo({ view: 'home', sessionId: target });
  } else {
    await navigateTo({ view: 'kanban', projectId: session.project_id, sessionId: target });
  }
}

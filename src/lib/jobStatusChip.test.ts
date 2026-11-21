/**
 * §36.4/§36.4.5 (#3050 PR 5) — Matt's "chips show live status" addition.
 *
 * Named test: "job exits → the chip row reflects done." Mutation: drop the
 * re-render (here: drop the status-dependent suffix entirely, i.e. make
 * `chipStatusSuffix` always return '') — confirm red.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { chipStatusSuffix, chipDisplayText, elapsedSeconds, isTerminalJobStatus, type JobStatusSnapshot } from './jobStatusChip';

describe('elapsedSeconds', () => {
  it('computes whole seconds between two stored-datetime strings', () => {
    expect(elapsedSeconds('2026-10-28 09:00:00', '2026-10-28 09:00:48')).toBe(48);
  });

  it('returns null when either timestamp is missing', () => {
    expect(elapsedSeconds(null, '2026-10-28 09:00:48')).toBeNull();
    expect(elapsedSeconds('2026-10-28 09:00:00', null)).toBeNull();
  });

  it('returns null for unparseable input', () => {
    expect(elapsedSeconds('not-a-date', '2026-10-28 09:00:48')).toBeNull();
  });

  it('returns null for a negative duration (clock skew / corrupted row)', () => {
    expect(elapsedSeconds('2026-10-28 09:00:48', '2026-10-28 09:00:00')).toBeNull();
  });
});

describe('chipStatusSuffix', () => {
  it('is empty for an unknown job (no snapshot yet)', () => {
    expect(chipStatusSuffix(undefined)).toBe('');
  });

  it('is empty for a pending job — the chip\'s own static text already says it', () => {
    expect(chipStatusSuffix({ status: 'pending', started_at: null, finished_at: null })).toBe('');
  });

  it('shows "running" for a running job', () => {
    expect(chipStatusSuffix({ status: 'running', started_at: '2026-10-28 09:00:00', finished_at: null })).toBe(' · running');
  });

  it('shows "done in Ns" for a done job with timing', () => {
    const snap: JobStatusSnapshot = { status: 'done', started_at: '2026-10-28 09:00:00', finished_at: '2026-10-28 09:00:48' };
    expect(chipStatusSuffix(snap)).toBe(' · done in 48s');
  });

  it('shows "done" (no duration) when timing is unavailable', () => {
    expect(chipStatusSuffix({ status: 'done', started_at: null, finished_at: null })).toBe(' · done');
  });

  it('shows "failed"/"cancelled"/"paused" for those statuses', () => {
    expect(chipStatusSuffix({ status: 'failed', started_at: null, finished_at: null })).toBe(' · failed');
    expect(chipStatusSuffix({ status: 'cancelled', started_at: null, finished_at: null })).toBe(' · cancelled');
    expect(chipStatusSuffix({ status: 'paused', started_at: null, finished_at: null })).toBe(' · paused');
  });

  // Ruling 4 (live-test #1b fix PR): `jobStatusChip.ts:56`'s merged
  // `'done'`/`'sent'` switch arm rendered a `sent` job as "done in 0s" —
  // `sent` only means Pane's own transport call succeeded, not that OC has
  // replied. `done` is reached only via §36.6 pickup actually matching a
  // reply. Mutation: revert to the merged `case 'done': case 'sent':` arm
  // → reddens (gets "done in 0s"-shaped text instead of "sent", since
  // started_at/finished_at are identical here).
  it('shows "sent" (no elapsed-time claim) for a sent job — never "done in Ns"', () => {
    const snap: JobStatusSnapshot = { status: 'sent', started_at: '2026-10-28 09:00:00', finished_at: '2026-10-28 09:00:00' };
    expect(chipStatusSuffix(snap)).toBe(' · sent');
    expect(chipStatusSuffix(snap)).not.toMatch(/done in/);
  });

  // Existing regression pin (Ruling 4): the split must not touch 'done'
  // itself — still "done in Ns" with real timing.
  it('still shows "done in Ns" for a done job with real timing, unchanged by the sent/done split', () => {
    const snap: JobStatusSnapshot = { status: 'done', started_at: '2026-10-28 09:00:00', finished_at: '2026-10-28 09:00:48' };
    expect(chipStatusSuffix(snap)).toBe(' · done in 48s');
  });

  // Ruling 3 (live-test #1b fix PR): a recurring/heartbeat job's status
  // stays 'pending' through a transport failure (self-heals at the next
  // scheduled firing) — last_error is the only observable trace.
  // Mutation: drop the `snapshot.last_error ?` check in the 'pending' arm
  // (always return '') → reddens (empty string instead of the failure
  // suffix).
  it('shows "last run failed" for a pending job with a non-null last_error', () => {
    const snap: JobStatusSnapshot = { status: 'pending', started_at: null, finished_at: null, last_error: 'sessions.send → HTTP 500' };
    expect(chipStatusSuffix(snap)).toBe(' · last run failed');
  });

  it('stays empty for a pending job with no last_error (never failed, or already cleared by a later success)', () => {
    expect(chipStatusSuffix({ status: 'pending', started_at: null, finished_at: null, last_error: null })).toBe('');
  });
});

describe('chipDisplayText — the named test: "job exits → the chip row reflects done"', () => {
  it('re-renders from "running" to "done in Ns" as the snapshot transitions', () => {
    const runningSnap: JobStatusSnapshot = { status: 'running', started_at: '2026-10-28 09:00:00', finished_at: null };
    const doneSnap: JobStatusSnapshot = { status: 'running', started_at: '2026-10-28 09:00:00', finished_at: null };
    // Mutate doneSnap into its post-exit shape (same object identity concept
    // ChatPanel.svelte's own reactive state update produces: the SAME
    // message's chip, re-rendered after a poll tick observes the job exited).
    const afterExit: JobStatusSnapshot = { status: 'done', started_at: '2026-10-28 09:00:00', finished_at: '2026-10-28 09:00:48' };

    const before = chipDisplayText('Spawned agent on \'Do the thing\'.', runningSnap);
    const after = chipDisplayText('Spawned agent on \'Do the thing\'.', afterExit);

    expect(before).toBe('Spawned agent on \'Do the thing\'. · running');
    expect(after).toBe('Spawned agent on \'Do the thing\'. · done in 48s');
    expect(after).not.toBe(before);
    void doneSnap;
  });

  it('never alters the static content for an unknown snapshot', () => {
    expect(chipDisplayText('Scheduled: Monday reminder · next Mon 9:00 AM', undefined)).toBe(
      'Scheduled: Monday reminder · next Mon 9:00 AM'
    );
  });
});

describe('chipDisplayText — C37 (#3139, live test #4): the "next" clause is computed at render time, in local time, reusing formatScheduleDateTime', () => {
  // Same `toStoredUtc` round-trip trick `scheduledPanel.test.ts` already
  // uses: `toISOString()` always produces the UTC-equivalent of a LOCAL
  // Date regardless of the host's actual timezone, so encoding a known
  // LOCAL wall-clock time this way and decoding it back through
  // `formatScheduleDateTime` is deterministic on any machine/CI timezone
  // — not just under `TZ=UTC` (though that's what CI runs).
  function toStoredUtc(d: Date): string {
    return d.toISOString().replace('T', ' ').replace(/\.\d+Z$/, '');
  }

  const fixedNow = new Date(2026, 9, 29, 15, 0, 0); // local: Thu Oct 29 2026, 15:00

  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(fixedNow);
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  /// Mutation: feed the raw UTC `next_run_at` string through directly
  /// instead of `formatScheduleDateTime(snapshot.next_run_at)` — reddens
  /// (wrong hour for any non-UTC `TZ` the test runner happens to use,
  /// since the raw stored string and the local-formatted string differ
  /// whenever the local UTC offset isn't exactly zero).
  it('appends a LOCAL-formatted " · next ..." clause when next_run_at is present', () => {
    const nextRunLocal = new Date(fixedNow);
    nextRunLocal.setHours(18, 0, 0, 0); // same day, local 18:00
    const snapshot: JobStatusSnapshot = {
      status: 'pending',
      started_at: null,
      finished_at: null,
      next_run_at: toStoredUtc(nextRunLocal),
    };

    const text = chipDisplayText('Scheduled: Monday reminder', snapshot);

    expect(text).toBe('Scheduled: Monday reminder · next Today 18:00');
    // Sanity: must NOT be the raw stored UTC string pasted in verbatim —
    // proves the clause went through the formatter, not a passthrough.
    expect(text).not.toContain(snapshot.next_run_at as string);
  });

  /// Regression guard: no snapshot, or a snapshot with no next_run_at at
  /// all (e.g. a paused job, or a snapshot shape that predates C37) —
  /// content is unchanged, no stray " · next" text invented from nothing.
  it('adds no "next" clause when next_run_at is absent or null', () => {
    expect(chipDisplayText('Scheduled: Monday reminder', undefined)).toBe('Scheduled: Monday reminder');
    expect(
      chipDisplayText('Scheduled: Monday reminder', { status: 'pending', started_at: null, finished_at: null, next_run_at: null })
    ).toBe('Scheduled: Monday reminder');
  });

  /// The status suffix still appends AFTER the next clause, for a
  /// terminal snapshot that also happens to carry a (now-stale) next_run_at
  /// — order matters for readability ("next ... · done in 48s", not the
  /// reverse).
  it('places the next clause before the status suffix', () => {
    const nextRunLocal = new Date(fixedNow);
    nextRunLocal.setHours(18, 0, 0, 0);
    const snapshot: JobStatusSnapshot = {
      status: 'done',
      started_at: '2026-10-29 14:59:00',
      finished_at: '2026-10-29 14:59:48',
      next_run_at: toStoredUtc(nextRunLocal),
    };
    expect(chipDisplayText('Scheduled: Monday reminder', snapshot)).toBe(
      'Scheduled: Monday reminder · next Today 18:00 · done in 48s'
    );
  });
});

describe('isTerminalJobStatus', () => {
  it('is true for done/sent/failed/cancelled', () => {
    expect(isTerminalJobStatus('done')).toBe(true);
    expect(isTerminalJobStatus('sent')).toBe(true);
    expect(isTerminalJobStatus('failed')).toBe(true);
    expect(isTerminalJobStatus('cancelled')).toBe(true);
  });

  it('is false for pending/running/paused/undefined — the polling loop must keep polling', () => {
    expect(isTerminalJobStatus('pending')).toBe(false);
    expect(isTerminalJobStatus('running')).toBe(false);
    expect(isTerminalJobStatus('paused')).toBe(false);
    expect(isTerminalJobStatus(undefined)).toBe(false);
  });
});

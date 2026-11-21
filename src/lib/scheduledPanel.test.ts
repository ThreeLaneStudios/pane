import { describe, it, expect } from 'vitest';
import {
  partitionScheduledJobs,
  isActiveStatus,
  isDoneStatus,
  rowVisualState,
  sortActiveRows,
  sortDoneRows,
  visibleDoneRows,
  formatScheduleDateTime,
  formatNextRun,
  formatLastRun,
  openScheduledJobSession,
  jobAwareEmptyStateHint,
  toDatetimeLocalValue,
  cronToFrequency,
  type ScheduledJobRow,
} from './scheduledPanel';
import { vi } from 'vitest';

function row(overrides: Partial<ScheduledJobRow>): ScheduledJobRow {
  return {
    id: 'job-1',
    title: 'Test job',
    kind: 'once',
    agent: 'Agent A',
    project: 'Home',
    project_id: 'workspace',
    next_run_at: null,
    status: 'pending',
    missed_count: 0,
    started_at: null,
    finished_at: null,
    last_error: null,
    session_id: 'session-1',
    brief: 'Test brief',
    cron: null,
    run_at: null,
    agent_id: 'agent-1',
    ...overrides,
  };
}

describe('#3050 PR 8 — partitionScheduledJobs (Active vs. Done by status)', () => {
  it('splits by the exact status sets §36.3.8 names', () => {
    const rows = [
      row({ id: 'a', status: 'pending' }),
      row({ id: 'b', status: 'running' }),
      row({ id: 'c', status: 'sent' }),
      row({ id: 'd', status: 'paused' }),
      row({ id: 'e', status: 'done' }),
      row({ id: 'f', status: 'failed' }),
      row({ id: 'g', status: 'stopped' }),
      row({ id: 'h', status: 'cancelled' }),
    ];
    const { active, done } = partitionScheduledJobs(rows);
    expect(active.map((r) => r.id)).toEqual(['a', 'b', 'c', 'd']);
    expect(done.map((r) => r.id)).toEqual(['e', 'f', 'g', 'h']);
  });

  it('isActiveStatus / isDoneStatus are mutually exclusive for every named status', () => {
    for (const s of ['pending', 'running', 'sent', 'paused', 'done', 'failed', 'stopped', 'cancelled']) {
      expect(isActiveStatus(s)).not.toBe(isDoneStatus(s));
    }
  });

  /** Mutation guard: an unknown status must land in NEITHER tab (never
   *  silently assumed Active) — catches a typo'd status string from
   *  regressing into the wrong tab or a crash. */
  it('an unrecognized status appears in neither tab', () => {
    const rows = [row({ id: 'z', status: 'bogus-status' })];
    const { active, done } = partitionScheduledJobs(rows);
    expect(active).toHaveLength(0);
    expect(done).toHaveLength(0);
  });
});

describe('#3151 PR2/C24 — visibleDoneRows (the Done tab\'s own "Show deleted" toggle)', () => {
  const doneRows = [
    row({ id: 'done-1', status: 'done' }),
    row({ id: 'failed-1', status: 'failed' }),
    row({ id: 'stopped-1', status: 'stopped' }),
    row({ id: 'cancelled-1', status: 'cancelled' }),
  ];

  /** Mutation guard: default (showDeleted=false) hides ONLY cancelled —
   *  done/failed/stopped must all survive. Mutation: filter out
   *  `stopped` too (conflating it with the toggle's scope) — reddens
   *  the stopped-row assertion. */
  it('hides only cancelled rows by default', () => {
    const visible = visibleDoneRows(doneRows, false);
    expect(visible.map((r) => r.id)).toEqual(['done-1', 'failed-1', 'stopped-1']);
  });

  /** Mutation guard: showDeleted=true reveals cancelled too — the exact
   *  inverse of the above. Mutation: make the toggle a no-op (always
   *  filter cancelled regardless of the flag) — reddens this test
   *  while leaving the default-off test passing, proving the flag
   *  itself is actually read. */
  it('reveals cancelled rows when showDeleted is true', () => {
    const visible = visibleDoneRows(doneRows, true);
    expect(visible.map((r) => r.id)).toEqual(['done-1', 'failed-1', 'stopped-1', 'cancelled-1']);
  });

  /** This toggle must never be confused with SessionsDrawer's unrelated
   *  `showHidden`/`hidden_at` toggle (Reed's scope §1.1's own verdict) —
   *  not a behavior test (there is no shared state to prove disjoint),
   *  so recorded as a design-property comment rather than an assertion
   *  that would prove nothing: `visibleDoneRows` takes a plain boolean
   *  with no import, reference, or shared store key from
   *  `SessionsDrawer.svelte`'s own toggle. */
});

describe('#3050 PR 8 — rowVisualState (the fifth row state: Active)', () => {
  it('a healthy pending row with no missed runs is "active" (the neutral pill + green dot)', () => {
    expect(rowVisualState(row({ status: 'pending', missed_count: 0 }))).toBe('active');
  });
  it('running -> "running" (teal left border)', () => {
    expect(rowVisualState(row({ status: 'running' }))).toBe('running');
  });
  it('a non-terminal recurring/heartbeat firing failure (status still pending, last_error set) -> "failed" (red left border), even with a positive missed_count — Ruling 3\'s self-heals-at-next-firing case, NOT the terminal Done-tab status', () => {
    expect(rowVisualState(row({ status: 'pending', last_error: 'boom', missed_count: 2 }))).toBe('failed');
  });
  it('a TERMINAL status=\'failed\' row (Done tab) is "done", not "failed" — the red stripe is only for a still-Active row', () => {
    expect(rowVisualState(row({ status: 'failed' }))).toBe('done');
  });
  /** Reed's audit (PR #3116, §3): the test above doesn't actually exercise
   *  the `isActiveStatus()` guard — the `row()` fixture defaults
   *  `last_error: null`, so a mutation that drops the guard entirely
   *  (`if (row.last_error) return 'failed';`) stays green regardless,
   *  because `last_error` is null either way. The backend's own write site
   *  (`scheduler.rs`'s 3rd-consecutive-failure path) sets `status='failed'`
   *  and `last_error` TOGETHER — a real terminal-failed row always has
   *  `last_error` set — so THIS is the fixture that actually distinguishes
   *  "terminal Done row with last_error" from "still-Active row with
   *  last_error": both have `last_error` non-null; only `status` differs,
   *  and the assertion must still land on 'done', not 'failed'. */
  it('a TERMINAL status=\'failed\' row that ALSO carries last_error (the real backend write shape) is still "done", not "failed" — pins the isActiveStatus() guard', () => {
    expect(rowVisualState(row({ status: 'failed', last_error: 'provider error — see the job\'s own transcript/report-back text for detail' }))).toBe('done');
  });
  it('paused -> "paused" (dimmed) even with a positive missed_count', () => {
    expect(rowVisualState(row({ status: 'paused', missed_count: 1 }))).toBe('paused');
  });
  it('pending with missed_count > 0 -> "missed" (amber left border)', () => {
    expect(rowVisualState(row({ status: 'pending', missed_count: 3 }))).toBe('missed');
  });
  it('any Done-tab status -> "done", regardless of missed_count/status nuance', () => {
    expect(rowVisualState(row({ status: 'done' }))).toBe('done');
    expect(rowVisualState(row({ status: 'cancelled', missed_count: 5 }))).toBe('done');
  });
});

describe('#3050 PR 8 — sortActiveRows (failed+missed first, then next run, paused last)', () => {
  it('orders failed/missed before healthy/running, and paused last', () => {
    const rows = [
      row({ id: 'paused', status: 'paused' }),
      row({ id: 'healthy', status: 'pending', next_run_at: '2026-11-02 09:00:00' }),
      row({ id: 'failed', status: 'pending', last_error: 'boom', next_run_at: '2026-11-02 09:00:00' }),
      row({ id: 'missed', status: 'pending', missed_count: 3, next_run_at: '2026-10-29 18:00:00' }),
      row({ id: 'running', status: 'running', next_run_at: '2026-10-29 20:00:00' }),
    ];
    const sorted = sortActiveRows(rows).map((r) => r.id);
    // failed+missed group first (any order between them is fine — not asserted),
    // then healthy/running by next_run_at, paused always last.
    expect(sorted.slice(0, 2).sort()).toEqual(['failed', 'missed'].sort());
    expect(sorted.slice(2, 4)).toEqual(['running', 'healthy']);
    expect(sorted[4]).toBe('paused');
  });

  /** Mutation guard: swap the rank comparison's tie-break to reverse
   *  next_run_at order — this exact scenario (two healthy rows) would
   *  then come out backwards. */
  it('within the healthy/running group, earlier next_run_at comes first', () => {
    const rows = [
      row({ id: 'later', status: 'pending', next_run_at: '2026-11-05 09:00:00' }),
      row({ id: 'sooner', status: 'pending', next_run_at: '2026-10-30 09:00:00' }),
    ];
    expect(sortActiveRows(rows).map((r) => r.id)).toEqual(['sooner', 'later']);
  });

  it('a row with no next_run_at (paused) never crashes the comparator and sorts after rows that have one', () => {
    const rows = [
      row({ id: 'has-next', status: 'pending', next_run_at: '2026-11-05 09:00:00' }),
      row({ id: 'no-next', status: 'paused', next_run_at: null }),
    ];
    expect(sortActiveRows(rows).map((r) => r.id)).toEqual(['has-next', 'no-next']);
  });
});

describe('#3050 PR 8 — sortDoneRows (newest first)', () => {
  it('orders by finished_at descending', () => {
    const rows = [
      row({ id: 'older', status: 'done', finished_at: '2026-10-20 10:00:00' }),
      row({ id: 'newer', status: 'done', finished_at: '2026-10-25 10:00:00' }),
    ];
    expect(sortDoneRows(rows).map((r) => r.id)).toEqual(['newer', 'older']);
  });

  it('a cancelled row with no finished_at still sorts deterministically (never throws)', () => {
    const rows = [
      row({ id: 'cancelled-no-finish', status: 'cancelled', finished_at: null }),
      row({ id: 'done-with-finish', status: 'done', finished_at: '2026-10-25 10:00:00' }),
    ];
    expect(() => sortDoneRows(rows)).not.toThrow();
    expect(sortDoneRows(rows)).toHaveLength(2);
  });
});

describe('#3050 PR 8 — 24-hour time formatting (Matt\'s override of the sheet\'s 12-hour rendering)', () => {
  // `formatScheduleDateTime` parses its `raw` argument as UTC (same
  // convention as `jobStatusChip.ts`'s `parseStoredDatetime` — SQLite's
  // `datetime('now')` strings are implicitly UTC) and renders LOCAL
  // wall-clock time — exactly like `chrono::Local::now().format("%H:%M")`
  // on the backend (§36.2's "24-hour, as the chips do" precedent, Reed's
  // scope audit §11). `toStoredUtc` below builds a raw string from a
  // LOCAL Date so these assertions are deterministic on any machine/CI
  // timezone: whatever the host's local offset is, encoding a local time
  // to its UTC-equivalent stored string and decoding it back must
  // reproduce that same local wall-clock time.
  function toStoredUtc(d: Date): string {
    return d.toISOString().replace('T', ' ').replace(/\.\d+Z$/, '');
  }

  const now = new Date(2026, 9, 29, 15, 0, 0); // local: Thu Oct 29 2026, 15:00

  function localAt(hour: number, minute: number, dayOffset = 0): Date {
    const d = new Date(now);
    d.setDate(d.getDate() + dayOffset);
    d.setHours(hour, minute, 0, 0);
    return d;
  }

  it('formats a same-day time as "Today HH:MM", 24-hour, never AM/PM', () => {
    const text = formatScheduleDateTime(toStoredUtc(localAt(18, 0)), now);
    expect(text).toBe('Today 18:00');
    expect(text).not.toMatch(/AM|PM/i);
  });

  it('formats tomorrow as "Tomorrow HH:MM"', () => {
    expect(formatScheduleDateTime(toStoredUtc(localAt(7, 0, 1)), now)).toBe('Tomorrow 07:00');
  });

  it('formats a further-out date as "{Weekday} {Month} {Day}, HH:MM", zero-padded', () => {
    // now is a local Thu; +4 days -> local Mon.
    expect(formatScheduleDateTime(toStoredUtc(localAt(9, 0, 4)), now)).toBe('Mon Nov 2, 09:00');
  });

  it('zero-pads single-digit hours and minutes', () => {
    expect(formatScheduleDateTime(toStoredUtc(localAt(9, 5)), now)).toBe('Today 09:05');
  });

  it('formatNextRun shows "Paused" for a paused row regardless of next_run_at', () => {
    expect(formatNextRun(row({ status: 'paused', next_run_at: toStoredUtc(localAt(9, 0, 4)) }), now)).toBe('Paused');
  });

  it('formatLastRun shows "Not run yet" when never started', () => {
    expect(formatLastRun(row({ started_at: null, finished_at: null }), now)).toBe('Not run yet');
  });

  it('formatLastRun shows "Running now" while running', () => {
    expect(formatLastRun(row({ status: 'running', started_at: toStoredUtc(localAt(14, 0)) }), now)).toBe('Running now');
  });

  it('formatLastRun appends "· failed" for a failed last run', () => {
    expect(
      formatLastRun(row({ status: 'failed', finished_at: toStoredUtc(localAt(9, 0)), last_error: 'boom' }), now)
    ).toBe('Today 09:00 · failed');
  });

  // ─── C19 (#3120 PR 3): jobAwareEmptyStateHint ─────────────────────────

  /// The issue's own example copy, reproduced exactly: title · agent · next
  /// run (relative-day word lowercased to read naturally mid-sentence).
  /// Mutation: revert the function to return the generic
  /// FRESH_SESSION_HINT string unconditionally — reddens (this exact
  /// job-aware text is absent).
  it('builds "title · agent · next run tomorrow HH:MM. Reports land here."', () => {
    const text = jobAwareEmptyStateHint(
      row({ title: 'Morning brief', agent: 'Brit', next_run_at: toStoredUtc(localAt(7, 30, 1)) }),
      now
    );
    expect(text).toBe('Morning brief · Brit · next run tomorrow 07:30. Reports land here.');
  });

  it('a same-day next run lowercases "today" the same way', () => {
    const text = jobAwareEmptyStateHint(row({ title: 'Check in', agent: 'Nova', next_run_at: toStoredUtc(localAt(18, 0)) }), now);
    expect(text).toBe('Check in · Nova · next run today 18:00. Reports land here.');
  });

  it('a further-out absolute date keeps its weekday capitalized mid-sentence', () => {
    const text = jobAwareEmptyStateHint(row({ title: 'Weekly review', agent: 'Nova', next_run_at: toStoredUtc(localAt(9, 0, 4)) }), now);
    expect(text).toBe('Weekly review · Nova · next run Mon Nov 2, 09:00. Reports land here.');
  });

  it('a paused job reads "next run paused", lowercased, not the capitalized column label', () => {
    const text = jobAwareEmptyStateHint(row({ title: 'Check in', agent: 'Nova', status: 'paused', next_run_at: toStoredUtc(localAt(9, 0, 4)) }), now);
    expect(text).toBe('Check in · Nova · next run paused. Reports land here.');
  });

  it('a once job with no next_run_at (already fired) reads "not scheduled yet" rather than a bare dash', () => {
    const text = jobAwareEmptyStateHint(row({ title: 'One-off', agent: 'Nova', next_run_at: null }), now);
    expect(text).toBe('One-off · Nova · next run not scheduled yet. Reports land here.');
  });

  it('formatLastRun appends "· N missed" when missed_count is positive and not failed', () => {
    // now is local Thu Oct 29; -6 days -> local Fri Oct 23.
    expect(
      formatLastRun(row({ status: 'pending', finished_at: toStoredUtc(localAt(9, 0, -6)), missed_count: 3 }), now)
    ).toBe('Fri Oct 23, 09:00 · 3 missed');
  });
});

describe('#3050 PR 8 — openScheduledJobSession (row click -> switch_session + navigateTo, task item 4)', () => {
  it('calls switchSession with the row\'s session_id, then navigateTo({ view: "home" }) for a workspace-scoped session', async () => {
    const switchSession = vi.fn().mockResolvedValue({ id: 'sess-1', project_id: 'workspace' });
    const navigateTo = vi.fn().mockResolvedValue(undefined);

    await openScheduledJobSession(row({ session_id: 'sess-1' }), { switchSession, navigateTo });

    expect(switchSession).toHaveBeenCalledWith('sess-1');
    expect(navigateTo).toHaveBeenCalledWith({ view: 'home', sessionId: 'sess-1' });
  });

  it('calls navigateTo({ view: "kanban", projectId }) for a project-scoped session', async () => {
    const switchSession = vi.fn().mockResolvedValue({ id: 'sess-2', project_id: 'proj-cabinet' });
    const navigateTo = vi.fn().mockResolvedValue(undefined);

    await openScheduledJobSession(row({ session_id: 'sess-2' }), { switchSession, navigateTo });

    expect(navigateTo).toHaveBeenCalledWith({ view: 'kanban', projectId: 'proj-cabinet', sessionId: 'sess-2' });
  });

  /** Mutation guard: a row click that navigates WITHOUT first calling
   *  switchSession would never go through PR 6's origin guard at all
   *  (the backend's own `demote_and_promote` origin-guard mutation is
   *  covered by PR 6's own test suite — this test instead pins the
   *  FRONTEND wiring: switchSession must be called, not bypassed). */
  it('never calls navigateTo before switchSession resolves (no bypass of the origin guard)', async () => {
    const callOrder: string[] = [];
    const switchSession = vi.fn().mockImplementation(async () => {
      callOrder.push('switchSession');
      return { id: 'sess-3', project_id: 'workspace' };
    });
    const navigateTo = vi.fn().mockImplementation(async () => {
      callOrder.push('navigateTo');
    });

    await openScheduledJobSession(row({ session_id: 'sess-3' }), { switchSession, navigateTo });

    expect(callOrder).toEqual(['switchSession', 'navigateTo']);
  });

  // #3151 PR1 Commit F — the one-deep back target is recorded on entry.
  it('records the back target with the PREVIOUS session id, read before switching', async () => {
    const switchSession = vi.fn().mockResolvedValue({ id: 'sess-job', project_id: 'proj-x' });
    const navigateTo = vi.fn().mockResolvedValue(undefined);
    const recordBackTarget = vi.fn();

    await openScheduledJobSession(row({ session_id: 'sess-job' }), {
      switchSession,
      navigateTo,
      getCurrentSessionId: () => 'sess-was-here',
      recordBackTarget,
    });

    expect(recordBackTarget).toHaveBeenCalledWith('sess-was-here', 'sess-job');
  });

  it('with no getCurrentSessionId/recordBackTarget deps supplied (e.g. this file\'s own earlier tests), the row click still succeeds — both are optional', async () => {
    const switchSession = vi.fn().mockResolvedValue({ id: 'sess-job', project_id: 'proj-x' });
    const navigateTo = vi.fn().mockResolvedValue(undefined);

    await expect(
      openScheduledJobSession(row({ session_id: 'sess-job' }), { switchSession, navigateTo })
    ).resolves.toBeUndefined();
  });

  // #3166 C58: the Jobs card bypasses SessionDropdownHeader's own
  // switchSession/loadSessions path too — same fix as the bell.
  it('calls setActiveSession with the Session switchSession resolved, reusing it rather than fetching again', async () => {
    const resolvedSession = { id: 'sess-job', project_id: 'proj-x', title: 'Morning brief' };
    const switchSession = vi.fn().mockResolvedValue(resolvedSession);
    const navigateTo = vi.fn().mockResolvedValue(undefined);
    const setActiveSession = vi.fn();

    await openScheduledJobSession(row({ session_id: 'sess-job' }), { switchSession, navigateTo, setActiveSession });

    expect(setActiveSession).toHaveBeenCalledWith(resolvedSession);
    expect(switchSession).toHaveBeenCalledTimes(1);
  });

  it('with no setActiveSession dep supplied, the row click still succeeds — optional, same as getCurrentSessionId/recordBackTarget', async () => {
    const switchSession = vi.fn().mockResolvedValue({ id: 'sess-job', project_id: 'proj-x' });
    const navigateTo = vi.fn().mockResolvedValue(undefined);

    await expect(
      openScheduledJobSession(row({ session_id: 'sess-job' }), { switchSession, navigateTo })
    ).resolves.toBeUndefined();
  });
});

describe('#3134 C26 — toDatetimeLocalValue (the once-job inverse formatter)', () => {
  /** Named test (Reed's scope §7's own required mutation pin): pinned to
   *  a non-UTC zone so a regression that swaps the local getters for
   *  `.toISOString()` (UTC) is actually visible — on a UTC-pinned CI
   *  host, that mutation would be silently invisible (local === UTC).
   *  Zone is passed explicitly via the `tz` argument (Matt's ruling,
   *  post-merge TZ-mocking fix) rather than mutating `process.env.TZ`
   *  mid-test, which vitest's default `threads` pool does not honor
   *  for already-initialized `Date` local getters. Mutation (run for
   *  this PR, see report): replace the body with
   *  `raw.replace(' ', 'T').slice(0, 16)` (an ISO-ish UTC slice,
   *  `.toISOString()`'s own shape) — this test's hour assertion reddens
   *  (`08:00`, the stored UTC hour, instead of `04:00`, the EDT-local
   *  hour). */
  it('renders the LOCAL hour, not the stored UTC hour, pinned to America/New_York', () => {
    // 2026-06-15 08:00:00 UTC == 2026-06-15 04:00 EDT (UTC-4, DST).
    const result = toDatetimeLocalValue('2026-06-15 08:00:00', 'America/New_York');
    expect(result).toBe('2026-06-15T04:00');
  });

  it('round-trips a January (EST, UTC-5, no DST) instant correctly too', () => {
    // 2026-01-15 08:00:00 UTC == 2026-01-15 03:00 EST (UTC-5).
    const result = toDatetimeLocalValue('2026-01-15 08:00:00', 'America/New_York');
    expect(result).toBe('2026-01-15T03:00');
  });

  it('returns "" for null/unparseable input — an empty, not-yet-filled control', () => {
    expect(toDatetimeLocalValue(null)).toBe('');
    expect(toDatetimeLocalValue('garbage')).toBe('');
  });
});

describe('#3134 C26 — cronToFrequency (the recurring-job inverse formatter, Ruling 3)', () => {
  /** Named test: all 4 of `buildCron()`'s own literal shapes classify
   *  correctly, inverse-round-tripping the exact strings that function
   *  produces. Mutation (run for this PR, see report): widen the DAILY
   *  dow check to also accept the weekdays dow value — the weekdays
   *  case then misclassifies as daily, and the "weekdays" assertion
   *  below reddens (observes daily's shape instead). */
  it('classifies all 4 known buckets', () => {
    expect(cronToFrequency('0 9 * * *')).toEqual({ frequency: 'daily', time: '09:00' });
    expect(cronToFrequency('30 9 * * 1-5')).toEqual({ frequency: 'weekdays', time: '09:30' });
    expect(cronToFrequency('15 14 * * 1')).toEqual({ frequency: 'weekly', time: '14:15' });
    expect(cronToFrequency('45 */4 * * *')).toEqual({ frequency: 'hourly', time: '00:45' });
  });

  // Named test (Ruling 3's own detection rule, the required "no
  // fidelity loss" trigger): a cron the picker cannot express returns
  // null. Mutation: loosen the hourly check to match any step value in
  // the hour field (not literally "every 4 hours") -- the every-2-hours
  // case below would misclassify as hourly instead of null, reddening
  // that assertion.
  it('returns null for an unmapped cron (the raw-field trigger)', () => {
    expect(cronToFrequency('7 3 * * 2,4')).toBeNull();
    expect(cronToFrequency('7 */2 * * *')).toBeNull();
    expect(cronToFrequency('0 9 1 * *')).toBeNull(); // day-of-month set — none of the 4 buckets ever set this
    expect(cronToFrequency('not a cron')).toBeNull();
  });

  it('rejects out-of-range minute/hour values rather than misclassifying them', () => {
    expect(cronToFrequency('99 9 * * *')).toBeNull();
    expect(cronToFrequency('0 99 * * *')).toBeNull();
  });
});

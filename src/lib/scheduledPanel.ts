/**
 * scheduledPanel.ts — Home → Scheduled panel (#3050 PR 8, §36.3.8).
 *
 * Pure, side-effect-free logic for the panel: the Active/Done tab
 * partition, the Active tab's sort ("failed and missed first, then by
 * next run, paused last" — design sheet 1a annotation #1), each row's
 * visual state (maps to `sched.css`'s `.sc.k-*`/`.st-*` classes), and
 * time formatting.
 *
 * Kept separate from `ScheduledPanel.svelte` so this has a real Vitest
 * surface with zero Tauri/`invoke` import-time dependency — same split
 * `jobStatusChip.ts` already established for the chat chip's own
 * formatting logic (4d-skip only covers the pixel walk, never the
 * data-level logic itself).
 *
 * §36.3.8 (Reed's PR 8 scope audit §1): the backend's `list_scheduled_jobs`
 * returns every non-spawn row unsorted beyond a stable `next_run_at` tie
 * break — tab membership (a pure `status` concern) and sort are THIS
 * module's job, not the backend's.
 */

export interface ScheduledJobRow {
  id: string;
  title: string;
  /** 'once' | 'recurring' | 'heartbeat' (never 'spawn' or 'wake' — the
   *  backend query already excludes both; Matt's ruling, #3050 PR 8
   *  follow-up — wake rows are chaining plumbing, not user-facing). */
  kind: string;
  agent: string;
  project: string;
  project_id: string;
  next_run_at: string | null;
  /** 'pending' | 'running' | 'sent' | 'paused' (Active) |
   *  'done' | 'failed' | 'stopped' | 'cancelled' (Done) */
  status: string;
  missed_count: number;
  started_at: string | null;
  finished_at: string | null;
  last_error: string | null;
  session_id: string;
  /** #3134 C26: added to `list_scheduled_jobs`'s SELECT for the Edit
   *  dialog's pre-fill — the backend's own row already carried every one
   *  of these columns (no schema change), this panel's query just didn't
   *  read them until Edit needed to. */
  brief: string;
  cron: string | null;
  run_at: string | null;
  agent_id: string;
}

/** §36.3.8: "Two tabs, driven by `status`." */
export const ACTIVE_STATUSES = ['pending', 'running', 'sent', 'paused'] as const;
export const DONE_STATUSES = ['done', 'failed', 'stopped', 'cancelled'] as const;

export function isActiveStatus(status: string): boolean {
  return (ACTIVE_STATUSES as readonly string[]).includes(status);
}

export function isDoneStatus(status: string): boolean {
  return (DONE_STATUSES as readonly string[]).includes(status);
}

/** §3151 PR2/C24 — the Done tab's own "Show deleted" toggle (NOT the
 *  SessionsDrawer's unrelated `showHidden`/`hidden_at` toggle — different
 *  mechanism, different column, see that PR's own scope note). By
 *  default, `cancelled` ("Deleted") rows are hidden from the Done tab so
 *  it shows "completed and failed only," matching what the tab name
 *  implies. `showDeleted = true` reveals them. `stopped` rows are
 *  unaffected either way — they were never part of this toggle's scope
 *  (#3136/#3151 both name only `cancelled` as the hidden-by-default
 *  status). Archived rows never reach this function at all — the
 *  backend's `list_scheduled_jobs` excludes `archived_at IS NOT NULL`
 *  unconditionally, before the toggle is ever consulted. */
export function visibleDoneRows(doneRows: ScheduledJobRow[], showDeleted: boolean): ScheduledJobRow[] {
  if (showDeleted) return doneRows;
  return doneRows.filter((r) => r.status !== 'cancelled');
}

export function partitionScheduledJobs(
  rows: ScheduledJobRow[]
): { active: ScheduledJobRow[]; done: ScheduledJobRow[] } {
  const active = rows.filter((r) => isActiveStatus(r.status));
  const done = rows.filter((r) => isDoneStatus(r.status));
  return { active, done };
}

/**
 * Row visual state — maps 1:1 to the design sheet's left-border/status-pill
 * classes (`sched.css`: `.sc.k-run` teal, `.sc.k-miss` amber, `.sc.k-fail`
 * red, `.sc.k-pause` dimmed, no stripe for a healthy "Active" row — the
 * fifth row state the Notes section calls out explicitly: "a healthy,
 * waiting schedule... the neutral pill with a green dot").
 *
 * The design sheet's Active tab (1a) shows a row with a RED "Failed"
 * badge ("Weekly review") that still has a future `next_run_at` — this is
 * NOT `agent_jobs.status = 'failed'` (that status is terminal, Done-tab
 * only, per §36.3.8's own partition). It's §36.3.4 Ruling 3's per-firing
 * failure signal on a recurring/heartbeat job that is STILL `'pending'`
 * (self-heals at the next scheduled firing — only the 3rd consecutive
 * failure flips `status` to the terminal `'failed'`): `last_error`
 * non-NULL while `status` is still an Active one. "Missed" is the
 * analogous non-terminal signal (`missed_count > 0`). `paused` overrides
 * both (Notes: "Paused rows dim instead" of showing a color stripe).
 */
export type RowVisualState = 'running' | 'missed' | 'failed' | 'paused' | 'active' | 'done';

export function rowVisualState(row: ScheduledJobRow): RowVisualState {
  if (row.status === 'running') return 'running';
  if (row.status === 'paused') return 'paused';
  if (isActiveStatus(row.status) && row.last_error) return 'failed';
  if (isActiveStatus(row.status) && row.missed_count > 0) return 'missed';
  if (isDoneStatus(row.status)) return 'done';
  return 'active';
}

/** §36.3.8/Notes: "Row sort is one flat list: failed and missed first,
 *  then by next run, paused last." */
function activeSortRank(row: ScheduledJobRow): number {
  const state = rowVisualState(row);
  if (state === 'failed' || state === 'missed') return 0;
  if (state === 'paused') return 2;
  return 1;
}

export function sortActiveRows(rows: ScheduledJobRow[]): ScheduledJobRow[] {
  return [...rows].sort((a, b) => {
    const ra = activeSortRank(a);
    const rb = activeSortRank(b);
    if (ra !== rb) return ra - rb;
    // Within the same rank, order by next_run_at ascending. A paused row
    // has no meaningful next_run_at (claim_due_jobs never selects it) —
    // sorts after any row that has one, stable among themselves.
    if (!a.next_run_at && !b.next_run_at) return 0;
    if (!a.next_run_at) return 1;
    if (!b.next_run_at) return -1;
    return a.next_run_at.localeCompare(b.next_run_at);
  });
}

/** §36.3.8: Done tab is "newest first" — by `finished_at` descending,
 *  falling back to `id` for a stable order among ties/missing values
 *  (e.g. a `cancelled` row's `finished_at` may be unset — Reed's scope
 *  audit §5 confirms `cancel_job` never writes `finished_at`). */
export function sortDoneRows(rows: ScheduledJobRow[]): ScheduledJobRow[] {
  return [...rows].sort((a, b) => {
    const at = a.finished_at ?? '';
    const bt = b.finished_at ?? '';
    if (at !== bt) return bt.localeCompare(at);
    return b.id.localeCompare(a.id);
  });
}

/** SQLite's `datetime('now')`-shaped string ("YYYY-MM-DD HH:MM:SS", no
 *  offset, implicitly UTC) — same parse shape `jobStatusChip.ts`'s
 *  `parseStoredDatetime` already uses. */
function parseStored(raw: string | null): Date | null {
  if (!raw) return null;
  const parsed = new Date(raw.replace(' ', 'T') + 'Z');
  return Number.isNaN(parsed.getTime()) ? null : parsed;
}

/** Zero-padded 24-hour `HH:MM` — Matt's explicit override of the design
 *  sheet's 12-hour rendering (`9:00 AM`) for this build. No `hour12`. */
function formatClock24(d: Date): string {
  const hh = String(d.getHours()).padStart(2, '0');
  const mm = String(d.getMinutes()).padStart(2, '0');
  return `${hh}:${mm}`;
}

function isSameCalendarDay(a: Date, b: Date): boolean {
  return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();
}

const WEEKDAY_NAMES = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
const MONTH_NAMES = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

/** "Today 18:00" / "Tomorrow 07:00" / "Mon Nov 2, 09:00" — same relative-
 *  day vocabulary the design sheet uses ("Today 6:00 PM", "Mon Nov 2,
 *  9:00 AM"), rendered in 24-hour time per Matt's override. */
export function formatScheduleDateTime(raw: string | null, now: Date = new Date()): string {
  const d = parseStored(raw);
  if (!d) return '';
  if (isSameCalendarDay(d, now)) return `Today ${formatClock24(d)}`;
  const tomorrow = new Date(now);
  tomorrow.setDate(tomorrow.getDate() + 1);
  if (isSameCalendarDay(d, tomorrow)) return `Tomorrow ${formatClock24(d)}`;
  return `${WEEKDAY_NAMES[d.getDay()]} ${MONTH_NAMES[d.getMonth()]} ${d.getDate()}, ${formatClock24(d)}`;
}

/** The "Next" column. A paused row has no next run (per the sheet's own
 *  muted "Paused" text in that cell). */
export function formatNextRun(row: ScheduledJobRow, now: Date = new Date()): string {
  if (row.status === 'paused') return 'Paused';
  if (!row.next_run_at) return '—';
  return formatScheduleDateTime(row.next_run_at, now);
}

/** C19 (#3120 PR 3): job-aware copy for a job session's own empty state,
 *  replacing the generic "Fresh session…" hint (`sessionStore.ts`'s
 *  `FRESH_SESSION_HINT`/`FRESH_SESSION_HINT_HOME`) when the zero-message
 *  session IS a job's own session — "Morning brief · Brit · next run
 *  tomorrow 07:30. Reports land here." (the issue's own example copy).
 *  Reuses `formatNextRun` verbatim (the "next run"/"Paused"/"—" column
 *  logic the panel already shows), lowercasing only the leading
 *  relative-day word ("Today"/"Tomorrow"/"Paused") to read naturally
 *  inside a sentence — an absolute weekday date ("Mon Nov 2, 09:00")
 *  stays capitalized, same as any other mid-sentence proper noun. */
export function jobAwareEmptyStateHint(row: ScheduledJobRow, now: Date = new Date()): string {
  const nextRun = formatNextRun(row, now);
  const nextRunText = nextRun === '—' ? 'not scheduled yet' : nextRun.replace(/^(Today|Tomorrow|Paused)/, (m) => m.toLowerCase());
  return `${row.title} · ${row.agent} · next run ${nextRunText}. Reports land here.`;
}

/** The "Last" column — never-run / running-now / failed / missed / plain
 *  completion, matching the sheet's own per-state text (1a's "Not run
 *  yet", "Running now · 0:42", "Today 9:00 AM · failed",
 *  "Fri Oct 23 · 3 missed"). Elapsed-seconds-while-running is left to
 *  the caller (needs a live clock tick this pure function doesn't have);
 *  this returns the static "Running now" text for that state. */
export interface SessionOpenDeps {
  /** `api.switchSession` — the Tauri `switch_session` command, PR 6's
   *  origin-guarded `demote_and_promote`. Must be called for every row
   *  click: it is the ONLY mechanism that resolves a job-origin session
   *  WITHOUT demoting the project's real active interactive session. */
  switchSession: (sessionId: string) => Promise<{ id: string; project_id: string }>;
  /** `navigateTo`, the same imperative navigation helper
   *  `NotificationPanel.svelte`'s `target_kind === 'session'` case uses. */
  navigateTo: (dest: {
    view: 'home' | 'kanban';
    projectId?: string;
    sessionId?: string;
  }) => Promise<unknown>;
  /**
   * #3166 C58: `sessionStore.setActiveSessionFromSwitch` — the Jobs
   * card is a job-session entry point that bypasses
   * `SessionDropdownHeader`'s own `switchSession`/`loadSessions`
   * (its `$effect` only re-fires on a `projectId` change), so without
   * this call the header kept showing the project's ordinary session's
   * stale name after opening a job session in the SAME project. Takes
   * whatever `switchSession` above already returned — no second round
   * trip. Optional so this file's own earlier tests (which don't
   * exercise this side effect) keep working unchanged.
   */
  setActiveSession?: (session: { id: string; project_id: string }) => void;
  /**
   * #3151 PR1 Commit F: the chat view's own current session, read ONCE
   * before switching — handed to `recordBackTarget` below so the back
   * button/`Cmd+[` can return to wherever the user was. Optional —
   * omitted entirely by `scheduledPanel.test.ts`'s existing pure-logic
   * tests, which don't exercise the back-target side effect.
   */
  getCurrentSessionId?: () => string | null;
  /**
   * #3151 PR1 Commit F: injected rather than imported directly — this
   * module's own header states "zero Tauri/`invoke` import-time
   * dependency," and `$lib/services/jobSessionBack.ts`'s real
   * `recordJobSessionEntry` imports `$lib/api`. The real caller
   * (`ScheduledPanel.svelte`) wires the real function in; tests inject
   * a spy, same as `switchSession`/`navigateTo` above.
   */
  recordBackTarget?: (previousSessionId: string | null, targetSessionId: string) => void;
}

/**
 * Row click → open that schedule's session, via the EXACT SAME frontend
 * path a notification's click-to-open already uses
 * (`NotificationPanel.svelte`'s `target_kind === 'session'` case):
 * `switchSession` (PR 6's origin-guarded `demote_and_promote`) then
 * `navigateTo`. Never a new command, never a new guard — task item 4.
 *
 * Extracted as a pure, dependency-injected function (not inline in
 * `ScheduledPanel.svelte`) so it has a real Vitest surface without
 * mounting the component or a live Tauri runtime.
 */
export async function openScheduledJobSession(
  row: ScheduledJobRow,
  deps: SessionOpenDeps
): Promise<void> {
  // #3151 PR1 Commit F: read BEFORE switching — this is "where the user
  // was a moment ago," the one-deep back target.
  const previousSessionId = deps.getCurrentSessionId?.() ?? null;
  const session = await deps.switchSession(row.session_id);
  deps.setActiveSession?.(session);
  const isHome = !session.project_id || session.project_id === 'workspace';
  if (isHome) {
    await deps.navigateTo({ view: 'home', sessionId: row.session_id });
  } else {
    await deps.navigateTo({ view: 'kanban', projectId: session.project_id, sessionId: row.session_id });
  }
  deps.recordBackTarget?.(previousSessionId, row.session_id);
}

/** #3134 C26 — the Edit dialog's two inverse formatters. Both pure, zero
 *  DOM, same file/pattern as `parseStored`/`formatClock24`/`buildCron`'s
 *  own forward-direction siblings (the latter lives in
 *  `ScheduledPanel.svelte`, not here — see that file's own `buildCron`
 *  doc comment, which `cronToFrequency` below is the literal inverse of).
 */

/** The simplified frequency picker's recurring subset — `'once'` is
 *  deliberately excluded (that path uses `toDatetimeLocalValue` below,
 *  never this type). */
export type Frequency = 'daily' | 'weekdays' | 'hourly' | 'weekly';

/** Once-job pre-fill: the stored UTC datetime string → the exact
 *  `<input type="datetime-local">` value shape (`YYYY-MM-DDTHH:MM`,
 *  local, no seconds). **Never `.toISOString()`** — that's UTC and
 *  would silently shift the pre-filled time by the local offset
 *  (Ruling 1's "inverse formatters" item). `''` for `null`/unparseable
 *  input (an empty, not-yet-filled control, same as a fresh "New job"
 *  open with no `formOnceDateTime` set).
 *
 *  `tz` (Matt's ruling, #3134 C26 post-merge TZ-mocking fix): an
 *  optional explicit IANA zone name. Omitted (the only way any real
 *  caller ever invokes this — `ScheduledPanel.svelte`'s
 *  `openEditJobModal` passes its own `editDatetimeTz` prop through
 *  unchanged, which is `undefined` for the one real mount in
 *  `+page.svelte`) → **identical to the original behavior**: LOCAL
 *  `Date` getters, same ones `formatClock24` above uses, i.e. whatever
 *  zone the process/browser is actually running in. A real user's
 *  pre-filled time is byte-for-byte unchanged by this parameter's
 *  existence. Supplied → resolved via `Intl.DateTimeFormat`'s
 *  `timeZone` option instead of the ambient getters, so a test can
 *  state the exact zone it means without mutating `process.env.TZ`
 *  (vitest's default `threads` pool does not honor a mid-test
 *  reassignment of that for already-initialized `Date` local getters —
 *  see `scheduledPanel.test.ts`'s and
 *  `ScheduledPanel.editJob.test.ts`'s own call sites). */
export function toDatetimeLocalValue(raw: string | null, tz?: string): string {
  const d = parseStored(raw);
  if (!d) return '';
  if (tz) {
    const parts = new Intl.DateTimeFormat('en-US', {
      timeZone: tz,
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
      hourCycle: 'h23',
    }).formatToParts(d);
    const part = (type: string) => parts.find((p) => p.type === type)?.value ?? '';
    return `${part('year')}-${part('month')}-${part('day')}T${part('hour')}:${part('minute')}`;
  }
  const yyyy = d.getFullYear();
  const mm = String(d.getMonth() + 1).padStart(2, '0');
  const dd = String(d.getDate()).padStart(2, '0');
  const hh = String(d.getHours()).padStart(2, '0');
  const mi = String(d.getMinutes()).padStart(2, '0');
  return `${yyyy}-${mm}-${dd}T${hh}:${mi}`;
}

/** Recurring-job pre-fill: the inverse of `ScheduledPanel.svelte`'s
 *  `buildCron()` — matches exactly the 4 literal shapes that function
 *  can produce (`daily`: `MM HH * * *`; `weekdays`: `MM HH * * 1-5`;
 *  `weekly`: `MM HH * * 1`; `hourly`/every-4-hours (step-4 hour field),
 *  which discards the hour field entirely, same as `buildCron`'s own
 *  hourly branch does on the way out — so the round-tripped `time`'s
 *  hour component is a `'00'` placeholder, never read back out by
 *  `buildCron` either). Returns `null` for ANYTHING else — most
 *  plausibly a cron authored by the `schedule_job` AI tool's free-form
 *  expression, which the picker was never meant to express (§36.3.8:
 *  "a deliberate subset, not free-form cron entry"). `null` is Ruling
 *  3's exact trigger for showing the raw cron text field instead of the
 *  frequency controls — checked ONCE, at dialog-open, never on every
 *  keystroke (so the control doesn't flicker mid-edit). */
export function cronToFrequency(cron: string): { frequency: Frequency; time: string } | null {
  const parts = cron.trim().split(/\s+/);
  if (parts.length !== 5) return null;
  const [min, hour, dom, month, dow] = parts;
  if (dom !== '*' || month !== '*') return null;
  if (!/^\d{1,2}$/.test(min) || Number(min) > 59) return null;
  const mm = min.padStart(2, '0');

  if (hour === '*/4') {
    if (dow !== '*') return null;
    return { frequency: 'hourly', time: `00:${mm}` };
  }
  if (!/^\d{1,2}$/.test(hour) || Number(hour) > 23) return null;
  const hh = hour.padStart(2, '0');
  if (dow === '*') return { frequency: 'daily', time: `${hh}:${mm}` };
  if (dow === '1-5') return { frequency: 'weekdays', time: `${hh}:${mm}` };
  if (dow === '1') return { frequency: 'weekly', time: `${hh}:${mm}` };
  return null;
}

export function formatLastRun(row: ScheduledJobRow, now: Date = new Date()): string {
  if (row.status === 'running') return 'Running now';
  if (!row.started_at && !row.finished_at) return 'Not run yet';
  const when = formatScheduleDateTime(row.finished_at ?? row.started_at, now);
  if (row.status === 'failed' || row.last_error) return `${when} · failed`;
  if (row.missed_count > 0) return `${when} · ${row.missed_count} missed`;
  return when;
}

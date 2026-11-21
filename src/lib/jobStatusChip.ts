/**
 * §36.4/§36.4.5 (#3050 PR 5) — Matt's "chips show live status" addition.
 *
 * The spawn chip AND PR 4's schedule chip read `agent_jobs.status`/
 * `finished_at`/`started_at` by the `job_id` already on the message, and
 * re-render when the scheduler marks the job done ("running" → "done in
 * 48s"). This file is the PURE, side-effect-free piece of that mechanism —
 * the text-formatting logic — kept separate from `ChatPanel.svelte`'s own
 * polling wiring so it has a real Vitest surface (4d-skip only covers the
 * actual pixel walk, never the data-level formatting logic itself).
 *
 * `JobStatusSnapshot` mirrors `api.ts::JobStatusInfo` (the real
 * `get_job_status` command's return shape) — kept as a separate type here
 * so this module has zero import-time dependency on `api.ts`/Tauri's
 * `invoke`, which would otherwise make it untestable outside a Tauri
 * context.
 */

import { formatScheduleDateTime } from './scheduledPanel';

export interface JobStatusSnapshot {
  status: string;
  started_at: string | null;
  finished_at: string | null;
  /** live-test #1b fix PR, Ruling 3: non-NULL when the last firing of a
   *  recurring/heartbeat job failed transport (status stays 'pending' by
   *  design — see `chipStatusSuffix`'s 'pending' arm). Optional so existing
   *  callers/tests that predate this field still type-check. */
  last_error?: string | null;
  /** C37 (#3139, live test #4): mirrors `api.ts::JobStatusInfo.next_run_at`
   *  (the raw UTC `agent_jobs.next_run_at` stored-datetime string). Optional
   *  so existing callers/tests that predate this field still type-check;
   *  `chipDisplayText` degrades gracefully to no "next" clause when absent. */
  next_run_at?: string | null;
}

/** SQLite's own `datetime('now')` format ("YYYY-MM-DD HH:MM:SS", no
 *  offset, implicitly UTC) — parsed here the same way every other UTC
 *  stored-datetime string in this codebase is: append 'Z' so `Date`
 *  treats it as UTC rather than local time. Returns `null` on anything
 *  unparseable rather than throwing — a chip's status text degrading to
 *  "no timing available" is strictly better than a crashed component. */
function parseStoredDatetime(raw: string | null): Date | null {
  if (!raw) return null;
  const parsed = new Date(raw.replace(' ', 'T') + 'Z');
  return Number.isNaN(parsed.getTime()) ? null : parsed;
}

/** Elapsed whole seconds between two stored-datetime strings, or `null` if
 *  either is missing/unparseable or the result would be negative (clock
 *  skew / corrupted row — degrade rather than show a nonsensical duration). */
export function elapsedSeconds(startedAt: string | null, finishedAt: string | null): number | null {
  const start = parseStoredDatetime(startedAt);
  const end = parseStoredDatetime(finishedAt);
  if (!start || !end) return null;
  const secs = Math.round((end.getTime() - start.getTime()) / 1000);
  return secs >= 0 ? secs : null;
}

/** The live-status SUFFIX appended to a chip's own static text (e.g.
 *  "Scheduled: Monday reminder · next Mon 9:00 AM" or "Spawned agent on
 *  '...'"). Empty string for an unknown/pending job (the chip's own
 *  original text already says everything needed in that case — this
 *  function adds nothing, never subtracts). */
export function chipStatusSuffix(snapshot: JobStatusSnapshot | undefined): string {
  if (!snapshot) return '';
  switch (snapshot.status) {
    case 'pending':
      // Ruling 3 (live-test #1b fix PR): a recurring/heartbeat job's
      // status stays 'pending' through a transport failure by design (it
      // self-heals at the next scheduled firing) — last_error being
      // non-NULL is the only observable trace of that failure, up to the
      // 3-consecutive-failures flip to 'failed' below.
      return snapshot.last_error ? ' · last run failed' : '';
    case 'running':
      return ' · running';
    case 'done': {
      const secs = elapsedSeconds(snapshot.started_at, snapshot.finished_at);
      return secs !== null ? ` · done in ${secs}s` : ' · done';
    }
    case 'sent':
      // Ruling 4 (live-test #1b fix PR): 'sent' means Pane's own transport
      // call succeeded — it does NOT mean OC has replied yet. 'done' is
      // only reached via §36.6 pickup actually matching OC's reply
      // (runtime.md ~line 408). Rendering an elapsed-time "done in Xs" here
      // claimed a completion that never happened; 'sent' makes no duration
      // claim at all.
      return ' · sent';
    case 'failed':
      return ' · failed';
    case 'cancelled':
      return ' · cancelled';
    case 'paused':
      return ' · paused';
    default:
      return '';
  }
}

/** The full chip display text: the message's own static `content`, plus
 *  a freshly-computed "next" clause (C37, below), plus the live-status
 *  suffix. This is the function `ChatPanel.svelte`'s chip template
 *  actually calls — pure, so it re-renders automatically whenever
 *  Svelte's own reactivity sees `snapshot` change (no manual DOM poke
 *  needed), and testable here with zero Tauri/DOM dependency.
 *
 *  C37 (#3139, live test #4): the "next" clause used to be baked into
 *  `messageContent` itself, server-side, in raw UTC, at chip-creation time
 *  (`execute_schedule_job`'s old `format!("Scheduled: {} · next {}", ...)`)
 *  — never reformatted client-side, so it always showed in UTC regardless
 *  of the viewer's local timezone. Fixed by REMOVING that clause from the
 *  stored content (Rust side, `ai::tools::execute_schedule_job`) and
 *  computing it HERE instead, at render time, from `snapshot.next_run_at`
 *  (newly plumbed onto `get_job_status`'s return value) — reusing
 *  `formatScheduleDateTime` (`scheduledPanel.ts`), imported below, which
 *  already renders local Today/Tomorrow/weekday time correctly and is
 *  already tested (`scheduledPanel.test.ts`). Zero new date-formatting
 *  logic is written here — one formatter, two consumers. Degrades
 *  gracefully to no "next" clause when no snapshot has loaded yet, or the
 *  snapshot carries no `next_run_at` (a paused job, or an older
 *  snapshot shape) — same "don't invent text the data doesn't support"
 *  convention `chipStatusSuffix` already follows for an unknown status. */
export function chipDisplayText(messageContent: string, snapshot: JobStatusSnapshot | undefined): string {
  const nextClause = snapshot?.next_run_at ? ` · next ${formatScheduleDateTime(snapshot.next_run_at)}` : '';
  return messageContent + nextClause + chipStatusSuffix(snapshot);
}

/** Whether a job's status is terminal — once true, the polling loop in
 *  `ChatPanel.svelte` should stop polling THIS job_id (no further status
 *  change is possible). Exported so the polling wiring and this module
 *  agree on exactly one definition of "terminal," rather than each
 *  hand-rolling its own list. */
export function isTerminalJobStatus(status: string | undefined): boolean {
  return status === 'done' || status === 'sent' || status === 'failed' || status === 'cancelled';
}

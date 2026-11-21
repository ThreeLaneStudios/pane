/**
 * ScheduledPanel.editJob.test.ts — #3134 C26 (Matt's four parts: a new
 * `update_job` command, Edit on Active rows opening the New-job dialog
 * pre-filled, the two inverse formatters, no edit while running).
 *
 * Reed's two explicitly-named tests live here:
 * `test_raw_cron_field_round_trips_unchanged_through_save` (the
 * no-fidelity-loss claim) and the streak-reset test lives in the
 * backend (`ai/tools.rs`'s `update_job_tests_3134_c26` module) since the
 * reset itself is a pure backend SQL concern.
 */

import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, fireEvent, waitFor } from '@testing-library/svelte/svelte5';
import ScheduledPanel from './ScheduledPanel.svelte';

beforeAll(() => {
  globalThis.ResizeObserver = class ResizeObserver {
    observe() {}
    unobserve() {}
    disconnect() {}
  };

  const win = globalThis.window as any;
  if (!win.addEventListener) {
    win.addEventListener = globalThis.addEventListener?.bind(globalThis) ?? (() => {});
  }
  if (!win.removeEventListener) {
    win.removeEventListener = globalThis.removeEventListener?.bind(globalThis) ?? (() => {});
  }
  if (!win.document) {
    win.document = globalThis.document;
  }
});

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock('$lib/services/navigation', () => ({
  navigateTo: vi.fn(),
}));

vi.mock('$lib/stores/app', () => {
  const { writable } = require('svelte/store');
  return { projects: writable([]) };
});

function activeRow(overrides: Partial<any> = {}) {
  return {
    id: 'job-active-1',
    title: 'Morning brief',
    kind: 'once',
    agent: 'Brit Freed',
    agent_id: 'agent-brit',
    project: 'Home',
    project_id: 'workspace',
    next_run_at: '2026-12-01 14:00:00',
    status: 'pending',
    missed_count: 0,
    started_at: null,
    finished_at: null,
    last_error: null,
    session_id: 'session-active-1',
    brief: 'The stored brief text',
    cron: null,
    run_at: '2026-12-01 14:00:00',
    ...overrides,
  };
}

const {
  listScheduledJobsMock,
  getAgentsMock,
  createScheduledJobMock,
  updateScheduledJobMock,
} = vi.hoisted(() => ({
  listScheduledJobsMock: vi.fn(),
  getAgentsMock: vi.fn(),
  createScheduledJobMock: vi.fn(),
  updateScheduledJobMock: vi.fn(),
}));

vi.mock('$lib/api', () => ({
  listScheduledJobs: listScheduledJobsMock,
  waitForRunningJobToStop: vi.fn().mockResolvedValue(undefined),
  pauseScheduledJob: vi.fn().mockResolvedValue(undefined),
  resumeScheduledJob: vi.fn().mockResolvedValue(undefined),
  deleteScheduledJob: vi.fn().mockResolvedValue(undefined),
  getAgents: getAgentsMock,
  createScheduledJob: createScheduledJobMock,
  updateScheduledJob: updateScheduledJobMock,
  runJobNow: vi.fn(),
  switchSession: vi.fn(),
}));

beforeEach(() => {
  listScheduledJobsMock.mockReset().mockResolvedValue([activeRow()]);
  getAgentsMock.mockReset().mockResolvedValue([{ id: 'agent-brit', name: 'Brit Freed' }]);
  createScheduledJobMock.mockReset().mockResolvedValue({ job_id: 'job-active-1', next_run_at: '2026-12-01 14:00:00' });
  updateScheduledJobMock.mockReset().mockResolvedValue({ job_id: 'job-active-1', next_run_at: '2026-12-01 14:00:00' });
});

describe('#3134 C26 — Edit button gating', () => {
  /// Named test (task item 4, "no edit while running"): Edit is
  /// disabled (not removed) on a running once/recurring row, and absent
  /// entirely for heartbeat/reminder kinds — neither is offered Edit at
  /// all (unscoped special-case machinery). Mutation (run for this PR,
  /// see report): drop the `disabled={row.status === 'running'}` binding
  /// — the running-row assertion below reddens (button no longer
  /// disabled); drop the `{#if row.kind === 'once' || row.kind ===
  /// 'recurring'}` guard — the heartbeat/reminder "absent" assertions
  /// redden (an Edit button renders for them).
  it('is disabled while running, and absent for heartbeat/reminder rows', async () => {
    listScheduledJobsMock.mockResolvedValue([
      activeRow({ id: 'job-once', kind: 'once', status: 'pending' }),
      activeRow({ id: 'job-running', kind: 'recurring', status: 'running', cron: '0 9 * * *' }),
      activeRow({ id: 'job-heartbeat', kind: 'heartbeat', status: 'pending', cron: '0 */4 * * *' }),
      activeRow({ id: 'job-reminder', kind: 'reminder', status: 'pending' }),
    ]);
    const { container, getAllByTitle } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());
    await waitFor(() => expect(container.querySelectorAll('[data-testid="scheduled-row"]').length).toBe(4));

    const editButtons = getAllByTitle('Edit');
    // Only the once and recurring rows get an Edit button at all.
    expect(editButtons.length).toBe(2);

    const runningRowEl = container.querySelector('[data-job-id="job-running"]');
    const runningEditBtn = runningRowEl?.querySelector('[data-testid="edit-btn"]') as HTMLButtonElement | null;
    expect(runningEditBtn).toBeTruthy();
    expect(runningEditBtn?.disabled).toBe(true);

    const onceRowEl = container.querySelector('[data-job-id="job-once"]');
    const onceEditBtn = onceRowEl?.querySelector('[data-testid="edit-btn"]') as HTMLButtonElement | null;
    expect(onceEditBtn?.disabled).toBe(false);

    const heartbeatRowEl = container.querySelector('[data-job-id="job-heartbeat"]');
    expect(heartbeatRowEl?.querySelector('[data-testid="edit-btn"]')).toBeNull();
    const reminderRowEl = container.querySelector('[data-job-id="job-reminder"]');
    expect(reminderRowEl?.querySelector('[data-testid="edit-btn"]')).toBeNull();
  });
});

describe('#3151 PR1 Commit G / #3171 PR 2 — Agent field is locked while editing', () => {
  /// Named test (the frontend half of G, carried through #3171 PR 2's
  /// SearchableSelect extraction): the Agent field's underlying input
  /// gets `disabled` the moment Edit is open — Project stays editable
  /// (no such gate on its own sibling field). Mutation: drop the
  /// `disabled={editingJobId !== null}` binding on the Agent
  /// SearchableSelect — reddens (the input is enabled during Edit).
  it('disables the Agent field when editing, enabled when creating', async () => {
    listScheduledJobsMock.mockResolvedValue([
      activeRow({ id: 'job-once', kind: 'once', status: 'pending', run_at: '2026-12-01 14:00:00' }),
    ]);
    const { container, getByTitle, getByText, getByTestId } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());

    // New job: Agent field is enabled.
    await fireEvent.click(getByText('New job'));
    await waitFor(() => expect(container.querySelector('[data-testid="new-schedule-modal"]')).toBeTruthy());
    const newJobAgentInput = getByTestId('agent-search') as HTMLInputElement;
    expect(newJobAgentInput.disabled).toBe(false);
    await fireEvent.click(getByText('Cancel'));

    // Edit: Agent field is disabled; Project field is NOT.
    const editBtn = await waitFor(() => getByTitle('Edit'));
    await fireEvent.click(editBtn);
    await waitFor(() => expect(container.querySelector('[data-testid="new-schedule-modal"]')).toBeTruthy());

    const editAgentInput = getByTestId('agent-search') as HTMLInputElement;
    const editProjectInput = getByTestId('project-search') as HTMLInputElement;
    expect(editAgentInput.disabled, 'Agent field must be disabled while editing').toBe(true);
    expect(editProjectInput.disabled, 'Project field must stay editable while editing').toBe(false);
  });

  /// The hint text appears only in edit mode.
  it('shows the "Change agent = new job" hint only while editing', async () => {
    listScheduledJobsMock.mockResolvedValue([
      activeRow({ id: 'job-once', kind: 'once', status: 'pending', run_at: '2026-12-01 14:00:00' }),
    ]);
    const { container, getByTitle, getByText, queryByText } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());

    await fireEvent.click(getByText('New job'));
    await waitFor(() => expect(container.querySelector('[data-testid="new-schedule-modal"]')).toBeTruthy());
    expect(queryByText('Change agent = new job')).toBeNull();
    await fireEvent.click(getByText('Cancel'));

    const editBtn = await waitFor(() => getByTitle('Edit'));
    await fireEvent.click(editBtn);
    await waitFor(() => expect(container.querySelector('[data-testid="new-schedule-modal"]')).toBeTruthy());
    expect(getByText('Change agent = new job')).toBeTruthy();
  });
});

describe('#3134 C26 — openEditJobModal pre-fill', () => {
  /// Named test: a once job's Edit pre-fill uses the LOCAL hour, not
  /// the stored UTC hour — catches a regression at the call site (not
  /// just `toDatetimeLocalValue`'s own unit test). Zone is pinned via
  /// the `editDatetimeTz` prop (Matt's ruling, post-merge TZ-mocking
  /// fix) — an explicit injection into the formatter under test
  /// through the one call site that invokes it, rather than mutating
  /// `process.env.TZ` mid-test (which vitest's default `threads` pool
  /// does not honor for already-initialized `Date` local getters). The
  /// prop defaults to `undefined` and the real `+page.svelte` mount
  /// never passes it, so production behavior is unchanged. Mutation:
  /// have `openEditJobModal` pass `row.next_run_at` through
  /// `.slice(0, 16).replace(' ', 'T')` (a UTC-literal slice) instead of
  /// `toDatetimeLocalValue(row.run_at, editDatetimeTz)` — reddens (the
  /// datetime-local input would show the UTC hour, "14:00", not the
  /// local one).
  it('prefills the once-job datetime-local input with the LOCAL time, not UTC', async () => {
    listScheduledJobsMock.mockResolvedValue([
      activeRow({ id: 'job-once', kind: 'once', status: 'pending', run_at: '2026-06-15 18:00:00' }),
    ]);
    const { container, getByTitle } = render(ScheduledPanel, { props: { editDatetimeTz: 'America/New_York' } });
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());

    const editBtn = await waitFor(() => getByTitle('Edit'));
    await fireEvent.click(editBtn);
    await waitFor(() => expect(container.querySelector('[data-testid="new-schedule-modal"]')).toBeTruthy());

    const dtInput = container.querySelector('input[type="datetime-local"]') as HTMLInputElement;
    expect(dtInput).toBeTruthy();
    // 2026-06-15 18:00:00 UTC == 2026-06-15 14:00 EDT (UTC-4, DST).
    expect(dtInput.value).toBe('2026-06-15T14:00');
  });

  /// Named test: a recurring job whose cron matches `buildCron()`'s
  /// "daily" shape pre-fills the frequency picker (not the raw field)
  /// with the right frequency/time. Mutation: skip calling
  /// `cronToFrequency` in `openEditJobModal` (always treat as raw) —
  /// this test's "picker visible, raw field absent" assertion reddens.
  it('prefills frequency + time for a recurring daily cron, picker visible', async () => {
    listScheduledJobsMock.mockResolvedValue([
      activeRow({ id: 'job-recurring', kind: 'recurring', status: 'pending', cron: '30 9 * * *', run_at: null }),
    ]);
    const { container, getByTitle } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());

    const editBtn = await waitFor(() => getByTitle('Edit'));
    await fireEvent.click(editBtn);
    await waitFor(() => expect(container.querySelector('[data-testid="new-schedule-modal"]')).toBeTruthy());

    expect(container.querySelector('[data-testid="raw-cron-input"]')).toBeNull();
    const freqSelect = container.querySelector('[data-testid="frequency-select"]') as HTMLSelectElement;
    expect(freqSelect).toBeTruthy();
    expect(freqSelect.value).toBe('daily');
    const timeInput = container.querySelector('input[type="time"]') as HTMLInputElement;
    expect(timeInput.value).toBe('09:30');
  });

  /// Named test (Ruling 3's own trigger): a recurring job whose cron
  /// does NOT match any of the 4 known buckets shows the raw text
  /// field, holding the stored cron, instead of the picker. Mutation:
  /// have `openEditJobModal` fall back to the picker with a default
  /// frequency instead of setting `formRawCron` when `cronToFrequency`
  /// returns null — this test's "raw field visible with the right
  /// value" assertion reddens.
  it('shows the raw cron field for a non-expressible cron, holding the stored value', async () => {
    listScheduledJobsMock.mockResolvedValue([
      activeRow({ id: 'job-weird-cron', kind: 'recurring', status: 'pending', cron: '7 3 * * 2,4', run_at: null }),
    ]);
    const { container, getByTitle } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());

    const editBtn = await waitFor(() => getByTitle('Edit'));
    await fireEvent.click(editBtn);
    await waitFor(() => expect(container.querySelector('[data-testid="new-schedule-modal"]')).toBeTruthy());

    const rawInput = container.querySelector('[data-testid="raw-cron-input"]') as HTMLInputElement;
    expect(rawInput).toBeTruthy();
    expect(rawInput.value).toBe('7 3 * * 2,4');
    // The normal frequency picker must be GONE, not just hidden alongside it.
    expect(container.querySelector('[data-testid="frequency-select"]')).toBeNull();
  });
});

describe('#3134 C26 — the fidelity test (Reed\'s explicit ask)', () => {
  /// THE required fidelity test: opening Edit on a non-expressible-cron
  /// row and saving WITHOUT touching the raw field must round-trip the
  /// cron byte-for-byte — never re-derived through `buildCron()`.
  /// Mutation (run for this PR, see report): have the submit branch
  /// always call `buildCron(formFrequency, formTime)` regardless of
  /// `formRawCron` — the cron argument asserted below reddens (observes
  /// a default bucket shape like `"0 9 * * *"` instead of the original
  /// `"7 3 * * 2,4"`).
  it('round-trips a non-expressible cron unchanged when the raw field is untouched', async () => {
    listScheduledJobsMock.mockResolvedValue([
      activeRow({ id: 'job-weird-cron', kind: 'recurring', status: 'pending', cron: '7 3 * * 2,4', run_at: null, title: 'Weird cron job', agent_id: 'agent-brit', project_id: 'workspace' }),
    ]);
    const { container, getByTitle, getByText } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());

    const editBtn = await waitFor(() => getByTitle('Edit'));
    await fireEvent.click(editBtn);
    await waitFor(() => expect(container.querySelector('[data-testid="new-schedule-modal"]')).toBeTruthy());
    expect((container.querySelector('[data-testid="raw-cron-input"]') as HTMLInputElement).value).toBe('7 3 * * 2,4');

    await fireEvent.click(getByText('Save changes'));

    await waitFor(() => expect(updateScheduledJobMock).toHaveBeenCalled());
    const [, params] = updateScheduledJobMock.mock.calls[0];
    expect(params.cron).toBe('7 3 * * 2,4');
  });
});

describe('#3134 C26 — submit routing', () => {
  /// Named test (also the real coverage for `api.updateScheduledJob`'s
  /// wrapper, per its own doc comment): editing and saving calls
  /// `updateScheduledJob`, never `createScheduledJob` — proves the one
  /// branch point in `submitNewSchedule()` routes correctly. Mutation:
  /// drop the `editingJobId !== null` branch — `createScheduledJobMock`
  /// would be called instead, reddening the "never called" assertion.
  it('calls updateScheduledJob, not createScheduledJob, when editing', async () => {
    listScheduledJobsMock.mockResolvedValue([
      activeRow({ id: 'job-once', kind: 'once', status: 'pending', run_at: '2026-12-01 14:00:00' }),
    ]);
    const { container, getByTitle, getByText } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());

    const editBtn = await waitFor(() => getByTitle('Edit'));
    await fireEvent.click(editBtn);
    await waitFor(() => expect(container.querySelector('[data-testid="new-schedule-modal"]')).toBeTruthy());

    await fireEvent.click(getByText('Save changes'));

    await waitFor(() => expect(updateScheduledJobMock).toHaveBeenCalledWith('job-once', expect.objectContaining({ brief: 'The stored brief text' })));
    expect(createScheduledJobMock).not.toHaveBeenCalled();
  });
});

describe('#3134 C26 — not a silent third edit mode', () => {
  /// Named test: opening "New job" — even right after closing an Edit
  /// session that was showing the raw cron field — never shows the raw
  /// field; the New-job flow always uses the normal picker. Mutation:
  /// drop the `formRawCron = null` reset in `openNewScheduleModal` —
  /// this test's "raw field absent" assertion reddens (a leftover raw
  /// cron input from the prior Edit session would still render).
  it('never shows the raw cron field when opening New job, even after a raw-cron Edit session', async () => {
    listScheduledJobsMock.mockResolvedValue([
      activeRow({ id: 'job-weird-cron', kind: 'recurring', status: 'pending', cron: '7 3 * * 2,4', run_at: null }),
    ]);
    const { container, getByTitle, getAllByText, getByText } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());

    // Open the raw-cron Edit session, then cancel out of it.
    const editBtn2 = await waitFor(() => getByTitle('Edit'));
    await fireEvent.click(editBtn2);
    await waitFor(() => expect(container.querySelector('[data-testid="raw-cron-input"]')).toBeTruthy());
    await fireEvent.click(getAllByText('Cancel')[0]);
    await waitFor(() => expect(container.querySelector('[data-testid="new-schedule-modal"]')).toBeNull());

    // Now open New job.
    await fireEvent.click(getByText('New job'));
    await waitFor(() => expect(container.querySelector('[data-testid="new-schedule-modal"]')).toBeTruthy());

    expect(container.querySelector('[data-testid="raw-cron-input"]')).toBeNull();
    expect(container.querySelector('h3')?.textContent).toBe('New job');
  });
});

describe('leftover 1 (#3139, live test #4) — "Create schedule" button renamed to "Create job"', () => {
  /// Mutation: revert the submit button's non-editing label back to
  /// "Create schedule" — reddens (the new text is never found).
  it('shows "Create job" as the submit button label when creating a new job (not editing)', async () => {
    listScheduledJobsMock.mockResolvedValue([]);
    const { getByText, queryByText } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());

    await fireEvent.click(getByText('New job'));

    await waitFor(() => expect(getByText('Create job')).toBeTruthy());
    expect(queryByText('Create schedule')).toBeNull();
  });

  /// Regression guard: the Edit flow's own label ("Save changes") is
  /// untouched by this rename — only the create-mode label changed.
  it('still shows "Save changes" (unchanged) when editing an existing job', async () => {
    listScheduledJobsMock.mockResolvedValue([
      activeRow({ id: 'job-rename-check', kind: 'once', status: 'pending', run_at: '2026-12-01 14:00:00' }),
    ]);
    const { container, getByTitle, getByText } = render(ScheduledPanel, {});
    await waitFor(() => expect(listScheduledJobsMock).toHaveBeenCalled());

    const editBtn = await waitFor(() => getByTitle('Edit'));
    await fireEvent.click(editBtn);
    await waitFor(() => expect(container.querySelector('[data-testid="new-schedule-modal"]')).toBeTruthy());

    expect(getByText('Save changes')).toBeTruthy();
  });
});

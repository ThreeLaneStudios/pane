<script lang="ts">
  /**
   * ScheduledPanel.svelte — Home → Scheduled panel (#3050 PR 8, §36.3.8).
   *
   * DOCK/SQUISH (design sheet README "Layout rules" + Notes #1/#2/#4):
   * a fixed 352px column on the LEFT of Home, at every window width. No
   * toggle, no overlay. Mounted in `+page.svelte` as the FIRST flex-row
   * child inside `.chat-panel.home-view`, BEFORE `<ChatSidebar
   * layoutMode="centered" />` — the exact same "fixed-width dock,
   * flex:1 sibling absorbs the squish" mechanism `SessionsDrawer.svelte`
   * already established for Home (its own file header, lines 5-8), just
   * mirrored onto the left side (this panel) instead of the right
   * (Sessions drawer, which keeps overlaying from the right, unchanged).
   * `width: 352px; flex-shrink: 0;` below is the whole mechanism — no
   * JS, no toggle state, no resize handle.
   *
   * QUERY: `api.listScheduledJobs()` — `ai::scheduler::list_scheduled_jobs`,
   * a bare command (not the `list_jobs` AI tool — see that command's own
   * doc comment for why). Returns every `agent_jobs` row except
   * `kind='spawn'`, across every project. Tab partition/sort/visual
   * state are `scheduledPanel.ts`'s pure functions (own Vitest coverage).
   *
   * ROW CLICK: `switch_session` (PR 6's origin-guarded
   * `demote_and_promote`) + `navigateTo` — the EXACT frontend path
   * `NotificationPanel.svelte`'s `target_kind === 'session'` case
   * already uses for a job-origin session (§36.7). Never a new command,
   * never a new guard.
   *
   * PRESETS/NEW SCHEDULE: `api.createScheduledJob()` — a thin wrapper
   * over `execute_schedule_job`, the SAME function the `schedule_job` AI
   * tool calls. Shared create path (task item 5).
   *
   * 24-HOUR TIME: Matt's explicit override of the design sheet's 12-hour
   * rendering (`9:00 AM`) — `scheduledPanel.ts`'s formatters never emit
   * AM/PM.
   *
   * EDIT (#3134 C26): `openEditJobModal(row)` is a sibling to
   * `openNewScheduleModal(preset)` below, reusing the SAME form-state
   * variables and the SAME modal/submit machinery — no dialog refactor.
   * Offered on Active rows for `kind IN ('once', 'recurring')` only
   * (`heartbeat`/`reminder` carry enough special-case machinery that
   * editing them is unscoped, separate work), disabled while
   * `status === 'running'` (flat refuse, same shape as the backend's own
   * `WHERE status != 'running'` guard — never a wait-then-edit dance).
   * Saving calls `api.updateScheduledJob(editingJobId, ...)` instead of
   * `api.createScheduledJob(...)` — the one branch point in
   * `submitNewSchedule()`. A recurring job whose stored cron doesn't
   * match any of `buildCron()`'s 4 literal shapes
   * (`cronToFrequency(row.cron)` returns `null`) shows that cron as a
   * raw text field (`formRawCron`) in place of the frequency/time
   * controls — checked ONCE at `openEditJobModal` time, never on every
   * keystroke. `openNewScheduleModal` always resets `editingJobId` and
   * `formRawCron` to `null` so a fresh "New job" open can never show a
   * leftover raw-cron field from a prior Edit session (not a silent
   * third edit mode — same dialog, one control conditionally swapped).
   */
  import { onMount, onDestroy } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import * as api from '$lib/api';
  import { navigateTo } from '$lib/services/navigation';
  import { get } from 'svelte/store';
  import { projects, activeChatThreadId } from '$lib/stores/app';
  import { recordJobSessionEntry } from '$lib/services/jobSessionBack';
  import { sessionStore } from '$lib/stores/sessionStore';
  import type { Session } from '$lib/types';
  import { matchesSearchQuery } from '$lib/utils/searchMatch';
  import SearchableSelect from './SearchableSelect.svelte';
  import {
    partitionScheduledJobs,
    sortActiveRows,
    sortDoneRows,
    visibleDoneRows,
    toDatetimeLocalValue,
    cronToFrequency,
    rowVisualState,
    formatNextRun,
    formatLastRun,
    openScheduledJobSession,
    type ScheduledJobRow,
  } from '$lib/scheduledPanel';

  /** #3134 C26 post-merge TZ-mocking fix (Matt's ruling): a test-only
   *  injection point for `toDatetimeLocalValue`'s explicit-zone input
   *  (see that function's own doc comment in `scheduledPanel.ts`).
   *  The one real mount (`+page.svelte`'s bare `<ScheduledPanel />`)
   *  never passes this, so it's `undefined` there and
   *  `openEditJobModal` below falls back to `toDatetimeLocalValue`'s
   *  original ambient-`Date`-getter behavior — a real user's Edit
   *  pre-fill is byte-for-byte unchanged by this prop's existence.
   *  Exists ONLY so `ScheduledPanel.editJob.test.ts`'s once-job
   *  pre-fill test can pin a zone explicitly instead of mutating
   *  `process.env.TZ` mid-test. */
  interface Props {
    editDatetimeTz?: string;
  }
  let { editDatetimeTz }: Props = $props();

  let jobs: ScheduledJobRow[] = $state([]);
  let loading = $state(true);
  let loadError: string | null = $state(null);
  let activeTab: 'active' | 'done' = $state('active');
  let confirmDeleteId: string | null = $state(null);
  let runNowInFlight: Set<string> = $state(new Set());
  // C31 (#3128, UI half): a refused Run now (e.g. "Run now is disabled
  // while a run is already in flight") previously only reached
  // console.error — nothing in the DOM changed. Keyed by row id (not a
  // single scalar) since more than one row's Run now could be in flight
  // at once.
  let runNowError: Map<string, string> = $state(new Map());
  // §36.3.7 carry-over (#3050 PR 9) — rows currently waiting on
  // `waitForRunningJobToStop` before their Pause/Delete action actually
  // applies. Separate from `runNowInFlight` (a different action, same
  // "disable + show it's working" shape).
  let stoppingInFlight: Set<string> = $state(new Set());
  // §3151 PR2/C24 — the Done tab's own "Show deleted" toggle (NOT
  // SessionsDrawer's unrelated hidden_at toggle). Default false: Done
  // shows completed+failed only, matching the tab name.
  let showDeleted = $state(false);
  let clearingDone = $state(false);
  // §3151 PR2/C24 — Delete on a Done card: hard delete, its own
  // confirm-id state (separate from the Active tab's `confirmDeleteId`
  // above — different command, different guard, never conflated).
  let confirmDeleteDoneId: string | null = $state(null);

  let showModal = $state(false);
  let selectedPresetId: string | null = $state(null);
  let formName = $state('');
  let formBrief = $state('');
  let formProjectId = $state('workspace');
  let formAgentId = $state('');
  let formFrequency: 'once' | 'daily' | 'weekdays' | 'hourly' | 'weekly' = $state('daily');
  let formTime = $state('09:00');
  let formOnceDateTime = $state('');
  let formError: string | null = $state(null);
  let formSubmitting = $state(false);
  // #3134 C26 — Edit: `null` means "New job" (the only state before this
  // PR). Non-null is the job id being edited; `submitNewSchedule()`
  // branches on this to call `updateScheduledJob` instead of
  // `createScheduledJob`.
  let editingJobId: string | null = $state(null);
  // `null` = the normal frequency/time picker. A non-null string is a
  // stored cron `cronToFrequency` couldn't classify into one of the 4
  // known buckets — the raw text field replaces the picker, holding
  // this value verbatim (Ruling 3).
  let formRawCron: string | null = $state(null);

  let agentsList: { id: string; name: string }[] = $state([]);

  // #3171 PR 2 (C51/C57): the New job dialog's Project/Agent fields now
  // use the SAME generic SearchableSelect component the AI-tab model
  // pickers use (Matt's ruling — "it has to be the same component").
  // `formProjectItems`/`formAgentItems` are the full, unfiltered
  // {id, label} option lists — SearchableSelect does its own search
  // filtering internally via `filterItem` below; the root-projects-only
  // filter (C56) still runs here since it's unrelated to search.
  let formProjectItems = $derived([
    { id: 'workspace', label: 'Home' },
    ...$projects.filter(p => !p.parent_id).map(p => ({ id: p.id, label: p.name })),
  ]);
  let formAgentItems = $derived(agentsList.map(a => ({ id: a.id, label: a.name })));

  function formItemDisplayText(items: { id: string; label: string }[], id: string): string {
    return items.find(i => i.id === id)?.label ?? '';
  }
  // The "Home" sentinel row is always shown regardless of the typed
  // query — matches the pre-extraction behavior, where "Home" was a
  // hardcoded `<option>` outside the search-filtered `{#each}` entirely.
  function formProjectFilterItem(item: { id: string }, lowerSearch: string): boolean {
    const labelled = item as { id: string; label: string };
    return labelled.id === 'workspace' || matchesSearchQuery(labelled.label, lowerSearch);
  }
  function formAgentFilterItem(item: { id: string }, lowerSearch: string): boolean {
    const labelled = item as { id: string; label: string };
    return matchesSearchQuery(labelled.label, lowerSearch);
  }
  // Preserves the original list order (fixture/backend order, never
  // re-sorted) — the native `<select>`s this replaces never sorted
  // their `<option>`s either. Still used unchanged by the Agent field
  // (no pinned row, no ordering complaint for it).
  function formItemsUnsorted(items: { id: string }[]): { id: string }[] {
    return items;
  }

  // #3178 (C57, third attempt) — the Project field's own ordering fix.
  // Reed's audit: C56's pin ("Home always visible regardless of query",
  // delivered entirely by `formProjectFilterItem`'s unconditional
  // pass-through, untouched by this function) and "typed match first"
  // are two different requirements `formItemsUnsorted` above can't
  // satisfy together — pinning-without-reordering and relevance-
  // ordering are mutually exclusive once more than one item matches.
  // Keeps the pin (Home is never filtered OUT — that's `filterItem`'s
  // job, not this one) but drops "always first": when a query is
  // typed, an item whose label actually matches ranks ahead of Home
  // when Home itself doesn't match. Home participates like any other
  // item — a query that DOES match "home" leaves it wherever a stable
  // sort puts it among the other matches (first, since it's first in
  // the source array), preserving "type nothing or type 'home' to find
  // it" exactly as before.
  function formProjectSortItems(items: { id: string }[], lowerSearch: string): { id: string }[] {
    if (!lowerSearch) return items; // empty query: unaffected, Home stays first (unsorted order).
    return [...items].sort((a, b) => {
      const aMatches = matchesSearchQuery((a as { id: string; label: string }).label, lowerSearch);
      const bMatches = matchesSearchQuery((b as { id: string; label: string }).label, lowerSearch);
      if (aMatches === bMatches) return 0; // stable sort: original relative order preserved within each group.
      return aMatches ? -1 : 1;
    });
  }

  const active = $derived(sortActiveRows(partitionScheduledJobs(jobs).active));
  // §3151 PR2/C24: `doneAll` is the full Done-tab dataset (including
  // cancelled — archived rows never reach `jobs` at all, the backend
  // already excludes them); `done` is what's actually rendered, after
  // the "Show deleted" toggle's filter. "Clear done" archives `doneAll`
  // — the full dataset, not just what the toggle happens to be showing
  // (see `api.clearDoneJobs`'s own doc comment for the reasoning).
  const doneAll = $derived(sortDoneRows(partitionScheduledJobs(jobs).done));
  const done = $derived(visibleDoneRows(doneAll, showDeleted));

  /** §36.3.8 (Check-in's named heartbeat default, docs/spec/runtime.md:167). */
  const HEARTBEAT_DEFAULT_BRIEF =
    "Review this project's open tasks and recent notes. If anything needs doing or deciding, do it or tell me. If nothing does, reply 'Nothing needed.' and stop.";

  interface Preset {
    id: string;
    name: string;
    subtitle: string;
    brief: string;
    frequency: 'daily' | 'hourly' | 'weekly';
    time: string;
    kind?: 'heartbeat';
  }

  const PRESETS: Preset[] = [
    {
      id: 'morning-brief',
      name: 'Morning brief',
      subtitle: "What's due, what changed, what's overdue.",
      brief:
        'Every morning, list what\u2019s due today and this week across my projects, flag anything overdue, and summarize what changed in my notes since yesterday. Keep it under 150 words and end with the one thing I should do first.',
      frequency: 'daily',
      time: '07:00',
    },
    {
      id: 'check-in',
      name: 'Check in',
      subtitle: 'A short status note every few hours.',
      brief: HEARTBEAT_DEFAULT_BRIEF,
      frequency: 'hourly',
      time: '09:00',
      kind: 'heartbeat',
    },
    {
      id: 'weekly-review',
      name: 'Weekly review',
      subtitle: 'Done, slipped, and next week\u2019s plan.',
      brief: 'Review this week: what got done, what slipped, and sketch next week\u2019s plan. Keep it concise.',
      frequency: 'weekly',
      time: '09:00',
    },
    {
      id: 'custom',
      name: 'Custom',
      subtitle: 'Start with a blank brief.',
      brief: '',
      frequency: 'daily',
      time: '09:00',
    },
  ];

  async function loadJobs() {
    loading = true;
    loadError = null;
    try {
      jobs = await api.listScheduledJobs();
    } catch (e) {
      loadError = e instanceof Error ? e.message : String(e);
      console.error('[ScheduledPanel] listScheduledJobs failed:', e);
    } finally {
      loading = false;
    }
  }

  let unlistenChip: (() => void) | null = null;

  onMount(() => {
    loadJobs();
    api.getAgents().then((a) => { agentsList = a.map((x) => ({ id: x.id, name: x.name })); }).catch((e) => {
      console.error('[ScheduledPanel] getAgents failed:', e);
    });
    // §8a sibling-parity (Reed's PR 8 scope audit): the panel is the
    // SECOND consumer of `pane-chip-changed` (ChatPanel.svelte is the
    // first) — not a re-introduction of anything removed. Any status
    // flip (pause/resume/run-now/a job finishing) refreshes the list.
    listen('pane-chip-changed', () => {
      void loadJobs();
    }).then((fn) => { unlistenChip = fn; });
  });

  onDestroy(() => {
    unlistenChip?.();
  });

  async function openRow(row: ScheduledJobRow) {
    try {
      await openScheduledJobSession(row, {
        switchSession: api.switchSession,
        navigateTo,
        // #3151 PR1 Commit F: the Jobs card entry point for the one-deep
        // back button/Cmd+[ — see jobSessionBack.ts's own doc comment.
        getCurrentSessionId: () => get(activeChatThreadId),
        recordBackTarget: recordJobSessionEntry,
        // #3166 C58: the Jobs card bypasses SessionDropdownHeader's own
        // switch path too — same fix as the bell (NotificationPanel.svelte).
        // `SessionOpenDeps.setActiveSession`'s own param type is the same
        // deliberately-narrow `{id, project_id}` shape `switchSession`'s
        // own dep type uses (test-mock convenience) — the real backend
        // `switch_session` command this always feeds from (`api.switchSession`,
        // typed `Promise<Session>` in api.ts) genuinely returns a full
        // `Session`, so this cast is honest at the one real call site,
        // not a widening of the test-facing interface.
        setActiveSession: (session) => sessionStore.setActiveSessionFromSwitch(session as Session),
      });
    } catch (e) {
      console.error('[ScheduledPanel] openRow failed:', e);
    }
  }

  async function handleRunNow(e: MouseEvent, row: ScheduledJobRow) {
    e.stopPropagation();
    if (row.status === 'running' || runNowInFlight.has(row.id)) return;
    runNowInFlight = new Set([...runNowInFlight, row.id]);
    // C31 (#3128, UI half): clear any stale error from a previous refusal
    // before retrying — a fresh attempt shouldn't still show the last one.
    if (runNowError.has(row.id)) {
      const cleared = new Map(runNowError);
      cleared.delete(row.id);
      runNowError = cleared;
    }
    try {
      await api.runJobNow(row.id);
      await loadJobs();
    } catch (e) {
      console.error('[ScheduledPanel] runJobNow failed:', e);
      const next = new Map(runNowError);
      next.set(row.id, e instanceof Error ? e.message : String(e));
      runNowError = next;
    } finally {
      const next = new Set(runNowInFlight);
      next.delete(row.id);
      runNowInFlight = next;
    }
  }

  async function handlePauseResume(e: MouseEvent, row: ScheduledJobRow) {
    e.stopPropagation();
    if (stoppingInFlight.has(row.id)) return;
    try {
      if (row.status === 'running') {
        // §36.3.7 carry-over (#3050 PR 9, Matt's carry-over ruling): no
        // optimistic row flip. Wait for the job to actually leave
        // 'running' (API-runtime: cancel flag + poll; OC-runtime: poll
        // only, no checkpoint to flag) BEFORE applying Pause — never a
        // flat, unactionable refusal the way this used to be.
        stoppingInFlight = new Set([...stoppingInFlight, row.id]);
        try {
          await api.waitForRunningJobToStop(row.id);
          await api.pauseScheduledJob(row.id);
        } finally {
          const next = new Set(stoppingInFlight);
          next.delete(row.id);
          stoppingInFlight = next;
        }
      } else if (row.status === 'paused') {
        await api.resumeScheduledJob(row.id);
      } else {
        await api.pauseScheduledJob(row.id);
      }
      await loadJobs();
    } catch (e) {
      console.error('[ScheduledPanel] pause/resume failed:', e);
    }
  }

  function requestDelete(e: MouseEvent, row: ScheduledJobRow) {
    e.stopPropagation();
    if (stoppingInFlight.has(row.id)) return;
    // §36.3.7 carry-over (#3050 PR 9): a running row is no longer a flat
    // refusal — the confirm step is now offered for it too; confirmDelete
    // (below) does the wait-then-cancel sequence.
    confirmDeleteId = row.id;
  }

  function cancelDelete(e: MouseEvent) {
    e.stopPropagation();
    confirmDeleteId = null;
  }

  async function confirmDelete(e: MouseEvent, row: ScheduledJobRow) {
    e.stopPropagation();
    try {
      if (row.status === 'running') {
        // §36.3.7 carry-over (#3050 PR 9) — same wait-before-act shape as
        // Pause above: no optimistic row flip while still really running.
        stoppingInFlight = new Set([...stoppingInFlight, row.id]);
        try {
          await api.waitForRunningJobToStop(row.id);
          await api.deleteScheduledJob(row.id);
        } finally {
          const next = new Set(stoppingInFlight);
          next.delete(row.id);
          stoppingInFlight = next;
        }
      } else {
        await api.deleteScheduledJob(row.id);
      }
      confirmDeleteId = null;
      await loadJobs();
    } catch (e) {
      console.error('[ScheduledPanel] deleteScheduledJob failed:', e);
    }
  }

  /** §3151 PR2/C24 — "Clear done": archives the full Done-tab dataset
   *  (shared `archive_terminal_jobs` mechanism, unconditional trigger —
   *  same backend function the tick's auto-archive sweep uses with the
   *  age-gated trigger instead). */
  async function handleClearDone() {
    if (clearingDone || doneAll.length === 0) return;
    clearingDone = true;
    try {
      await api.clearDoneJobs();
      await loadJobs();
    } catch (e) {
      console.error('[ScheduledPanel] clearDoneJobs failed:', e);
    } finally {
      clearingDone = false;
    }
  }

  /** §3151 PR2/C24 — Delete on a Done card: hard delete (job row +
   *  session), NOT the Active tab's soft-cancel `confirmDelete` above —
   *  deliberately separate state/functions so the two guards can never
   *  be conflated. */
  function requestDeleteDone(e: MouseEvent, row: ScheduledJobRow) {
    e.stopPropagation();
    confirmDeleteDoneId = row.id;
  }

  function cancelDeleteDone(e: MouseEvent) {
    e.stopPropagation();
    confirmDeleteDoneId = null;
  }

  async function confirmDeleteDoneRow(e: MouseEvent, row: ScheduledJobRow) {
    e.stopPropagation();
    try {
      await api.deleteArchivedJob(row.id);
      confirmDeleteDoneId = null;
      await loadJobs();
    } catch (e) {
      console.error('[ScheduledPanel] deleteArchivedJob failed:', e);
    }
  }

  function openNewScheduleModal(preset: Preset | null) {
    // #3134 C26: explicit reset, every open — a fresh "New job" must
    // never inherit `editingJobId`/`formRawCron` left over from a prior
    // Edit session (the "not a silent third edit mode" guarantee).
    editingJobId = null;
    formRawCron = null;
    selectedPresetId = preset?.id ?? null;
    formName = preset?.name ?? '';
    formBrief = preset?.brief ?? '';
    formFrequency = preset ? (preset.frequency === 'hourly' ? 'hourly' : preset.frequency) : 'daily';
    formTime = preset?.time ?? '09:00';
    formProjectId = 'workspace';
    formAgentId = agentsList[0]?.id ?? '';
    formError = null;
    showModal = true;
  }

  /** #3134 C26 — the Edit action's own open function, a sibling to
   *  `openNewScheduleModal` above (same form-state variables, same
   *  modal). `kind = 'once'` pre-fills from `row.run_at` (the stable,
   *  user-authored value — `next_run_at` can drift via Run now without
   *  that being an edit the user made). `kind = 'recurring'` checks
   *  `cronToFrequency` ONCE, here, to decide picker vs. raw field
   *  (Ruling 3) — never re-checked while the dialog stays open. */
  function openEditJobModal(row: ScheduledJobRow) {
    editingJobId = row.id;
    selectedPresetId = null;
    formName = row.title;
    formBrief = row.brief;
    formProjectId = row.project_id;
    formAgentId = row.agent_id;
    formError = null;
    if (row.kind === 'once') {
      formFrequency = 'once';
      formOnceDateTime = toDatetimeLocalValue(row.run_at, editDatetimeTz);
      formRawCron = null;
    } else {
      const parsed = row.cron ? cronToFrequency(row.cron) : null;
      if (parsed) {
        formFrequency = parsed.frequency;
        formTime = parsed.time;
        formRawCron = null;
      } else {
        // Ruling 3: not one of buildCron()'s 4 known shapes — show it
        // verbatim in the raw field instead of the picker.
        formRawCron = row.cron ?? '';
      }
    }
    showModal = true;
  }

  function closeModal() {
    showModal = false;
    formError = null;
  }

  function selectPreset(preset: Preset) {
    openNewScheduleModal(preset);
  }

  /** Builds a 5-field cron string from the simplified frequency picker —
   *  the "New schedule" form's "When" row is a deliberate subset of the
   *  design sheet's full picker (daily/weekdays/hourly/weekly only, no
   *  arbitrary custom cron entry) — flagged in the PR report as a scope
   *  simplification for 4d to review live. */
  function buildCron(): string {
    const [hh, mm] = formTime.split(':');
    switch (formFrequency) {
      case 'hourly':
        return `${mm} */4 * * *`;
      case 'weekdays':
        return `${mm} ${hh} * * 1-5`;
      case 'weekly':
        return `${mm} ${hh} * * 1`;
      case 'daily':
      default:
        return `${mm} ${hh} * * *`;
    }
  }

  async function submitNewSchedule() {
    formError = null;
    if (!formBrief.trim()) {
      formError = 'A brief is required.';
      return;
    }
    if (!formAgentId) {
      formError = 'Pick an agent.';
      return;
    }
    formSubmitting = true;
    try {
      // #3134 C26: the one branch point. Edit mode calls
      // `updateScheduledJob(editingJobId, ...)` — rewrites the SAME row.
      // `cron: formRawCron ?? buildCron()` is the fidelity guarantee
      // (Ruling 3): a non-expressible cron the user never touched is
      // passed through byte-for-byte, never re-derived through
      // `buildCron()`.
      if (editingJobId !== null) {
        if (formFrequency === 'once') {
          if (!formOnceDateTime) {
            formError = 'Pick a date and time.';
            formSubmitting = false;
            return;
          }
          await api.updateScheduledJob(editingJobId, {
            brief: formBrief,
            agent: formAgentId,
            projectId: formProjectId,
            title: formName || undefined,
            runAt: formOnceDateTime,
          });
        } else {
          await api.updateScheduledJob(editingJobId, {
            brief: formBrief,
            agent: formAgentId,
            projectId: formProjectId,
            title: formName || undefined,
            cron: formRawCron ?? buildCron(),
          });
        }
        closeModal();
        await loadJobs();
        return;
      }

      const preset = PRESETS.find((p) => p.id === selectedPresetId);
      const kind = preset?.kind;
      if (formFrequency === 'once') {
        if (!formOnceDateTime) {
          formError = 'Pick a date and time.';
          formSubmitting = false;
          return;
        }
        await api.createScheduledJob({
          brief: formBrief,
          agent: formAgentId,
          projectId: formProjectId,
          title: formName || undefined,
          runAt: formOnceDateTime,
        });
      } else {
        await api.createScheduledJob({
          brief: formBrief,
          agent: formAgentId,
          projectId: formProjectId,
          title: formName || undefined,
          cron: buildCron(),
          kind,
        });
      }
      closeModal();
      await loadJobs();
    } catch (e) {
      formError = e instanceof Error ? e.message : String(e);
    } finally {
      formSubmitting = false;
    }
  }
</script>

<aside class="sp-panel" data-testid="scheduled-panel">
  <div class="sp-head">
    <!-- C21 (#3120 PR 3, Matt's ruling): "Scheduled" → "Jobs", "+ New
         schedule" → "+ New job" — the two strings the ruling names.
         Everything else in this panel (the modal's own header, the empty-
         state copy, the "Couldn't load schedules" error text) is
         deliberately untouched — not named by the ruling, left as a
         noted inconsistency rather than silently widened. -->
    <span class="eyb">Jobs</span>
    <span class="n">{jobs.length}</span>
    <span class="sp"></span>
    <button class="btn-o" type="button" onclick={() => openNewScheduleModal(null)}>
      <svg class="ic" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12h14"/><path d="M12 5v14"/></svg>
      New job
    </button>
  </div>

  {#if loading}
    <div class="sp-status">Loading…</div>
  {:else if loadError}
    <div class="sp-status sp-status--error">Couldn't load schedules: {loadError}</div>
  {:else if jobs.length === 0}
    <div class="emp">
      <div class="art">
        <svg class="ic" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></svg>
      </div>
      <h4>No schedules yet</h4>
      <p>Give an agent a recurring job. Each schedule writes its runs into its own session.</p>
    </div>
    <div class="qs">
      {#each PRESETS as preset (preset.id)}
        <button class="q" type="button" onclick={() => selectPreset(preset)} data-testid="preset-card-{preset.id}">
          <span class="qi">
            <svg class="ic" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/></svg>
          </span>
          <span class="q-body">
            <b>{preset.name}</b>
            <span>{preset.subtitle}</span>
          </span>
        </button>
      {/each}
    </div>
  {:else}
    <div class="sp-tabs">
      <span class:on={activeTab === 'active'} onclick={() => (activeTab = 'active')} role="tab" tabindex="0" onkeydown={() => {}}>
        Active<em>{active.length}</em>
      </span>
      <span class:on={activeTab === 'done'} onclick={() => (activeTab = 'done')} role="tab" tabindex="0" onkeydown={() => {}}>
        Done<em>{done.length}</em>
      </span>
    </div>
    {#if activeTab === 'done'}
      <!--
        §3151 PR2/C24 — Done tab's own header row: "Show deleted" toggle
        (reveals cancelled rows, NOT the SessionsDrawer's unrelated
        hidden_at toggle) and "Clear done" (archives the full Done-tab
        dataset — `doneAll`, unaffected by this very toggle).
      -->
      <div class="sp-done-hdr">
        <label class="sp-show-deleted">
          <input type="checkbox" data-testid="show-deleted-toggle" checked={showDeleted} onchange={(e) => (showDeleted = (e.target as HTMLInputElement).checked)} />
          Show deleted
        </label>
        <button
          class="sp-clear-done"
          type="button"
          data-testid="clear-done-btn"
          disabled={clearingDone || doneAll.length === 0}
          onclick={handleClearDone}
        >
          {clearingDone ? 'Clearing…' : 'Clear done'}
        </button>
      </div>
    {/if}
    <div class="sp-list">
      {#if activeTab === 'active'}
        {#each active as row (row.id)}
          {@const state = rowVisualState(row)}
          <div
            class="sc"
            class:k-run={state === 'running'}
            class:k-miss={state === 'missed'}
            class:k-fail={state === 'failed'}
            class:k-pause={state === 'paused'}
            onclick={() => openRow(row)}
            role="button"
            tabindex="0"
            onkeydown={(e) => { if (e.key === 'Enter') openRow(row); }}
            data-testid="scheduled-row"
            data-job-id={row.id}
          >
            <div class="sc-top">
              <span class="sc-t">
                {row.title}
                {#if row.kind === 'once'}<span class="once">Once</span>{/if}
              </span>
              <span class="st" class:st-run={state === 'running'} class:st-miss={state === 'missed'} class:st-fail={state === 'failed'} class:st-pause={state === 'paused'} class:st-on={state === 'active'}>
                {#if state === 'running'}Running
                {:else if state === 'missed'}Missed {row.missed_count}
                {:else if state === 'failed'}Failed
                {:else if state === 'paused'}Paused
                {:else}<i class="g"></i>Active{/if}
              </span>
            </div>
            <div class="sc-m">
              <span class="proj-name">{row.project}</span>
              <span class="dt">·</span>
              <span class="agent-name">{row.agent}</span>
              <span class="acts">
                <button class="ib" class:off={row.status === 'running' || runNowInFlight.has(row.id)} title="Run now" type="button" disabled={row.status === 'running' || runNowInFlight.has(row.id)} onclick={(e) => handleRunNow(e, row)}>
                  <svg class="ic" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M4 14a1 1 0 0 1-.78-1.63l9.9-10.2a.5.5 0 0 1 .86.46l-1.92 6.02A1 1 0 0 0 13 10h7a1 1 0 0 1 .78 1.63l-9.9 10.2a.5.5 0 0 1-.86-.46l1.92-6.02A1 1 0 0 0 11 14z"/></svg>
                </button>
                <!-- #3134 C26: Edit — kind IN ('once','recurring') only
                     (heartbeat/reminder are unscoped, out of this PR),
                     flat-disabled while running (refuse, not a
                     wait-then-edit dance — same shape as the backend's
                     own guard, deliberately NOT the same code as the
                     Delete button's wait-then-act path). -->
                {#if row.kind === 'once' || row.kind === 'recurring'}
                  <button
                    class="ib"
                    class:off={row.status === 'running'}
                    disabled={row.status === 'running'}
                    title="Edit"
                    type="button"
                    data-testid="edit-btn"
                    onclick={(e) => { e.stopPropagation(); openEditJobModal(row); }}
                  >
                    <svg class="ic" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 1 1 3 3L7 19l-4 1 1-4Z"/></svg>
                  </button>
                {/if}
                <button
                  class="ib"
                  class:off={stoppingInFlight.has(row.id)}
                  disabled={stoppingInFlight.has(row.id)}
                  title={stoppingInFlight.has(row.id) ? 'Stopping…' : row.status === 'paused' ? 'Resume' : row.status === 'running' ? 'Stop then pause' : 'Pause'}
                  type="button"
                  data-testid="pause-resume-btn"
                  onclick={(e) => handlePauseResume(e, row)}
                >
                  {#if row.status === 'paused'}
                    <svg class="ic" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><polygon points="6 3 20 12 6 21 6 3"/></svg>
                  {:else}
                    <svg class="ic" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><rect x="14" y="4" width="4" height="16" rx="1"/><rect x="6" y="4" width="4" height="16" rx="1"/></svg>
                  {/if}
                </button>
                {#if confirmDeleteId === row.id}
                  <button class="ib dng" disabled={stoppingInFlight.has(row.id)} title="Confirm delete" type="button" onclick={(e) => confirmDelete(e, row)}>Yes</button>
                  <button class="ib" disabled={stoppingInFlight.has(row.id)} title="Cancel" type="button" onclick={cancelDelete}>No</button>
                {:else}
                  <button
                    class="ib dng"
                    class:off={stoppingInFlight.has(row.id)}
                    disabled={stoppingInFlight.has(row.id)}
                    title={stoppingInFlight.has(row.id) ? 'Stopping…' : row.status === 'running' ? 'Stop then delete' : 'Delete'}
                    type="button"
                    data-testid="delete-btn"
                    onclick={(e) => requestDelete(e, row)}
                  >
                    <svg class="ic" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/></svg>
                  </button>
                {/if}
              </span>
            </div>
            <!--
              C7 (#3120 PR 3, Reed's PR 3 brief): Active rows open on a
              row click same as Done, but had no visible affordance saying
              so — undiscoverable. Adds the same "Open session" link Done
              already shows; no new click handler (the row's own onclick
              above already calls openRow for the whole row — this span is
              purely a visual hint, same "decoration only" shape Done's
              own link already has).
            -->
            <div class="sc-r linked">
              <div><span class="k">Next</span>{formatNextRun(row)}</div>
              <div><span class="k">Last</span>{formatLastRun(row)}</div>
              <span class="lnk">Open session</span>
            </div>
            {#if runNowError.has(row.id)}
              <!-- C31 (#3128, UI half): reuses the modal's own
                   sp-status--error visual language rather than inventing
                   new UI — a transient row-level error instead of the
                   console.error-only swallow this replaces. -->
              <div class="sp-status sp-status--error row-err" data-testid="run-now-error" data-job-id={row.id}>{runNowError.get(row.id)}</div>
            {/if}
          </div>
        {/each}
      {:else}
        {#each done as row (row.id)}
          <div class="sc done" onclick={() => openRow(row)} role="button" tabindex="0" onkeydown={(e) => { if (e.key === 'Enter') openRow(row); }} data-testid="scheduled-row" data-job-id={row.id}>
            <div class="sc-top">
              <span class="sc-t">
                {row.title}
                {#if row.kind === 'once'}<span class="once">Once</span>{/if}
              </span>
              <span class="st" class:st-done={row.status === 'done'} class:st-fail={row.status === 'failed' || row.status === 'stopped'} class:st-del={row.status === 'cancelled'}>
                {#if row.status === 'done'}Completed
                {:else if row.status === 'failed'}Failed
                {:else if row.status === 'stopped'}Stopped
                {:else}Deleted{/if}
              </span>
            </div>
            <!--
              C11 (#3120 PR 2, Matt's ruling) is SUPERSEDED here, for Done
              cards only, by §3151 PR2/C24 (Matt's later ruling, #3151's
              own acceptance text: "delete removes the session"): a
              single Delete action is back, but it is NOT the deleted
              `kind='once' && (done||failed)` branch C11 removed — that
              one called `execute_cancel_job` (soft, status='cancelled').
              This one calls `api.deleteArchivedJob` (HARD delete: job
              row + its own session), offered on EVERY Done row
              regardless of `kind`, via `confirmDeleteDoneId`/
              `requestDeleteDone`/`confirmDeleteDoneRow` — deliberately
              separate state from the Active tab's own
              `confirmDeleteId`/`requestDelete`/`confirmDelete` so the
              two guards (soft-cancel vs. hard-delete) can never be
              conflated by sharing a variable.
            -->
            <div class="sc-m">
              <span class="proj-name">{row.project}</span>
              <span class="dt">·</span>
              <span class="agent-name">{row.agent}</span>
              <span class="acts">
                {#if confirmDeleteDoneId === row.id}
                  <button class="ib dng" title="Confirm delete" type="button" onclick={(e) => confirmDeleteDoneRow(e, row)}>Yes</button>
                  <button class="ib" title="Cancel" type="button" onclick={cancelDeleteDone}>No</button>
                {:else}
                  <button
                    class="ib dng"
                    title="Delete (removes job and session)"
                    type="button"
                    data-testid="delete-btn"
                    onclick={(e) => requestDeleteDone(e, row)}
                  >
                    <svg class="ic" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/></svg>
                  </button>
                {/if}
              </span>
            </div>
            <div class="sc-r one">
              <div><span class="k">Ended</span>{formatLastRun(row)}</div>
              <span class="lnk">Open session</span>
            </div>
          </div>
        {/each}
      {/if}
    </div>
  {/if}
</aside>

{#if showModal}
  <!-- #3175 C57: .modal nested inside .scrim (was a sibling) so .scrim's
       flex-centering actually centers it — mirrors Settings.svelte's
       .modal-overlay/.modal-content nesting. stopPropagation on .modal
       keeps click-inside from bubbling into .scrim's closeModal. -->
  <div class="scrim" role="button" tabindex="-1" aria-label="Close" onclick={closeModal} onkeydown={(e) => { if (e.key === 'Escape') closeModal(); }}>
  <div class="modal" data-testid="new-schedule-modal" onclick={(e) => e.stopPropagation()}>
    <div class="md-h">
      <!-- C21 (#3128, Matt's widened ruling): PR 3's own in-source comment
           above flagged this header as "deliberately untouched — not
           named by the ruling" at the time. #3128 names it explicitly —
           the ruling now widens to cover it. -->
      <h3>{editingJobId ? 'Edit job' : 'New job'}</h3>
      <button class="ib" type="button" onclick={closeModal} aria-label="Close">
        <svg class="ic" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>
      </button>
    </div>
    <div class="md-b">
      {#if !editingJobId}
        <div>
          <div class="eyb">Start from</div>
          <div class="presets">
            {#each PRESETS as preset (preset.id)}
              <button class="pc" class:on={selectedPresetId === preset.id} type="button" onclick={() => selectPreset(preset)} data-testid="modal-preset-{preset.id}">
                <span class="qi"><svg class="ic" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/></svg></span>
                <b>{preset.name}</b>
                <span>{preset.subtitle}</span>
              </button>
            {/each}
          </div>
        </div>
      {/if}
      <div>
        <div class="fl">Name</div>
        <input class="inp" type="text" bind:value={formName} placeholder="Job name" />
      </div>
      <div>
        <div class="fl">Brief<em>prefilled from the preset · edit anything</em></div>
        <textarea class="inp ta" bind:value={formBrief} rows="4"></textarea>
      </div>
      <div class="g2">
        <div>
          <div class="fl">Project</div>
          <!-- #3171 PR 2 (C51/C57): the SAME generic SearchableSelect the
               AI-tab model pickers are backed by (Matt's ruling — "it has
               to be the same component the model pickers use. One fix for
               all three."). `formProjectItems` already excludes folders
               (C56's `!p.parent_id` filter, preserved above); "Home"
               stays pinned regardless of the typed query via
               `formProjectFilterItem`. #3178 (C57, third attempt):
               `sortItems` is `formProjectSortItems`, NOT the plain
               `formItemsUnsorted` the Agent field below still uses — a
               typed match now ranks ahead of an unmatched-but-pinned
               Home instead of Home always sitting first/highlighted.
               #3178 (C57, rewrite, Matt's ruling): `inline` — this field
               lives inside the New job dialog's own scrollable content
               area, not a panel with "room [that has] never failed"
               (Matt, re: the AI-tab model pickers, which stay floating).
               Five passes of fixed-position math for this exact field
               (see SearchableSelect's own `inline` doc comment) are
               replaced, not patched again: the list now renders in flow
               and the dialog's own scroll reaches the last item. -->
          <SearchableSelect
            items={formProjectItems}
            selectedId={formProjectId}
            onSelect={(id) => { formProjectId = id; }}
            getDisplayText={(id) => formItemDisplayText(formProjectItems, id)}
            filterItem={formProjectFilterItem}
            sortItems={formProjectSortItems}
            placeholder="Search projects…"
            testId="project-search"
            inline
          >
            {#snippet renderItem(item)}
              {(item as { id: string; label: string }).label}
            {/snippet}
          </SearchableSelect>
        </div>
        <div>
          <div class="fl">
            Agent
            <!-- #3151 PR1 Commit G: editing a job can never change its
                 agent — the session is permanently bound to it
                 (§36.3.3). Project stays editable (its own sibling
                 field, above, has no such gate); only Agent locks. -->
            {#if editingJobId}<em>Change agent = new job</em>{/if}
          </div>
          <!-- #3171 PR 2 (C51/C57): same shared component as Project,
               above — `disabled` preserves the existing edit-mode lock.
               #3178 (C57, rewrite, Matt's ruling): `inline`, same reason
               as Project's own above it — this field is in the same
               dialog, same scrollable content area. -->
          <SearchableSelect
            items={formAgentItems}
            selectedId={formAgentId}
            onSelect={(id) => { formAgentId = id; }}
            getDisplayText={(id) => formItemDisplayText(formAgentItems, id)}
            filterItem={formAgentFilterItem}
            sortItems={formItemsUnsorted}
            placeholder="Pick an agent"
            testId="agent-search"
            disabled={editingJobId !== null}
            inline
          >
            {#snippet renderItem(item)}
              {(item as { id: string; label: string }).label}
            {/snippet}
          </SearchableSelect>
        </div>
      </div>
      <div>
        <div class="fl">When</div>
        {#if formRawCron !== null}
          <!-- #3134 C26 Ruling 3: a stored cron `cronToFrequency`
               couldn't classify into one of the picker's 4 known
               shapes — shown verbatim instead of the frequency/time
               controls, in the identical form position. Saving without
               touching this round-trips the cron byte-for-byte (never
               passed through `buildCron()`). -->
          <div class="when">
            <input class="inp" type="text" bind:value={formRawCron} data-testid="raw-cron-input" />
          </div>
        {:else}
          <div class="when">
            <select class="inp" bind:value={formFrequency} data-testid="frequency-select">
              <option value="once">Once</option>
              <option value="daily">Daily</option>
              <option value="weekdays">Weekdays</option>
              <option value="hourly">Every 4 hours</option>
              <option value="weekly">Weekly (Monday)</option>
            </select>
            {#if formFrequency === 'once'}
              <input class="inp" type="datetime-local" bind:value={formOnceDateTime} />
            {:else}
              <span class="at">at</span>
              <input class="inp tm" type="time" bind:value={formTime} />
            {/if}
          </div>
        {/if}
      </div>
      {#if formError}
        <div class="sp-status sp-status--error">{formError}</div>
      {/if}
    </div>
    <div class="md-f">
      <!-- #3134 C26: "Runs go to a NEW session" is false for an edit —
           saving reuses the job's existing session/history, the whole
           point of Edit over cancel-and-recreate.

           #3171 PR 2 (C43): Reed's correction to the issue text — this
           footer ALREADY tracked `formName` live (a `$state` referenced
           directly in markup is reactive by construction, no snapshot/
           staleness bug to fix). The actual defect was the COPY: the old
           `formName || 'this job'` substitution put the literal filler
           word "this job" inside the quotes when the field was empty,
           reading as a false claim ("...called "this job"." — the
           session will NOT literally be named that; it's auto-titled
           from the brief/schedule). Fixed by restructuring the empty
           branch into its own sentence — no quotes, no fake name —
           instead of a filler-word substitution into one template
           string. Same restructuring applied to the Edit-mode "Saves
           to" line for consistency (its own empty branch is dead code
           today — `formName` is always pre-filled from the existing job
           when editing — but shouldn't be left as the one remaining
           instance of the old pattern). -->
      {#if editingJobId}
        {#if formName}
          <span class="sum">Saves to "{formName}"'s existing session.</span>
        {:else}
          <span class="sum">Saves to the job's existing session, named after it.</span>
        {/if}
      {:else if formName}
        <span class="sum">Runs go to a new session called "{formName}".</span>
      {:else}
        <span class="sum">Runs go to a new session, named after the job.</span>
      {/if}
      <button class="btn-g" type="button" onclick={closeModal}>Cancel</button>
      <button class="btn-p" type="button" disabled={formSubmitting} onclick={submitNewSchedule}>{editingJobId ? 'Save changes' : 'Create job'}</button>
    </div>
  </div>
  </div>
{/if}

<style>
  /* §36.3.8 (#3050 PR 8) — the ONE mechanism for item 1's "always
     docked, no toggle, no overlay": fixed width, flex-shrink: 0. The
     sibling `<ChatSidebar layoutMode="centered" />` in +page.svelte is
     flex:1 and absorbs every width change (left-nav rail expand/collapse,
     Sessions drawer open/close) — no JS here participates in that squish
     at all, same as `SessionsDrawer.svelte`'s own `.sd-panel` precedent. */
  .sp-panel {
    width: 352px;
    flex-shrink: 0;
    border-right: 1px solid var(--border-faint);
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--surface-0, #0c0c11);
    overflow: hidden;
  }

  .sp-head {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 14px 12px 18px;
    flex-shrink: 0;
  }
  .eyb {
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--fg-3);
  }
  .sp-head .n { font-size: 11.5px; color: var(--fg-4); font-variant-numeric: tabular-nums; }
  .sp { flex: 1; }

  .btn-o {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 11px;
    border-radius: 7px;
    border: 1px solid rgba(0, 228, 228, 0.55);
    color: var(--accent);
    font-size: 12.5px;
    font-weight: 500;
    background: transparent;
    white-space: nowrap;
    cursor: pointer;
  }
  .btn-o .ic { width: 13px; height: 13px; }

  .sp-status {
    padding: 14px 18px;
    font-size: 12.5px;
    color: var(--fg-3);
  }
  .sp-status--error { color: var(--status-danger); }
  .row-err { margin-top: 6px; font-size: 11.5px; }

  .sp-tabs {
    display: flex;
    gap: 4px;
    padding: 0 18px;
    border-bottom: 1px solid var(--border-faint);
    margin-bottom: 10px;
    flex-shrink: 0;
  }
  .sp-tabs span {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    padding: 8px 4px;
    margin-right: 14px;
    font-size: 13px;
    font-weight: 500;
    color: var(--fg-3);
    border-bottom: 2px solid transparent;
    margin-bottom: -1px;
    cursor: pointer;
  }
  .sp-tabs span.on { color: var(--fg-1); border-bottom-color: var(--accent); }
  .sp-tabs em { font-style: normal; font-size: 11px; color: var(--fg-4); font-variant-numeric: tabular-nums; }

  /* §3151 PR2/C24 — Done tab's own header row (toggle + Clear done). */
  .sp-done-hdr {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 0 18px 8px;
    flex-shrink: 0;
  }
  .sp-show-deleted {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--fg-3);
    cursor: pointer;
  }
  .sp-clear-done {
    font-size: 12px;
    color: var(--fg-3);
    background: var(--surface-2);
    border: 1px solid var(--border-faint);
    border-radius: 6px;
    padding: 4px 10px;
    cursor: pointer;
  }
  .sp-clear-done:hover:not(:disabled) { background: var(--surface-3); color: var(--fg-1); }
  .sp-clear-done:disabled { opacity: 0.5; cursor: default; }

  .sp-list {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 0 12px 12px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  /* §36.3.8/Notes: "Status stripe reuses the Kanban left edge: teal
     running, amber missed, red failed, none when healthy. Paused rows
     dim instead." */
  .sc {
    position: relative;
    background: var(--surface-1);
    border: 1px solid var(--border-faint);
    border-radius: 8px;
    padding: 8px 10px 8px 13px;
    display: flex;
    flex-direction: column;
    gap: 5px;
    cursor: pointer;
  }
  .sc:hover { background: var(--surface-2); border-color: var(--border); }
  .sc.k-run { border-left: 3px solid var(--accent); }
  .sc.k-miss { border-left: 3px solid var(--status-warning); }
  .sc.k-fail { border-left: 3px solid var(--status-danger); }
  .sc.k-pause { opacity: 0.6; }

  .sc-top { display: flex; align-items: center; gap: 8px; min-height: 22px; }
  .sc-t {
    flex: 1;
    min-width: 0;
    font-size: 13.5px;
    font-weight: 600;
    color: var(--fg-1);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .once {
    font-size: 9.5px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--fg-3);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 1px 5px;
    margin-left: 6px;
    vertical-align: 1px;
  }

  .st {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.05em;
    text-transform: uppercase;
    padding: 3px 7px;
    border-radius: 6px;
    white-space: nowrap;
  }
  .st-run { background: var(--accent-soft); color: var(--accent); }
  .st-on { background: var(--surface-2); color: var(--fg-3); border: 1px solid var(--border-faint); }
  .st-on i.g { width: 6px; height: 6px; border-radius: 50%; background: var(--status-success); display: inline-block; }
  .st-pause { background: var(--surface-2); color: var(--fg-3); border: 1px solid var(--border-faint); }
  .st-miss { background: rgba(245, 158, 11, 0.14); color: var(--status-warning); }
  .st-fail { background: rgba(239, 68, 68, 0.14); color: var(--status-danger); }
  .st-done { background: rgba(34, 197, 94, 0.12); color: var(--status-success); }
  .st-del { background: var(--surface-2); color: var(--fg-4); border: 1px solid var(--border-faint); }

  .sc-m {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--fg-3);
    white-space: nowrap;
    overflow: visible;
  }
  .sc-m .dt { color: var(--fg-4); opacity: 0.6; }
  .proj-name, .agent-name { overflow: hidden; text-overflow: ellipsis; }

  .acts { display: flex; gap: 2px; margin-left: auto; padding-left: 8px; }
  .acts .ib {
    width: 24px;
    height: 24px;
    border-radius: 6px;
    background: transparent;
    border: 1px solid var(--border-faint);
    color: var(--fg-3);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
  }
  .acts .ib:hover { background: var(--surface-3); color: var(--fg-1); }
  .acts .ib.off { opacity: 0.35; cursor: default; }
  .acts .ib.dng:hover { color: var(--status-danger); }
  .acts .ib .ic { width: 12px; height: 12px; }

  .sc-r {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
    font-size: 11.5px;
    color: var(--fg-2);
    font-variant-numeric: tabular-nums;
    padding-top: 6px;
    border-top: 1px solid var(--border-faint);
  }
  .sc-r.one { grid-template-columns: 1fr auto; align-items: end; }
  /* C7 (#3120 PR 3): Active rows get a third column for the "Open
     session" link, same end-aligned treatment as .one's single link. */
  .sc-r.linked { grid-template-columns: 1fr 1fr auto; align-items: end; }
  .sc-r .k {
    display: block;
    font-size: 9.5px;
    font-weight: 600;
    letter-spacing: 0.07em;
    text-transform: uppercase;
    color: var(--fg-4);
    margin-bottom: 2px;
  }
  .lnk { font-size: 12px; font-weight: 500; color: var(--accent); white-space: nowrap; }

  /* empty state + preset quick-cards */
  .emp { display: flex; flex-direction: column; align-items: center; text-align: center; padding: 32px 18px 14px; flex-shrink: 0; }
  .emp .art {
    width: 56px;
    height: 56px;
    border-radius: 50%;
    background: radial-gradient(circle at 50% 42%, rgba(0, 228, 228, 0.12), transparent 68%);
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--fg-4);
    margin-bottom: 12px;
  }
  .emp .art .ic { width: 22px; height: 22px; }
  .emp h4 { margin: 0 0 6px; font-size: 14px; font-weight: 600; color: var(--fg-1); }
  .emp p { margin: 0; font-size: 12.5px; line-height: 1.5; color: var(--fg-3); max-width: 260px; }

  .qs { display: flex; flex-direction: column; gap: 8px; padding: 6px 12px; overflow-y: auto; }
  .q {
    display: flex;
    align-items: center;
    gap: 12px;
    background: var(--surface-1);
    border: 1px solid var(--border-faint);
    border-radius: 8px;
    padding: 11px 12px;
    cursor: pointer;
    text-align: left;
    width: 100%;
  }
  .q:hover { border-color: rgba(0, 228, 228, 0.4); background: var(--surface-2); }
  .q .qi {
    width: 30px;
    height: 30px;
    border-radius: 7px;
    background: var(--accent-soft);
    color: var(--accent);
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }
  .q-body { display: flex; flex-direction: column; }
  .q b { font-size: 13px; font-weight: 600; color: var(--fg-1); }
  .q span { font-size: 11.5px; color: var(--fg-3); margin-top: 1px; }

  /* modal */
  /* #3175 C57: flex-centering instead of transform: translate(-50%, -50%) —
     a transformed ancestor becomes the containing block for position:fixed
     descendants, which broke SearchableSelect's viewport-relative dropdown
     positioning for the Project/Agent pickers. Mirrors Settings.svelte's
     .modal-overlay/.modal-content pattern, which never had this problem. */
  .scrim {
    position: fixed;
    inset: 0;
    background: rgba(5, 5, 7, 0.62);
    z-index: 50;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .modal {
    position: relative;
    z-index: 51;
    width: 640px;
    max-height: 86vh;
    overflow-y: auto;
    background: var(--surface-1);
    border: 1px solid var(--border-strong);
    border-radius: 16px;
    box-shadow: var(--shadow-pop);
    display: flex;
    flex-direction: column;
  }
  .md-h {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 18px 20px 14px 24px;
    border-bottom: 1px solid var(--border-faint);
  }
  .md-h h3 { font-size: 16px; margin: 0; flex: 1; color: var(--fg-1); }
  .md-h .ib { background: transparent; border: none; color: var(--fg-3); cursor: pointer; }
  .md-b { padding: 18px 24px 6px; display: flex; flex-direction: column; gap: 16px; }
  .md-f {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 14px 20px 16px 24px;
    border-top: 1px solid var(--border-faint);
    margin-top: 8px;
  }
  .md-f .sum { flex: 1; font-size: 12px; color: var(--fg-3); }

  .presets { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 10px; margin-top: 8px; }
  .pc {
    background: var(--surface-2);
    border: 1px solid var(--border-faint);
    border-radius: 10px;
    padding: 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-height: 92px;
    cursor: pointer;
    text-align: left;
  }
  .pc .qi { width: 26px; height: 26px; border-radius: 7px; background: var(--surface-3); color: var(--fg-3); display: flex; align-items: center; justify-content: center; }
  .pc b { font-size: 13px; color: var(--fg-1); font-weight: 600; }
  .pc span { font-size: 11px; color: var(--fg-3); line-height: 1.4; }
  .pc.on { border-color: var(--accent); background: rgba(0, 228, 228, 0.06); box-shadow: 0 0 0 3px var(--accent-soft); }
  .pc.on .qi { background: var(--accent-soft); color: var(--accent); }

  .fl { font-size: 12.5px; font-weight: 500; color: var(--fg-1); margin-bottom: 6px; display: flex; align-items: baseline; gap: 8px; }
  .fl em { font-style: normal; font-weight: 400; font-size: 11.5px; color: var(--fg-4); }

  .inp {
    width: 100%;
    background: var(--surface-0, #0e0e12);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 9px 12px;
    font-size: 13.5px;
    line-height: 1.4;
    color: var(--fg-1);
    font-family: inherit;
    box-sizing: border-box;
  }
  /* C12 (#3120 PR 3, dropdown height half): the 4 <select>s (Project,
     Agent, Frequency, Time) relied on the browser's own default
     line-height at 13.5px, which rendered noticeably cramped/short on
     some platforms. An explicit min-height (comfortably above the glyph
     box at this font-size/padding) fixes every <select> and <input> this
     class touches in one place; the textarea variant below keeps its own
     taller rows-driven height unaffected. */
  .inp:not(.ta) { min-height: 38px; }
  .inp.ta { line-height: 1.55; resize: vertical; }
  /* C12 (#3128): root cause of the still-cramped dropdowns — the 3
     <select class="inp">s (Project, Agent, Frequency) had no
     appearance: none, so WKWebView's native select chrome ignored
     min-height above for the select's own box. Every other styled
     <select> in this codebase already sets this before any
     padding/sizing (Settings.svelte's .select-input, Editor.svelte,
     ChatPanel.svelte) — §8a sibling-parity check (git log -S'appearance'
     on this file) confirmed this was a plain omission, never a
     deliberate removal. Same custom-arrow pattern as Settings.svelte's
     .select-input, sized for this panel's existing 9px/12px padding. */
  select.inp {
    appearance: none;
    -webkit-appearance: none;
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='10' height='6'%3E%3Cpath d='M0 0l5 6 5-6z' fill='rgba(255,255,255,0.4)'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: right 12px center;
    padding-right: 30px;
    cursor: pointer;
  }
  .g2 { display: grid; grid-template-columns: 1fr 1fr; gap: 14px; }
  .when { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; }
  .when .inp { width: auto; }
  /* C29 (#3128, dark-calendar half): the Once picker's native
     datetime-local/time popups rendered in the browser's LIGHT theme by
     default, clashing with this panel's dark UI — §8a sibling-parity
     check (git log -S'color-scheme' on this file) confirmed this was a
     plain omission, same pattern as Editor.svelte's .pill-date-picker and
     TaskMetadataHeader.svelte's input[type="date"]. The "seconds"/time-
     field half of C29 is explicitly NOT this — see Matt's correction, a
     separate backend commit fixing parse_run_at_local. */
  .when input[type="datetime-local"],
  .when input[type="time"] {
    color-scheme: dark;
  }
  .at { font-size: 12.5px; color: var(--fg-3); }

  .btn-p {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 34px;
    padding: 0 15px;
    border-radius: 8px;
    background: var(--accent);
    color: var(--fg-on-accent);
    font-size: 13px;
    font-weight: 600;
    border: 0;
    white-space: nowrap;
    cursor: pointer;
  }
  .btn-p:disabled { opacity: 0.5; cursor: default; }
  .btn-g {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 34px;
    padding: 0 14px;
    border-radius: 8px;
    background: transparent;
    color: var(--fg-3);
    font-size: 13px;
    font-weight: 500;
    border: 1px solid var(--border);
    white-space: nowrap;
    cursor: pointer;
  }
</style>

/**
 * #3050 PR 8 — Settings → AI, "Concurrent jobs" row.
 *
 * §36.2.2: `ai_settings.job_concurrency` already exists and is already
 * read by `scheduler_tick` — this PR adds ONLY the Settings UI row.
 * Unlike the ceiling/checkpoint pair (`job_ceiling_tool_calls` >=
 * `max_tool_iterations`, PR 3's own clamp), `job_concurrency` has no
 * cross-field relationship — a plain bounded numeric value (min 1: a 0
 * would silently stall the entire scheduler).
 *
 * Same "faithful extraction, not a re-implementation" test-double
 * pattern `Settings.ctx-window-override.test.ts` already established for
 * this file (Settings.svelte requires a full Tauri window context and is
 * ~5700 lines — mounting it is impractical for this one row's logic).
 * The extraction below mirrors `handleAbBlur`'s real body verbatim for
 * the `job_concurrency`/`job_ceiling_tool_calls`/`max_tool_iterations`
 * branches it exercises.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';

function makeAbBehaviorHarness(invokeImpl: (cmd: string, args: any) => Promise<any>) {
  let abMaxIter = 30;
  let abJobCeiling = 200;
  let abJobConcurrency = 2;
  let abJobConcurrencySaved: number | null = null;

  async function handleAbBlur(
    key: 'max_tool_iterations' | 'loop_timeout_seconds' | 'job_ceiling_tool_calls' | 'job_concurrency',
    rawValue: number | string,
    min: number,
    max: number,
    defaultVal: number
  ) {
    let num = typeof rawValue === 'string' ? parseInt(rawValue, 10) : rawValue;
    if (isNaN(num) || rawValue === '') {
      num = key === 'job_concurrency' ? (abJobConcurrencySaved ?? defaultVal) : defaultVal;
    }
    num = Math.max(min, Math.min(max, num));

    // §36.2.2 Ruling 4 — ceiling >= checkpoint, clamped both directions.
    // job_concurrency has NO cross-field clamp.
    let ceilingToSave: number | null = null;
    if (key === 'job_ceiling_tool_calls' && num < abMaxIter) {
      num = abMaxIter;
    } else if (key === 'max_tool_iterations' && num > abJobCeiling) {
      ceilingToSave = num;
    }

    if (key === 'max_tool_iterations') {
      abMaxIter = num;
    } else if (key === 'job_concurrency') {
      abJobConcurrency = num;
    } else {
      abJobCeiling = num;
    }
    if (ceilingToSave !== null) {
      abJobCeiling = ceilingToSave;
    }

    await invokeImpl('set_ai_setting', { key, value: String(num) });
    if (ceilingToSave !== null) {
      await invokeImpl('set_ai_setting', { key: 'job_ceiling_tool_calls', value: String(ceilingToSave) });
    }
    if (key === 'job_concurrency') {
      abJobConcurrencySaved = num;
    }
  }

  return {
    handleAbBlur,
    get abMaxIter() { return abMaxIter; },
    set abMaxIter(v: number) { abMaxIter = v; },
    get abJobCeiling() { return abJobCeiling; },
    set abJobCeiling(v: number) { abJobCeiling = v; },
    get abJobConcurrency() { return abJobConcurrency; },
  };
}

describe('#3050 PR 8 — Settings "Concurrent jobs" round-trips to ai_settings', () => {
  let savedCalls: { key: string; value: string }[] = [];
  let invokeMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    savedCalls = [];
    invokeMock = vi.fn(async (cmd: string, args: any) => {
      if (cmd === 'set_ai_setting') {
        savedCalls.push({ key: args.key, value: args.value });
      }
      return undefined;
    });
  });

  it('saves job_concurrency via set_ai_setting with the entered value', async () => {
    const h = makeAbBehaviorHarness(invokeMock);
    await h.handleAbBlur('job_concurrency', 5, 1, 20, 2);
    expect(savedCalls).toEqual([{ key: 'job_concurrency', value: '5' }]);
    expect(h.abJobConcurrency).toBe(5);
  });

  it('clamps to the minimum (1) — a 0 would stall the entire scheduler', async () => {
    const h = makeAbBehaviorHarness(invokeMock);
    await h.handleAbBlur('job_concurrency', 0, 1, 20, 2);
    expect(h.abJobConcurrency).toBe(1);
    expect(savedCalls).toEqual([{ key: 'job_concurrency', value: '1' }]);
  });

  it('clamps to the maximum (20)', async () => {
    const h = makeAbBehaviorHarness(invokeMock);
    await h.handleAbBlur('job_concurrency', 999, 1, 20, 2);
    expect(h.abJobConcurrency).toBe(20);
  });

  it('reverts to the last-saved value on an empty/NaN blur', async () => {
    const h = makeAbBehaviorHarness(invokeMock);
    await h.handleAbBlur('job_concurrency', 7, 1, 20, 2);
    await h.handleAbBlur('job_concurrency', '', 1, 20, 2);
    expect(h.abJobConcurrency).toBe(7);
  });

  /** Mutation guard: if `set_ai_setting` were never called (the save
   *  call swapped for a no-op), this test's own round-trip assertion
   *  catches it — proving the test actually exercises the save path,
   *  not just in-memory state. */
  it('mutation guard: the save call must actually reach set_ai_setting (not a stale in-memory-only value)', async () => {
    const h = makeAbBehaviorHarness(invokeMock);
    await h.handleAbBlur('job_concurrency', 4, 1, 20, 2);
    expect(invokeMock).toHaveBeenCalledWith('set_ai_setting', { key: 'job_concurrency', value: '4' });
  });
});

describe("#3050 PR 8 — adding 'Concurrent jobs' does NOT perturb PR 3's existing ceiling >= checkpoint clamp", () => {
  let invokeMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    invokeMock = vi.fn(async () => undefined);
  });

  it('job_ceiling_tool_calls is still clamped up to max_tool_iterations when saved lower', async () => {
    const h = makeAbBehaviorHarness(invokeMock);
    h.abMaxIter = 30;
    await h.handleAbBlur('job_ceiling_tool_calls', 10, h.abMaxIter, 5000, 200);
    expect(h.abJobCeiling).toBe(30);
  });

  it('max_tool_iterations raised above the current (lower) ceiling also raises the ceiling to match', async () => {
    const h = makeAbBehaviorHarness(invokeMock);
    h.abJobCeiling = 10; // below checkpoint's own [1,30] range, so this is reachable
    await h.handleAbBlur('max_tool_iterations', 25, 1, 30, 30);
    expect(h.abJobCeiling).toBe(25);
  });

  it('a job_concurrency save does not touch abJobCeiling/abMaxIter at all — the regression check this PR must pass, not a new clamp target', async () => {
    const h = makeAbBehaviorHarness(invokeMock);
    h.abJobCeiling = 200;
    h.abMaxIter = 30;
    await h.handleAbBlur('job_concurrency', 8, 1, 20, 2);
    expect(h.abJobCeiling).toBe(200);
    expect(h.abMaxIter).toBe(30);
  });
});

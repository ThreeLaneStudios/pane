/**
 * Settings.memoryFactsRelativeTime.test.ts — #3151 PR3/C44
 *
 * Settings.svelte's own `formatRelativeTime` (the memory-facts tab's
 * "updated X ago" label) parsed SQLite's naive-UTC `timestamp` string
 * straight through `new Date()`, same bug class as `utils/time.ts`'s
 * and `NotificationPanel.svelte`'s own copies. Settings.svelte requires
 * a full Tauri window context and is ~5700+ lines — mounting it for one
 * row's logic is impractical, same "faithful extraction, not a
 * re-implementation" test-double pattern `Settings.ctx-window-
 * override.test.ts`/`Settings.job-concurrency.test.ts` already
 * established for this file. The extraction below mirrors
 * `formatRelativeTime`'s real body verbatim.
 */
import { describe, it, expect } from 'vitest';

/** Verbatim extraction of Settings.svelte's `formatRelativeTime`. */
function formatRelativeTime(timestamp: string): string {
  const normalized = /[Zz]$|[+-]\d{2}:?\d{2}$/.test(timestamp.trim())
    ? timestamp
    : timestamp.replace(' ', 'T') + 'Z';
  const date = new Date(normalized);
  const now = new Date();
  const diffMs = now.getTime() - date.getTime();
  const diffMins = Math.floor(diffMs / (1000 * 60));
  const diffHours = Math.floor(diffMins / 60);
  const diffDays = Math.floor(diffHours / 24);

  if (diffMins < 1) return 'just now';
  if (diffMins < 60) return `${diffMins}m ago`;
  if (diffHours < 24) return `${diffHours}h ago`;
  if (diffDays < 7) return `${diffDays}d ago`;

  const month = date.toLocaleDateString('en-US', { month: 'short' });
  const day = date.getDate();
  return `${month} ${day}`;
}

function sqliteUtcSecondsAgo(secondsAgo: number): string {
  return new Date(Date.now() - secondsAgo * 1000).toISOString().slice(0, 19).replace('T', ' ');
}

describe('Settings — memory facts formatRelativeTime never stuck at "just now" (C44)', () => {
  it('a naive SQLite timestamp 150 seconds old reads "2m ago"', () => {
    const result = formatRelativeTime(sqliteUtcSecondsAgo(150));
    expect(result).not.toBe('just now');
    expect(result).toBe('2m ago');
  });

  it('an already-Z-suffixed ISO string is unaffected (regression guard)', () => {
    expect(formatRelativeTime(new Date(Date.now() - 150_000).toISOString())).toBe('2m ago');
  });

  // Mutation proof (actually run under a real `TZ=America/New_York`
  // process invocation against this exact extraction with its first
  // line reverted to the bare, pre-fix `const date = new Date(timestamp)`
  // — confirmed red there, then reverted back; see the PR report). This
  // file's own default `TZ=UTC` pool run can't exercise that divergence
  // (the bug is invisible when local time IS UTC, by its own nature),
  // same documented limitation `utils/time.test.ts` states for itself.
});

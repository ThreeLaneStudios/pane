/**
 * #3151 PR3/C44: SQLite's `datetime('now')` shape ("YYYY-MM-DD HH:MM:SS",
 * no offset, implicitly UTC) is NOT ISO-8601 — `new Date()` on it parses
 * as LOCAL wall-clock time, not UTC. In any timezone behind UTC that
 * makes `Date.now() - parsed` negative, so every caller downstream that
 * checks `diffSec < 60` sees it satisfied unconditionally: every
 * timestamp shows as "Just now" forever, regardless of real age. Same
 * fix pattern already used elsewhere in this codebase
 * (`ChatPanel.svelte`, `scheduledPanel.ts`, `jobStatusChip.ts`): append
 * `Z` (after swapping the space for `T`) so `Date` treats it as UTC. A
 * timestamp already carrying an explicit offset/`Z` (an ISO string, or a
 * bare epoch number) is passed through unchanged — this only normalizes
 * the bare SQLite shape.
 */
export function parseStoredTimestamp(timestamp: string | number): Date {
  if (typeof timestamp === 'number') return new Date(timestamp);
  const hasOffset = /[Zz]$|[+-]\d{2}:?\d{2}$/.test(timestamp.trim());
  return hasOffset ? new Date(timestamp) : new Date(timestamp.replace(' ', 'T') + 'Z');
}

/**
 * Format a timestamp as relative time (e.g., "2 hours ago", "yesterday")
 */
export function formatRelativeTime(timestamp: string | number | null | undefined): string {
  if (!timestamp) return 'Never';
  
  const date = parseStoredTimestamp(timestamp);
  const now = new Date();
  const diffMs = now.getTime() - date.getTime();
  const diffSec = Math.floor(diffMs / 1000);
  const diffMin = Math.floor(diffSec / 60);
  const diffHour = Math.floor(diffMin / 60);
  const diffDay = Math.floor(diffHour / 24);
  const diffWeek = Math.floor(diffDay / 7);
  const diffMonth = Math.floor(diffDay / 30);
  const diffYear = Math.floor(diffDay / 365);

  if (diffSec < 60) return 'Just now';
  if (diffMin < 60) return `${diffMin} ${diffMin === 1 ? 'minute' : 'minutes'} ago`;
  if (diffHour < 24) return `${diffHour} ${diffHour === 1 ? 'hour' : 'hours'} ago`;
  if (diffDay === 1) return 'Yesterday';
  if (diffDay < 7) return `${diffDay} days ago`;
  if (diffWeek < 4) return `${diffWeek} ${diffWeek === 1 ? 'week' : 'weeks'} ago`;
  if (diffMonth < 12) return `${diffMonth} ${diffMonth === 1 ? 'month' : 'months'} ago`;
  return `${diffYear} ${diffYear === 1 ? 'year' : 'years'} ago`;
}

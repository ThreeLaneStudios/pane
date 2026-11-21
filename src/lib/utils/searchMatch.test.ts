/**
 * searchMatch.test.ts — #3166 C51/C57.
 *
 * `matchesSearchQuery` is the one shared substring-match predicate now
 * wired into three previously-unsearchable fields: the image-model
 * picker (Settings.svelte), and the New job dialog's Project/Agent
 * fields (ScheduledPanel.svelte) — plus `NewSessionSheet.svelte`'s own
 * `matchesSearch`, which this was extracted FROM.
 */
import { describe, it, expect } from 'vitest';
import { matchesSearchQuery } from './searchMatch';

describe('#3166 C51/C57 — matchesSearchQuery', () => {
  it('empty query matches everything', () => {
    expect(matchesSearchQuery('Aurum', '')).toBe(true);
    expect(matchesSearchQuery('', '')).toBe(true);
  });

  it('whitespace-only query matches everything, same as empty', () => {
    expect(matchesSearchQuery('Aurum', '   ')).toBe(true);
  });

  it('case-insensitive substring match', () => {
    expect(matchesSearchQuery('Aurum', 'aur')).toBe(true);
    expect(matchesSearchQuery('Aurum', 'AUR')).toBe(true);
    expect(matchesSearchQuery('aurum', 'Aurum')).toBe(true);
  });

  it('a query that matches nothing returns false', () => {
    expect(matchesSearchQuery('Aurum', 'zzz')).toBe(false);
  });

  it('leading/trailing whitespace on the query is trimmed before matching', () => {
    expect(matchesSearchQuery('Aurum', '  aur  ')).toBe(true);
  });

  /// Mutation guard: a query that's a substring of nothing in the name
  /// must stay false — proves the match isn't accidentally permissive
  /// (e.g. always returning true).
  it('a non-matching query against a real name stays false, not a false positive', () => {
    expect(matchesSearchQuery('Valencia', 'aurum')).toBe(false);
  });
});

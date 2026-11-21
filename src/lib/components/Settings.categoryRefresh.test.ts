/**
 * Settings.categoryRefresh.test.ts — #2972 Stage 4 (`docs/spec/
 * data-refresh.md` §34, inventory + cross-screen verification pass).
 *
 * Finding (inventory, §34 Stage 4): `Settings.svelte`'s CATEGORIES tab
 * shows `definedCategories` (loaded via `loadCategories()`), and its own
 * `addCategory()`/`updateCategory()`/`removeCategoryDef()` handlers each
 * already call `loadCategories()` again after their own mutation — but
 * before this fix, nothing re-read the list when a category changed
 * ELSEWHERE (e.g. an agent tool call, or Sidebar's own category edit)
 * while Settings was open. Not one of §34.12's nine screens; never
 * migrated by any prior #2972 stage.
 *
 * Harness: source-level assertions, not DOM `render()` — matching the
 * established pattern for this file (`Settings.test.ts`'s own doc
 * comment: "Settings.svelte requires the full Tauri window context and
 * is ~4400+ lines. Source-level assertions are used instead."). No other
 * test in this directory renders `Settings.svelte` in jsdom.
 *
 * Measured directly against pre-fix `Settings.svelte` (git-stashed for
 * this proof, see the sibling fix's PR description for the exact
 * before/after `pnpm vitest run` output): the file contained no
 * `subscribeDataChanged` import and no `ENTITY_CATEGORY` reference
 * anywhere — these assertions fail against that source. After the fix,
 * both exist and are wired together in one handler.
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = readFileSync(resolve(__dirname, 'Settings.svelte'), 'utf-8');

describe('Settings.svelte — category pane-data-changed listener (#2972 stage 4)', () => {
  it('imports subscribeDataChanged and ENTITY_CATEGORY from $lib/refreshEvent', () => {
    expect(source).toMatch(/import\s*\{[^}]*subscribeDataChanged[^}]*\}\s*from\s*'\$lib\/refreshEvent'/);
    expect(source).toMatch(/import\s*\{[^}]*ENTITY_CATEGORY[^}]*\}\s*from\s*'\$lib\/refreshEvent'/);
  });

  it('calls subscribeDataChanged and checks entity_type against ENTITY_CATEGORY', () => {
    expect(source).toMatch(/subscribeDataChanged\(\s*\(payload\)\s*=>/);
    expect(source).toMatch(/payload\.entity_type === ENTITY_CATEGORY/);
  });

  it('the category branch calls loadCategories(), not a broader reload', () => {
    // Extract the $effect block registering the listener and confirm its
    // body calls loadCategories() — not api.listProjects()/a full page
    // reload — so this is a scoped patch, matching §34.5's discipline.
    const match = source.match(/subscribeDataChanged\(\s*\(payload\)\s*=>\s*\{([\s\S]{0,200}?)\}\s*\);/);
    expect(match, 'subscribeDataChanged(...) call site must exist').toBeTruthy();
    expect(match![1]).toMatch(/loadCategories\(\)/);
  });

  it('unregisters the listener on effect teardown (no leaked subscription)', () => {
    expect(source).toMatch(/unlistenDataChanged\.then\(\(fn\)\s*=>\s*fn\(\)\)/);
  });
});

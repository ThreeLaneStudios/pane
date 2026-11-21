import { expect, type Page } from '@playwright/test';
import { getTauriMockScript } from './tauri-mock';

/**
 * #3182 (C64 narrow-window restore, inline list height cap) — the
 * "Vite + Playwright WebKit harness with mocked IPC" Reed's audit
 * (docs/audits/2026-11-05-scope-3178-c57-third.md §0) ran ad hoc and
 * `trash`'d; this file is that harness's first committed form. Plain
 * `vite dev`, zero Rust/DB/filesystem — `window.__TAURI_INTERNALS__`
 * is entirely mocked, same mechanism `fixtures.ts`/`tauri-mock.ts`
 * already use for the note-CRUD specs in this directory. Extends the
 * existing mock (doesn't fork it) with the three commands the Home
 * view's app shell and ScheduledPanel's "New job" dialog need that the
 * original mock (written for FolderView/notes) never had to answer:
 * `should_show_onboarding`, `get_agents`, `list_scheduled_jobs`.
 *
 * Never launches the real app binary (#3179) — this is `vite dev`
 * serving the SvelteKit app directly to Playwright's browser, no Tauri
 * process involved at all.
 */
export async function setupHomeView(
  page: Page,
  opts: { agentCount?: number; projects?: unknown[] } = {}
) {
  const agentCount = opts.agentCount ?? 8; // > 5 rows on purpose — the cap's whole point
  // #3190 (C65 final): `list_projects` already has a base-mock handler
  // (returns `mockData.projects`, always `[]`) — opt-in override here so
  // specs that need real favorited/categorized project rows (e.g. the
  // sidebar's grouped rail/label view) don't have to fork this fixture.
  const projectsJson = JSON.stringify(opts.projects ?? null);
  const mockScript = getTauriMockScript();
  const seedScript = `
    ${mockScript}
    (function() {
      const originalInvoke = window.__TAURI_INTERNALS__.invoke;
      const AGENTS = Array.from({ length: ${agentCount} }, (_, i) => ({
        id: 'agent-' + i,
        name: 'Agent ' + (i + 1),
        source: 'local',
        provider: 'mock',
        model: 'mock-model',
      }));
      const PROJECTS_OVERRIDE = ${projectsJson};
      window.__TAURI_INTERNALS__.invoke = async (cmd, args) => {
        if (cmd === 'get_startup_status') return { phase: 'ready', message: null };
        if (cmd === 'should_show_onboarding') return false;
        if (cmd === 'get_agents') return AGENTS;
        if (cmd === 'list_scheduled_jobs') return [];
        // #3190 (C65 final): \`api.syncProjectsWithNotebooks()\` actually
        // calls the \`get_projects\` command (its own \`sync_projects_with
        // _notebooks doesn't exist, use get_projects instead\` comment,
        // api.ts) — that's the one Sidebar.svelte's loadData() awaits on
        // mount. list_projects/sync_projects_with_notebooks covered too
        // in case another call site still uses either.
        if ((cmd === 'get_projects' || cmd === 'list_projects' || cmd === 'sync_projects_with_notebooks') && PROJECTS_OVERRIDE !== null) return PROJECTS_OVERRIDE;
        return originalInvoke(cmd, args);
      };
    })();
  `;
  // ADR-0070 (#3195 PR B): the __PANE_TEST_MODE__ bypass this fixture used
  // to inject (mirroring `e2e/fixtures.ts`) is removed with the AuthScreen
  // gate it existed to satisfy. No replacement mock is needed: the base
  // mock's `get_ai_setting` handler already returns `null` for any key
  // (see `tauri-mock.ts`'s default), which is correct "off" semantics for
  // `account_enabled` with zero new mock lines — `initAuth()` reads the
  // flag, gets `null` (not `'true'`), treats it as off, and resolves
  // immediately with no network call. The harness boots to Home because
  // that is what the app does.
  await page.addInitScript({ content: seedScript });
  await page.goto('http://localhost:5173');
  await page.waitForSelector('[data-testid="app-loaded"]', { timeout: 15000 });
}

/**
 * ADR-0070 (#3195 PR B) red-first test 1 (T1) deliverable: the mocked-
 * harness half of "Pane opens to Home — on every launch, with no sign-in
 * and no network call in the launch path" (Decision 1).
 *
 * Asserts, against the mocked-IPC harness (`setupHomeView` above):
 * - `[data-testid="app-loaded"]` is visible — boot reached Home.
 * - `.auth-screen` count is 0 — the deleted `AuthScreen.svelte`'s root
 *   class never mounts.
 * - `.auth-loading` is not mounted — `authReady` settled, the loading
 *   placeholder unmounted.
 * - `window.__PANE_ACCOUNT_SYNC_CALLS__` (tauri-mock.ts's IPC recorder,
 *   covering the exact 20 commands `require_account_enabled()` guards) is
 *   empty — no account/sync IPC call was made reaching Home, confirming
 *   the "no network call in the launch path" half of Decision 1 that a
 *   DOM assertion alone can't prove.
 *
 * Call this AFTER `setupHomeView`, which already waits for `app-loaded`;
 * this helper re-asserts it defensively so it's a complete, self-
 * contained check if ever called standalone.
 */
export async function assertLocalFirstBoot(page: Page) {
  await page.waitForSelector('[data-testid="app-loaded"]', { timeout: 15000 });
  await expect(page.locator('[data-testid="app-loaded"]')).toBeVisible();
  await expect(page.locator('.auth-screen')).toHaveCount(0);
  await expect(page.locator('.auth-loading')).toHaveCount(0);
  const ipcCalls = await page.evaluate(
    () => (window as unknown as { __PANE_ACCOUNT_SYNC_CALLS__?: string[] }).__PANE_ACCOUNT_SYNC_CALLS__ ?? []
  );
  expect(ipcCalls).toEqual([]);
}

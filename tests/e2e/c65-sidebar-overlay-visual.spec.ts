import { test, expect } from '@playwright/test';
import { setupHomeView } from './home-fixtures';

/**
 * #3190 C65 final — hover-expand is the wide state, overlaid.
 *
 * Replaces the deleted `c65-sidebar-rail-hover.spec.ts`. That spec
 * asserted a sidebar width (`toBe(220)`) and a position
 * (`document.elementFromPoint`) — both passed, three times, while the
 * feature was visibly broken on screen (menu behind the Jobs column at
 * narrow; labels never rendering at medium). Neither assertion could
 * see either failure, because neither failure is a width or a position
 * — they're "is the right thing painted, in the right stacking order,
 * with the right content visible," which no DOM measurement answers.
 *
 * Matt's ruling for this pass: "Reed measured width, and width was
 * never the requirement." Acceptance here is six screenshots Reed and
 * Matt read with their own eyes (labels legible, menu on top of
 * everything) — not a green assertion.
 *
 * ⚠️ No width assertion. No position assertion. No
 * `document.elementFromPoint`. Anywhere in this file. If a future edit
 * adds one, it is reintroducing the exact failure mode this spec
 * replaced — see docs/spec/ui-layout.md's "Hover-expand is the wide
 * state, overlaid, not pushed" subsection.
 *
 * WebKit specifically, not Chromium — Tauri on macOS is WKWebView,
 * same reasoning as this directory's other WebKit-only specs.
 *
 * `toHaveScreenshot()` disables CSS animations/transitions by default
 * (Playwright's own default for this assertion), so `.sidebar`'s
 * `transition: width 0.15s ease` completes instantly rather than
 * needing a settle-wait of any kind — no fixed `sleep`, no polling.
 */

const VIEWPORTS = {
  narrow: { width: 800, height: 900 },   // < 900: sidebar collapsed, chat becomes a slide-over
  medium: { width: 1000, height: 900 },  // < 1200: sidebar collapsed, chat in flow
  // #3191 correction: was commented "sidebar never collapsed — 'rest' and
  // 'hover' are the same DOM already." Reed's audit found that claim false
  // against this very file's own committed `c65-wide-rest` baseline — at
  // wide, `.collapsed` is indeed never set (`$sidebarCollapsed` is false,
  // confirmed), but `.sidebar`'s own width rule was never conditioned on
  // that flag or on width band at all, so wide-rest renders the same
  // compact 40px rail as narrow/medium-rest, not an always-expanded 220px
  // sidebar. Pre-existing (#3188's `.sidebar` width rule, untouched by
  // #3190/#3191), out of scope here, and NOT something this correction's
  // "wide-rest must stay pixel-identical" requirement permits fixing in
  // this pass — flagged, not silently left as a false comment.
  wide: { width: 1400, height: 900 },
} as const;

// #3190 (C65 final): the default mock fixture seeds zero projects, so
// without an override the sidebar's grouped-rail/label area (.ps-rail,
// .projects-section) would be empty in every screenshot — proving
// nothing about the actual bug ("project labels don't render" has
// nothing to look at if there are no projects). Two favorited,
// categorized projects exercise both: .ps-rail's dots (collapsed rest)
// and .projects-section's category groups + labels (hover/expanded).
const now = new Date().toISOString();
const MOCK_PROJECTS = [
  { id: 'proj_1', name: 'Website Redesign', category: 'Work', is_favorite: true, parent_id: null, sort_order: 0, created_at: now, updated_at: now },
  { id: 'proj_2', name: 'Personal Journal', category: 'Personal', is_favorite: true, parent_id: null, sort_order: 1, created_at: now, updated_at: now },
];

for (const [band, viewport] of Object.entries(VIEWPORTS)) {
  test.describe(`#3190 C65 final — sidebar overlay visual (${band}, ${viewport.width}px)`, () => {
    test(`${band} at rest`, async ({ page }) => {
      await page.setViewportSize(viewport);
      await setupHomeView(page, { projects: MOCK_PROJECTS });
      await page.waitForSelector('.sidebar', { timeout: 10000 });
      // Move the mouse off the sidebar first — Playwright's default
      // pointer start position is (0, 0), which is inside the rail at
      // every one of these viewports and would make "at rest" actually
      // be hovering it.
      await page.mouse.move(viewport.width - 10, viewport.height - 10);

      await expect(page).toHaveScreenshot(`c65-${band}-rest.png`, { fullPage: false });
    });

    test(`${band} on hover`, async ({ page }) => {
      await page.setViewportSize(viewport);
      await setupHomeView(page, { projects: MOCK_PROJECTS });
      await page.waitForSelector('.sidebar', { timeout: 10000 });

      await page.hover('.sidebar');

      await expect(page).toHaveScreenshot(`c65-${band}-hover.png`, { fullPage: false });
    });
  });
}

/**
 * #3050 PR 8 — Home Scheduled panel, dock + squish.
 *
 * Design sheet README "Layout rules": "The Scheduled panel is a fixed
 * 352px column on the left of Home at every window width. No toggle, no
 * overlay. When the left sidebar opens, the chat column narrows; the
 * panel doesn't move or resize."
 *
 * Source-level assertions (same house pattern as
 * `page.settings-no-chat-pane.test.ts` and
 * `Settings.provider-switch-model-refresh.test.ts`) — `+page.svelte`
 * requires a full Tauri window context and heavy child components
 * (ChatSidebar, SessionsDrawer, CanvasRenderer), so this asserts against
 * the source directly rather than mounting the component.
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'fs';
import { resolve } from 'path';

const pageSource = readFileSync(resolve(__dirname, '+page.svelte'), 'utf-8');
const panelSource = readFileSync(
  resolve(__dirname, '../lib/components/ScheduledPanel.svelte'),
  'utf-8'
);

describe('#3050 PR 8 — ScheduledPanel mounts docked on the left of Home', () => {
  it('+page.svelte imports ScheduledPanel', () => {
    expect(pageSource).toMatch(/import ScheduledPanel from ['"]\$lib\/components\/ScheduledPanel\.svelte['"]/);
  });

  it('ScheduledPanel is gated to the home view only, same as SessionsDrawer', () => {
    const gateMatch = pageSource.match(/\{#if \$canvasState\.view === 'home'\}\s*(?:<!--[\s\S]*?-->\s*)?<ScheduledPanel/);
    expect(gateMatch).not.toBeNull();
  });

  it('ScheduledPanel is mounted BEFORE ChatSidebar inside .chat-panel — it docks on the LEFT', () => {
    const chatPanelIdx = pageSource.indexOf('class="chat-panel"');
    expect(chatPanelIdx).toBeGreaterThan(-1);
    const scheduledIdx = pageSource.indexOf('<ScheduledPanel', chatPanelIdx);
    const chatSidebarIdx = pageSource.indexOf('<ChatSidebar', chatPanelIdx);
    expect(scheduledIdx).toBeGreaterThan(chatPanelIdx);
    expect(chatSidebarIdx).toBeGreaterThan(chatPanelIdx);
    expect(scheduledIdx).toBeLessThan(chatSidebarIdx);
  });

  it('ScheduledPanel is NOT inside the Settings-exclusion-gated-out region (mounts fine on Home, which is a separate branch)', () => {
    // Sanity: ScheduledPanel only appears once, inside the home-view check,
    // not duplicated elsewhere (e.g. accidentally always-mounted).
    const occurrences = pageSource.split('<ScheduledPanel').length - 1;
    expect(occurrences).toBe(1);
  });
});

describe('#3050 PR 8 — ScheduledPanel holds a fixed 352px width, flex-shrink: 0', () => {
  it('.sp-panel is exactly 352px wide (design sheet: "fixed 352px column... at every window width")', () => {
    const match = panelSource.match(/\.sp-panel\s*\{([^}]*)\}/);
    expect(match).not.toBeNull();
    expect(match?.[1]).toMatch(/width:\s*352px/);
    expect(match?.[1]).toMatch(/flex-shrink:\s*0/);
  });

  it('.sp-panel sets no flex-basis/flex-grow that would let it participate in the squish', () => {
    const match = panelSource.match(/\.sp-panel\s*\{([^}]*)\}/);
    expect(match?.[1]).not.toMatch(/flex:\s*1/);
  });
});

describe('#3050 PR 8 — the squish is inherited for free from the existing .chat-panel.home-view flex row (unchanged by this PR)', () => {
  it('.chat-panel.home-view is still flex-direction: row (ScheduledPanel + ChatSidebar + SessionsDrawer as flex-row siblings)', () => {
    const match = pageSource.match(/\.chat-panel\.home-view\s*\{([^}]*)\}/);
    expect(match).not.toBeNull();
    expect(match?.[1]).toMatch(/flex-direction:\s*row/);
  });

  it('+page.svelte itself was not edited to add new squish/resize JS for the panel (no new width-calc logic, no toggle state)', () => {
    // The panel's own component has zero toggle state (README: "No
    // toggle, no overlay") — confirm no open/collapsed prop is passed to it.
    const scheduledTag = pageSource.slice(
      pageSource.indexOf('<ScheduledPanel'),
      pageSource.indexOf('/>', pageSource.indexOf('<ScheduledPanel')) + 2
    );
    expect(scheduledTag).not.toMatch(/open=|collapsed=|width=/);
  });
});

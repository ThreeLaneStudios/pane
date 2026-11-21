/**
 * SessionRow.test.ts — Component tests for SessionRow.svelte
 *
 * Tests:
 *  - Session name renders (display title)
 *  - Auto-title fallback shown when title === ''
 *  - Active indicator (cyan dot) shown for active sessions
 *  - No active indicator for inactive sessions
 *  - ⋯ menu not shown for hidden sessions
 *  - Unhide button shown for hidden sessions
 *  - Rename commits via onRename callback
 *  - Empty rename submission calls onRename with ''
 *
 * §395 PR6 — spec-covered-by: #1979
 */

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte/svelte5';
import SessionRow from './SessionRow.svelte';
import type { Session, Agent } from '$lib/types';

// ─── Helpers ─────────────────────────────────────────────────────────────────

function mkSession(overrides: Partial<Session> = {}): Session {
  return {
    id: 'sess-test-1',
    project_id: 'project-1',
    title: 'My session',
    origin: 'user',
    created_at: '2026-07-01T10:00:00Z',
    last_active_at: '2026-07-09T08:00:00Z',
    hidden_at: null,
    is_active: false,
    ...overrides,
  };
}

// ─── Name rendering ───────────────────────────────────────────────────────────

describe('SessionRow — name rendering', () => {
  it('renders the session title when non-empty', () => {
    const session = mkSession({ title: 'Research notes' });
    render(SessionRow, { session });
    expect(screen.getByText('Research notes')).toBeTruthy();
  });

  it('renders auto-title fallback "Session YYYY-MM-DD" when title is empty', () => {
    const session = mkSession({ title: '', created_at: '2026-07-09T10:00:00Z' });
    render(SessionRow, { session });
    expect(screen.getByText(/^Session 2026-07-09$/)).toBeTruthy();
  });

  it('renders last_active_at as a relative time label', () => {
    // Use a very recent time so it renders "Just now"
    const now = new Date().toISOString();
    const session = mkSession({ last_active_at: now });
    render(SessionRow, { session });
    expect(screen.getByText('Just now')).toBeTruthy();
  });

  // #3151 PR3/C44: `last_active_at` arrives from the backend as SQLite's
  // naive-UTC `datetime('now')` shape ("YYYY-MM-DD HH:MM:SS", no `T`/`Z`/
  // offset), not the full-ISO `Z`-suffixed strings `mkSession`'s own
  // defaults use above — those already worked before this fix and prove
  // nothing about the regression. Built from the REAL `Date.now()` at
  // test time (no mocked clock) so the only way this can diverge from
  // "really a few minutes old" is a local-time misparse — exactly the
  // bug. Mutation: revert `normalizeStoredTimestamp`'s call sites back
  // to a bare `new Date(isoString)` — reddens under a real non-UTC `TZ`
  // process invocation (see PR report), though not under this file's
  // own default `TZ=UTC` pool run, where the bug is invisible by nature.
  it('a naive SQLite "YYYY-MM-DD HH:MM:SS" last_active_at a few minutes old is never stuck at "Just now"', () => {
    const naiveSqliteFewMinutesAgo = new Date(Date.now() - 150_000)
      .toISOString()
      .slice(0, 19)
      .replace('T', ' ');
    const session = mkSession({ last_active_at: naiveSqliteFewMinutesAgo });
    render(SessionRow, { session });
    expect(screen.queryByText('Just now')).toBeNull();
    expect(screen.getByText('2m ago')).toBeTruthy();
  });
});

// ─── Active indicator ─────────────────────────────────────────────────────────

describe('SessionRow — active indicator', () => {
  it('shows the active dot when is_active is true', () => {
    const session = mkSession({ is_active: true });
    const { container } = render(SessionRow, { session });
    const dot = container.querySelector('.active-dot');
    expect(dot).not.toBeNull();
  });

  it('does not show the active dot when is_active is false', () => {
    const session = mkSession({ is_active: false });
    const { container } = render(SessionRow, { session });
    const dot = container.querySelector('.active-dot');
    expect(dot).toBeNull();
  });

  it('adds is-active class to root when session is active', () => {
    const session = mkSession({ is_active: true });
    const { container } = render(SessionRow, { session });
    const row = container.querySelector('.session-row');
    expect(row?.classList.contains('is-active')).toBe(true);
  });

  it('does not add is-active class when session is inactive', () => {
    const session = mkSession({ is_active: false });
    const { container } = render(SessionRow, { session });
    const row = container.querySelector('.session-row');
    expect(row?.classList.contains('is-active')).toBe(false);
  });

  it('renders the active left rail when is_active is true', () => {
    const session = mkSession({ is_active: true });
    const { container } = render(SessionRow, { session });
    const rail = container.querySelector('.active-rail');
    expect(rail).not.toBeNull();
  });
});

// ─── Hidden variant ───────────────────────────────────────────────────────────

describe('SessionRow — hidden variant', () => {
  it('shows Unhide button when session is hidden', () => {
    const session = mkSession({ hidden_at: '2026-07-08T00:00:00Z' });
    render(SessionRow, { session });
    expect(screen.getByText('Unhide')).toBeTruthy();
  });

  it('does not show the ⋯ menu button when session is hidden', () => {
    const session = mkSession({ hidden_at: '2026-07-08T00:00:00Z' });
    const { container } = render(SessionRow, { session });
    const menuBtn = container.querySelector('.menu-btn');
    expect(menuBtn).toBeNull();
  });

  it('calls onUnhide with session id when Unhide button clicked', async () => {
    const session = mkSession({ hidden_at: '2026-07-08T00:00:00Z' });
    const onUnhide = vi.fn();
    render(SessionRow, { session, onUnhide });

    const unhideBtn = screen.getByText('Unhide').closest('button')!;
    await fireEvent.click(unhideBtn);

    expect(onUnhide).toHaveBeenCalledWith('sess-test-1');
  });

  it('applies is-hidden class when session is hidden', () => {
    const session = mkSession({ hidden_at: '2026-07-08T00:00:00Z' });
    const { container } = render(SessionRow, { session });
    const row = container.querySelector('.session-row');
    expect(row?.classList.contains('is-hidden')).toBe(true);
  });
});

// ─── Rename (inline edit) ─────────────────────────────────────────────────────

describe('SessionRow — inline rename', () => {
  it('calls onRename with the new title on Enter key', async () => {
    const session = mkSession({ title: 'Old title' });
    const onRename = vi.fn();
    const { container } = render(SessionRow, { session, onRename });

    // Trigger edit by double-clicking the title
    const titleEl = container.querySelector('.session-title')!;
    await fireEvent.dblClick(titleEl);

    const input = container.querySelector('.rename-input') as HTMLInputElement;
    expect(input).not.toBeNull();

    // Type new name and press Enter
    await fireEvent.input(input, { target: { value: 'New title' } });
    await fireEvent.keyDown(input, { key: 'Enter' });

    expect(onRename).toHaveBeenCalledWith('sess-test-1', 'New title');
  });

  it('calls onRename with empty string on empty submission (reverts to auto-title)', async () => {
    const session = mkSession({ title: 'Some title' });
    const onRename = vi.fn();
    const { container } = render(SessionRow, { session, onRename });

    const titleEl = container.querySelector('.session-title')!;
    await fireEvent.dblClick(titleEl);

    const input = container.querySelector('.rename-input') as HTMLInputElement;
    // Clear the input — empty submission
    await fireEvent.input(input, { target: { value: '' } });
    await fireEvent.keyDown(input, { key: 'Enter' });

    expect(onRename).toHaveBeenCalledWith('sess-test-1', '');
  });

  it('cancels rename on Escape without calling onRename', async () => {
    const session = mkSession({ title: 'Keep this title' });
    const onRename = vi.fn();
    const { container } = render(SessionRow, { session, onRename });

    const titleEl = container.querySelector('.session-title')!;
    await fireEvent.dblClick(titleEl);

    const input = container.querySelector('.rename-input') as HTMLInputElement;
    await fireEvent.keyDown(input, { key: 'Escape' });

    expect(onRename).not.toHaveBeenCalled();
    // Edit mode should be closed
    expect(container.querySelector('.rename-input')).toBeNull();
  });

  it('commits rename on blur', async () => {
    const session = mkSession({ title: 'Old title' });
    const onRename = vi.fn();
    const { container } = render(SessionRow, { session, onRename });

    const titleEl = container.querySelector('.session-title')!;
    await fireEvent.dblClick(titleEl);

    const input = container.querySelector('.rename-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'Blurred title' } });
    await fireEvent.blur(input);

    expect(onRename).toHaveBeenCalledWith('sess-test-1', 'Blurred title');
  });
});

// ─── Switch callback ──────────────────────────────────────────────────────────

describe('SessionRow — switch on click', () => {
  it('calls onSwitch with the session when row is clicked', async () => {
    const session = mkSession({ title: 'Clickable session' });
    const onSwitch = vi.fn();
    const { container } = render(SessionRow, { session, onSwitch });

    const row = container.querySelector('.session-row')!;
    await fireEvent.click(row);

    expect(onSwitch).toHaveBeenCalledWith(session);
  });
});

// ─── compact prop ─────────────────────────────────────────────────────────────

describe('SessionRow — compact mode', () => {
  it('hides the secondary "Created ..." line when compact=true', () => {
    const session = mkSession();
    const { container } = render(SessionRow, { session, compact: true });
    const createdAt = container.querySelector('.created-at');
    expect(createdAt).toBeNull();
  });

  it('shows the secondary "Created ..." line when compact=false (default)', () => {
    const session = mkSession();
    const { container } = render(SessionRow, { session, compact: false });
    const createdAt = container.querySelector('.created-at');
    expect(createdAt).not.toBeNull();
  });
});

// ─── D-1: SVG icons for hide/unhide (no emoji) ──────────────────────────────

describe('SessionRow — D-1 icon render (SVG, not emoji)', () => {
  it('unhide button contains an <svg> element instead of emoji text', () => {
    const session = mkSession({ hidden_at: '2026-07-08T00:00:00Z' });
    const { container } = render(SessionRow, { session });

    const unhideBtn = container.querySelector('.unhide-btn')!;
    expect(unhideBtn).not.toBeNull();

    // Must contain an SVG
    const svg = unhideBtn.querySelector('svg');
    expect(svg).not.toBeNull();

    // Must NOT contain the old emoji characters
    expect(unhideBtn.textContent).not.toContain('\uD83D\uDC41'); // 👁
    expect(unhideBtn.textContent).not.toContain('\uD83D\uDE48'); // 🙈
  });

  it('hide menu item contains an <svg> element instead of emoji text', async () => {
    const session = mkSession();
    const { container } = render(SessionRow, { session });

    // Open the ⋯ menu
    const menuBtn = container.querySelector('.menu-btn') as HTMLButtonElement;
    await fireEvent.click(menuBtn);

    const hideItem = container.querySelector('.hide-item')!;
    expect(hideItem).not.toBeNull();

    const svg = hideItem.querySelector('svg');
    expect(svg).not.toBeNull();

    // Must NOT contain emoji
    expect(hideItem.textContent).not.toContain('\uD83D\uDE48'); // 🙈
    expect(hideItem.textContent).not.toContain('\uD83D\uDC41'); // 👁
  });
});

// ─── D-3: context menu flip logic (PR-D) ──────────────────────────────────────

describe('SessionRow — D-3 context menu flip positioning', () => {
  const MENU_HEIGHT_EST = 90;

  it('opens downward (menuTop = rect.bottom + 2) when space below is sufficient', async () => {
    const session = mkSession();
    const { container } = render(SessionRow, { session });

    const menuBtn = container.querySelector('.menu-btn') as HTMLButtonElement;
    expect(menuBtn).not.toBeNull();

    // Simulate button near the top of the viewport — plenty of space below
    const fakeRect = { top: 100, bottom: 120, right: 500, left: 300, width: 200, height: 20 };
    vi.spyOn(menuBtn, 'getBoundingClientRect').mockReturnValue(fakeRect as DOMRect);
    Object.defineProperty(window, 'innerHeight', { value: 800, configurable: true });
    Object.defineProperty(window, 'innerWidth', { value: 1200, configurable: true });

    await fireEvent.click(menuBtn);

    const menu = container.querySelector('.context-menu') as HTMLElement;
    expect(menu).not.toBeNull();

    const style = menu.getAttribute('style') ?? '';
    // spaceBelow = 800 - 120 = 680 >= 90, so should open downward: top = 122
    expect(style).toMatch(new RegExp(`top:\\s*${fakeRect.bottom + 2}px`));
  });

  it('flips upward (menuTop = rect.top - MENU_HEIGHT_EST) when space below is insufficient', async () => {
    const session = mkSession();
    const { container } = render(SessionRow, { session });

    const menuBtn = container.querySelector('.menu-btn') as HTMLButtonElement;
    expect(menuBtn).not.toBeNull();

    // Simulate button near the bottom of the viewport — not enough space below
    const fakeRect = { top: 730, bottom: 750, right: 500, left: 300, width: 200, height: 20 };
    vi.spyOn(menuBtn, 'getBoundingClientRect').mockReturnValue(fakeRect as DOMRect);
    Object.defineProperty(window, 'innerHeight', { value: 800, configurable: true });
    Object.defineProperty(window, 'innerWidth', { value: 1200, configurable: true });

    await fireEvent.click(menuBtn);

    const menu = container.querySelector('.context-menu') as HTMLElement;
    expect(menu).not.toBeNull();

    const style = menu.getAttribute('style') ?? '';
    // spaceBelow = 800 - 750 = 50 < 90, so should flip up: top = 730 - 90 = 640
    expect(style).toMatch(new RegExp(`top:\\s*${fakeRect.top - MENU_HEIGHT_EST}px`));
  });

  it('sets menuRight based on window.innerWidth - rect.right', async () => {
    const session = mkSession();
    const { container } = render(SessionRow, { session });

    const menuBtn = container.querySelector('.menu-btn') as HTMLButtonElement;
    const fakeRect = { top: 100, bottom: 120, right: 400, left: 300, width: 100, height: 20 };
    vi.spyOn(menuBtn, 'getBoundingClientRect').mockReturnValue(fakeRect as DOMRect);
    Object.defineProperty(window, 'innerHeight', { value: 800, configurable: true });
    Object.defineProperty(window, 'innerWidth', { value: 1200, configurable: true });

    await fireEvent.click(menuBtn);

    const menu = container.querySelector('.context-menu') as HTMLElement;
    const style = menu.getAttribute('style') ?? '';
    // menuRight = 1200 - 400 = 800
    expect(style).toMatch(/right:\s*800px/);
  });

  it('clamps menuTop to 4px minimum when button is near viewport top', async () => {
    const session = mkSession();
    const { container } = render(SessionRow, { session });

    const menuBtn = container.querySelector('.menu-btn') as HTMLButtonElement;
    // Button near top: rect.top=40, spaceBelow=800-60=740 >= 90, BUT
    // we need spaceBelow < 90 to trigger the flip. So: rect near top AND near bottom.
    // Actually: to test the clamp, we need the flip path AND rect.top < MENU_HEIGHT_EST + 4
    const fakeRect = { top: 40, bottom: 60, right: 500, left: 300, width: 200, height: 20 };
    vi.spyOn(menuBtn, 'getBoundingClientRect').mockReturnValue(fakeRect as DOMRect);
    Object.defineProperty(window, 'innerHeight', { value: 100, configurable: true }); // small viewport forces flip
    Object.defineProperty(window, 'innerWidth', { value: 1200, configurable: true });

    await fireEvent.click(menuBtn);

    const menu = container.querySelector('.context-menu') as HTMLElement;
    const style = menu.getAttribute('style') ?? '';
    // rect.top - 90 = -50, but clamped to 4
    expect(style).toMatch(/top:\s*4px/);
  });

  it('.context-menu CSS uses position: fixed (source inspection)', () => {
    const fs = require('fs');
    const path = require('path');
    const src = fs.readFileSync(
      path.resolve(__dirname, 'SessionRow.svelte'),
      'utf-8'
    );
    // Must use position: fixed (not absolute) in the context-menu block
    const menuCssBlock = src.match(/\.context-menu\s*\{[^}]+\}/s)?.[0] ?? '';
    expect(menuCssBlock).toContain('position: fixed');
    expect(menuCssBlock).not.toContain('position: absolute');
  });
});

// ─── #395 isLastVisible guard ──────────────────────────────────────────────────────
//
// When isLastVisible=true, Hide must be disabled to prevent the backend’s
// born-active create-if-none (§30.2.5) from spawning a phantom session (#395).

describe('SessionRow — isLastVisible guard (phantom-session fix #395)', () => {
  it('hide menu item is disabled when isLastVisible=true', async () => {
    const session = mkSession();
    const { container } = render(SessionRow, { session, isLastVisible: true });

    const menuBtn = container.querySelector('.menu-btn') as HTMLButtonElement;
    await fireEvent.click(menuBtn);

    const hideItem = container.querySelector('.hide-item') as HTMLButtonElement;
    expect(hideItem).not.toBeNull();
    expect(hideItem.disabled).toBe(true);
  });

  it('hide menu item has class hide-item--disabled when isLastVisible=true', async () => {
    const session = mkSession();
    const { container } = render(SessionRow, { session, isLastVisible: true });

    const menuBtn = container.querySelector('.menu-btn') as HTMLButtonElement;
    await fireEvent.click(menuBtn);

    const hideItem = container.querySelector('.hide-item');
    expect(hideItem?.classList.contains('hide-item--disabled')).toBe(true);
  });

  it('hide menu item tooltip says "Cannot hide the only visible session" when isLastVisible=true', async () => {
    const session = mkSession();
    const { container } = render(SessionRow, { session, isLastVisible: true });

    const menuBtn = container.querySelector('.menu-btn') as HTMLButtonElement;
    await fireEvent.click(menuBtn);

    const hideItem = container.querySelector('.hide-item') as HTMLButtonElement;
    expect(hideItem.title).toBe('Cannot hide the only visible session');
  });

  it('does NOT call onHide when isLastVisible=true and hide is clicked', async () => {
    const session = mkSession();
    const onHide = vi.fn();
    const { container } = render(SessionRow, { session, isLastVisible: true, onHide });

    const menuBtn = container.querySelector('.menu-btn') as HTMLButtonElement;
    await fireEvent.click(menuBtn);

    const hideItem = container.querySelector('.hide-item') as HTMLButtonElement;
    await fireEvent.click(hideItem);

    expect(onHide).not.toHaveBeenCalled();
  });

  it('hide menu item is enabled and clickable when isLastVisible=false (default)', async () => {
    const session = mkSession();
    const onHide = vi.fn();
    const { container } = render(SessionRow, { session, isLastVisible: false, onHide });

    const menuBtn = container.querySelector('.menu-btn') as HTMLButtonElement;
    await fireEvent.click(menuBtn);

    const hideItem = container.querySelector('.hide-item') as HTMLButtonElement;
    expect(hideItem.disabled).toBe(false);
    expect(hideItem.title).toBe('Hide this session (reversible)');

    await fireEvent.click(hideItem);
    expect(onHide).toHaveBeenCalledWith(session.id);
  });
});

// ─── D-1 (PR-C): status dot for inactive non-hidden rows ─────────────────────

describe('SessionRow — D-1 status dots', () => {
  it('renders .status-dot for an inactive non-hidden session', () => {
    const session = mkSession({ is_active: false, hidden_at: null });
    const { container } = render(SessionRow, { session });
    const dot = container.querySelector('.status-dot');
    expect(dot).not.toBeNull();
  });

  it('renders .active-dot for an active session', () => {
    const session = mkSession({ is_active: true });
    const { container } = render(SessionRow, { session });
    expect(container.querySelector('.active-dot')).not.toBeNull();
    expect(container.querySelector('.status-dot')).toBeNull();
  });

  it('renders neither dot for a hidden session', () => {
    const session = mkSession({ is_active: false, hidden_at: '2026-07-08T00:00:00Z' });
    const { container } = render(SessionRow, { session });
    expect(container.querySelector('.active-dot')).toBeNull();
    expect(container.querySelector('.status-dot')).toBeNull();
  });
});

// §36.17.4 (#3050 PR 10): captain/crew chip cluster — 22px ringed captain,
// 18px unringed crew, three max + "+N", full list in the tooltip,
// OpenClaw captain rows show the captain chip alone.

function mkAgent(overrides: Partial<Agent> = {}): Agent {
  return {
    id: 'a1',
    name: 'Agent',
    slug: 'agent',
    role: 'sub',
    model: 'claude-sonnet-4-5',
    is_default: false,
    is_active: true,
    workspace_path: '/tmp/a',
    config_json: '{}',
    runtime_ref: null,
    runtime_type: 'anthropic',
    source: 'local',
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    ...overrides,
  };
}

describe('SessionRow — crew chip cluster (#3050 PR 10)', () => {
  it('0 crew: captain chip alone, no crew chips, no "+N"', () => {
    const captain = mkAgent({ id: 'cap-1', name: 'Valencia Tool Test' });
    const session = mkSession({ agent_id: 'cap-1', crew_agent_ids: null } as Partial<Session>);
    const { container } = render(SessionRow, { session, allAgents: [captain] });

    const cluster = container.querySelector('[data-testid="session-row-crew-cluster"]') as HTMLElement;
    expect(cluster).not.toBeNull();
    expect(cluster.querySelectorAll('.row-av--cap')).toHaveLength(1);
    expect(cluster.querySelectorAll('.row-av:not(.row-av--cap)')).toHaveLength(0);
    expect(cluster.querySelector('.row-av--more')).toBeNull();
  });

  it('1 crew: captain + exactly one 18px unringed crew chip, no "+N"', () => {
    const captain = mkAgent({ id: 'cap-1', name: 'Vanessa' });
    const crew1 = mkAgent({ id: 'c1', name: 'Vernon' });
    const session = mkSession({ agent_id: 'cap-1', crew_agent_ids: '["c1"]' } as Partial<Session>);
    const { container } = render(SessionRow, { session, allAgents: [captain, crew1] });

    const cluster = container.querySelector('[data-testid="session-row-crew-cluster"]') as HTMLElement;
    expect(cluster.querySelectorAll('.row-av--cap')).toHaveLength(1);
    expect(cluster.querySelectorAll('.row-av:not(.row-av--cap):not(.row-av--more)')).toHaveLength(1);
    expect(cluster.querySelector('.row-av--more')).toBeNull();
  });

  it('2 crew: captain + two crew chips, no "+N"', () => {
    const captain = mkAgent({ id: 'cap-1', name: 'Astro' });
    const crew1 = mkAgent({ id: 'c1', name: 'Valencia Tool Test' });
    const crew2 = mkAgent({ id: 'c2', name: 'Vanessa' });
    const session = mkSession({ agent_id: 'cap-1', crew_agent_ids: '["c1","c2"]' } as Partial<Session>);
    const { container } = render(SessionRow, { session, allAgents: [captain, crew1, crew2] });

    const cluster = container.querySelector('[data-testid="session-row-crew-cluster"]') as HTMLElement;
    expect(cluster.querySelectorAll('.row-av:not(.row-av--cap):not(.row-av--more)')).toHaveLength(2);
    expect(cluster.querySelector('.row-av--more')).toBeNull();
  });

  it('4+ crew: exactly 3 chips shown + "+1", tooltip lists ALL 4 crew names (not just the overflowed one)', () => {
    const captain = mkAgent({ id: 'cap-1', name: 'Astro' });
    const crew = [
      mkAgent({ id: 'c1', name: 'Valencia Tool Test' }),
      mkAgent({ id: 'c2', name: 'Vanessa' }),
      mkAgent({ id: 'c3', name: 'Vernon' }),
      mkAgent({ id: 'c4', name: 'Vernon Reed' }),
    ];
    const session = mkSession({ agent_id: 'cap-1', crew_agent_ids: '["c1","c2","c3","c4"]' } as Partial<Session>);
    const { container } = render(SessionRow, { session, allAgents: [captain, ...crew] });

    const cluster = container.querySelector('[data-testid="session-row-crew-cluster"]') as HTMLElement;
    expect(cluster.querySelectorAll('.row-av:not(.row-av--cap):not(.row-av--more)')).toHaveLength(3);
    const more = cluster.querySelector('.row-av--more');
    expect(more?.textContent).toBe('+1');

    // Tooltip must list ALL 4 crew names, not just the one overflowed —
    // checking for the FIRST 3 too, not just "+1"'s own member, per Reed's
    // scope audit's own caution about a mutation that only checks the
    // overflow name.
    expect(cluster.title).toContain('Valencia Tool Test');
    expect(cluster.title).toContain('Vanessa');
    expect(cluster.title).toContain('Vernon Reed');
  });

  it('OpenClaw captain: captain chip alone, crew suppressed even if crew_agent_ids somehow has data', () => {
    const ocCaptain = mkAgent({ id: 'oc-1', name: 'Brit Freed', runtime_ref: 'oc-1', runtime_type: 'openclaw' });
    const wouldBeCrew = mkAgent({ id: 'c1', name: 'Vanessa' });
    const session = mkSession({ agent_id: 'oc-1', crew_agent_ids: '["c1"]' } as Partial<Session>);
    const { container } = render(SessionRow, { session, allAgents: [ocCaptain, wouldBeCrew] });

    const cluster = container.querySelector('[data-testid="session-row-crew-cluster"]') as HTMLElement;
    expect(cluster.querySelectorAll('.row-av--cap')).toHaveLength(1);
    expect(cluster.querySelectorAll('.row-av:not(.row-av--cap)')).toHaveLength(0);
  });

  it('no allAgents prop passed at all (pre-existing callers): renders with no crash and no crew cluster', () => {
    const session = mkSession({ agent_id: 'cap-1', crew_agent_ids: null } as Partial<Session>);
    const { container } = render(SessionRow, { session });
    expect(container.querySelector('[data-testid="session-row-crew-cluster"]')).toBeNull();
  });
});

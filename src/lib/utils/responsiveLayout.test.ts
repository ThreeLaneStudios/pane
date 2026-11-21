/**
 * responsiveLayout.test.ts — #3182 C64.
 *
 * Pure function, no DOM — jsdom-coverable exactly as the issue
 * requires for anything that isn't genuinely real-layout-dependent.
 * The real-layout half of C64 (the drawer's actual DOM parent, the
 * resize-and-back visual restore) has NO jsdom test — see
 * `tests/e2e/c64-narrow-layout-restore.spec.ts` (WebKit) for that;
 * this file only proves the state machine, not anything rendered.
 */
import { describe, it, expect } from 'vitest';
import { deriveResponsiveState, type ResponsiveLayoutState } from './responsiveLayout';

function wideOpenState(): ResponsiveLayoutState {
  return { sidebarCollapsed: false, autoCollapsed: false, chatSidebarOpen: true, autoChatCollapsed: false };
}

describe('#3182 C64 — deriveResponsiveState', () => {
  it('narrow→medium→wide sequence restores chatSidebarOpen to its pre-narrow value', () => {
    let state = wideOpenState();

    // Narrow: sidebar collapses, chat force-closes, both flagged auto.
    state = deriveResponsiveState(800, state);
    expect(state.sidebarCollapsed).toBe(true);
    expect(state.autoCollapsed).toBe(true);
    expect(state.chatSidebarOpen).toBe(false);
    expect(state.autoChatCollapsed).toBe(true);

    // Medium: sidebar stays collapsed; chat untouched (still closed,
    // still flagged — C64's three states, chat has no medium-specific
    // behavior of its own).
    state = deriveResponsiveState(1000, state);
    expect(state.sidebarCollapsed).toBe(true);
    expect(state.chatSidebarOpen).toBe(false);
    expect(state.autoChatCollapsed).toBe(true);

    // Wide: both restore — this is the bug C64 fixes. Before this
    // commit, nothing in the handler ever set `$chatSidebarOpen` back
    // to true on widen; only the sidebar had this restore path.
    state = deriveResponsiveState(1400, state);
    expect(state.sidebarCollapsed).toBe(false);
    expect(state.autoCollapsed).toBe(false);
    expect(state.chatSidebarOpen).toBe(true);
    expect(state.autoChatCollapsed).toBe(false);
  });

  it('a manual close while wide is never force-reopened on a later resize event', () => {
    // User is wide, manually closes chat — in the real component this
    // is a direct $chatSidebarOpen = false with no autoChatCollapsed
    // write at all (no narrow/medium ever happened).
    const manuallyClosedWide: ResponsiveLayoutState = {
      sidebarCollapsed: false,
      autoCollapsed: false,
      chatSidebarOpen: false,
      autoChatCollapsed: false,
    };

    // Any further wide-band resize event (e.g. 1200 -> 1400) must
    // never flip chatSidebarOpen back on, because autoChatCollapsed
    // was never set true for this close.
    const next = deriveResponsiveState(1400, manuallyClosedWide);
    expect(next.chatSidebarOpen).toBe(false);
    expect(next.autoChatCollapsed).toBe(false);
  });

  it('narrowing when chat is already closed does not stamp autoChatCollapsed over a manual close', () => {
    // User manually closed chat, THEN narrowed (manual close site is
    // responsible for leaving autoChatCollapsed false — this asserts
    // the narrow branch itself never second-guesses that).
    const manuallyClosed: ResponsiveLayoutState = {
      sidebarCollapsed: false,
      autoCollapsed: false,
      chatSidebarOpen: false,
      autoChatCollapsed: false,
    };
    const next = deriveResponsiveState(800, manuallyClosed);
    expect(next.chatSidebarOpen).toBe(false);
    expect(next.autoChatCollapsed).toBe(false); // still false — not auto, still manual
  });

  it('sidebar-only parity: the pre-existing autoCollapsed restore still works exactly as before (regression guard)', () => {
    let state: ResponsiveLayoutState = { sidebarCollapsed: false, autoCollapsed: false, chatSidebarOpen: false, autoChatCollapsed: false };
    state = deriveResponsiveState(800, state);
    expect(state.sidebarCollapsed).toBe(true);
    expect(state.autoCollapsed).toBe(true);
    state = deriveResponsiveState(1400, state);
    expect(state.sidebarCollapsed).toBe(false);
    expect(state.autoCollapsed).toBe(false);
  });

  it('medium band (900-1199) never touches chat state at all', () => {
    const state = deriveResponsiveState(1000, {
      sidebarCollapsed: false,
      autoCollapsed: false,
      chatSidebarOpen: true,
      autoChatCollapsed: false,
    });
    expect(state.chatSidebarOpen).toBe(true);
    expect(state.autoChatCollapsed).toBe(false);
  });
});

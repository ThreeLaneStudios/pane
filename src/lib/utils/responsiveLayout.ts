/**
 * responsiveLayout.ts — #3182 C64.
 *
 * `+page.svelte`'s "Responsive breakpoints" `$effect` computes the
 * three width states (wide / medium `<1200` / narrow `<900`) and
 * writes `$sidebarCollapsed`/`autoCollapsed`/`$chatSidebarOpen`/
 * `autoChatCollapsed` from them. That branching is a pure function of
 * (width, current state) — extracted here specifically so it's
 * jsdom-testable (no DOM, no stores, no window) without needing the
 * real-layout WebKit harness this PR's other C64 test uses.
 *
 * `autoCollapsed` (sidebar) and `autoChatCollapsed` (chat) each record
 * ONLY "this breakpoint handler forced this closed," never "the user
 * chose this" — the exact distinction `$chatSidebarOpen` alone used to
 * conflate (C64's root cause). A manual close/open resets its own
 * `auto*Collapsed` flag at the call site (`+page.svelte`'s keyboard
 * shortcut and expand-button handlers) — this function never resets
 * those flags on its own; it only ever SETS one true (when it forces a
 * close) or consumes-and-clears one (when it restores on widen).
 */

export interface ResponsiveLayoutState {
  sidebarCollapsed: boolean;
  autoCollapsed: boolean;
  chatSidebarOpen: boolean;
  autoChatCollapsed: boolean;
}

export function deriveResponsiveState(
  width: number,
  state: ResponsiveLayoutState,
): ResponsiveLayoutState {
  const { sidebarCollapsed, autoCollapsed, chatSidebarOpen, autoChatCollapsed } = state;

  if (width < 900) {
    // Narrow: sidebar collapses, chat closes too — but ONLY mark chat
    // as auto-collapsed (and only close it) if it's currently open.
    // If it's already closed (user's own choice, or a prior narrow
    // pass), a repeated resize event inside the same band must not
    // re-stamp `autoChatCollapsed = true` over a manual close.
    return {
      sidebarCollapsed: true,
      autoCollapsed: true,
      chatSidebarOpen: chatSidebarOpen ? false : chatSidebarOpen,
      autoChatCollapsed: chatSidebarOpen ? true : autoChatCollapsed,
    };
  }

  if (width < 1200) {
    // Medium: sidebar collapsed, main + chat shown — chat is
    // untouched here entirely (C64's three states).
    return { sidebarCollapsed: true, autoCollapsed: true, chatSidebarOpen, autoChatCollapsed };
  }

  // Wide: restore ONLY what was auto-collapsed. A manual close that
  // happened while already wide never set `autoChatCollapsed`/
  // `autoCollapsed`, so it's never force-reopened here.
  return {
    sidebarCollapsed: autoCollapsed ? false : sidebarCollapsed,
    autoCollapsed: autoCollapsed ? false : autoCollapsed,
    chatSidebarOpen: autoChatCollapsed ? true : chatSidebarOpen,
    autoChatCollapsed: autoChatCollapsed ? false : autoChatCollapsed,
  };
}

/**
 * userReachableScan.ts — #3043 item 2
 *
 * Mechanically derives the set of `CanvasViewType` values a REAL user
 * control (a button, menu item, or keyboard shortcut) can open, by
 * scanning the app's own UI source for the call sites that actually set
 * `canvasState`'s `view` — the same "read the real source, extract the
 * literal values" discipline `canvas.showViewParity.test.ts` already
 * applies to `SHOW_VIEW_SCREENS` (tools.rs/canvas.ts), applied here to
 * `USER_REACHABLE_VIEWS` (canvas.ts), which until #3043 was purely
 * hand-maintained — "confirmed by grep/live-testing", Reed's audit noted,
 * but with no mechanical guard. That gap is exactly how `search_results`
 * sat in the list for months with zero real control behind it.
 *
 * WHAT COUNTS: any `navigateTo({ view: 'x', ... })`,
 * `canvasState.set({ view: 'x', ... })`, or
 * `canvasState.update(state => ({ ..., view: 'x', ... }))` call site with
 * a LITERAL `view` string, found in the app's own UI code.
 *
 * WHAT'S EXCLUDED, and why (by construction, not tuned to a target
 * answer):
 * - Any callback passed to `listen('pane-navigate', ...)`. The ONLY
 *   emitter of this Tauri event is the AI backend (`commands.rs`'s
 *   `navigate_home`/`navigate_all_projects`/.../`show_view` dispatch,
 *   confirmed by grep — zero other emit sites exist). A user never fires
 *   this event, so a call site inside this callback is the agent reading
 *   its own echo, not a user control.
 * - `handleNavigateFromStream` (ChatSidebar.svelte) — dispatches whatever
 *   `view` the AI chat stream's `navigate` payload named.
 *   `decideNavigationAction`'s own tests (`navigationDispatch.test.ts`)
 *   already prove that function's decision logic; this scan excludes its
 *   side effects for the same reason as `pane-navigate` above — the
 *   agent being ABLE to request a view is exactly what this list exists
 *   to constrain, not evidence for it.
 * - `handleCanvasCommand` (ChatSidebar.svelte) — the AI canvas-command
 *   dispatcher (confirmed dead, zero callers, by the #3043 delta-2
 *   audit) — agent-origin regardless.
 * - `handleSendMessage`'s `if (navView) { ... }` block (ChatSidebar.svelte)
 *   — REMOVED #3044 item 4: the free-text chat navigation shortcuts this
 *   exclusion existed for ("go to home", "show tasks" parsed via a
 *   `navMap` lookup into a dynamic `navView` variable) were deleted
 *   outright per Matt's ruling ("every chat message goes to the agent") —
 *   there is no longer any such block anywhere in `src/` for this scan to
 *   exclude. Confirmed dead (not merely unused): grepped the real tree for
 *   `navView` post-deletion, zero hits outside this historical note and
 *   the one source-text citation in ChatSidebar.svelte's own deletion
 *   comment. The marker that matched it (`/if\s*\(\s*navView\s*\)\s*\{/g`)
 *   is removed from `EXCLUDED_BLOCK_MARKERS` below in the same commit.
 *
 * NOT scanned at all (whole-file; these are the navigation ENGINE, not
 * call sites — `destination.view`/`fn`/`value` are parameter names, not
 * literal screens, so including them would be a category error, not a
 * finding): `src/lib/services/navigation.ts`,
 * `src/lib/utils/navigationDispatch.ts`.
 *
 * UNRESOLVABLE TARGETS FAIL LOUDLY (#3043 item 3): a matched call whose
 * `view` property is present but is NOT a single string literal (a
 * variable, a ternary, a template expression, a function call) throws
 * `UnresolvableViewTargetError`, naming the file and line — never
 * silently skipped. A scanner that quietly drops what it can't parse
 * degrades into exactly the hand-maintained, unverifiable list it
 * replaces.
 */

export interface ScanFile {
  /** Repo-relative path, used only for citations/error messages. */
  path: string;
  source: string;
}

export interface ViewCitation {
  path: string;
  line: number;
}

export interface ScanResult {
  /** Every distinct literal view name a real navigation call site sets. */
  views: Set<string>;
  /** view name -> every file:line call site that set it. */
  citations: Map<string, ViewCitation[]>;
}

export class UnresolvableViewTargetError extends Error {
  constructor(path: string, line: number, snippet: string) {
    super(
      `Unresolvable navigation target at ${path}:${line} — "view" is not a literal string ` +
        `(found: ${snippet}). The reachable-view scan must fail loudly on what it can't ` +
        `statically resolve, never skip it silently.`,
    );
    this.name = 'UnresolvableViewTargetError';
  }
}

/**
 * Markers whose enclosing `{ ... }` block is excluded from the scan — the
 * agent's own navigation-dispatch code. (A fourth marker,
 * `/if\s*\(\s*navView\s*\)\s*\{/g`, used to live here — the free-text chat
 * shortcut block it matched was deleted outright in #3044 item 4, not
 * merely gated, so the marker is removed rather than kept around matching
 * nothing. See this file's header comment.) Order doesn't matter; each is
 * matched independently and its own brace-balanced body is carved out.
 */
const EXCLUDED_BLOCK_MARKERS: RegExp[] = [
  /listen\(\s*['"]pane-navigate['"]/g,
  /(?:async\s+)?function\s+handleNavigateFromStream\s*\([^)]*\)\s*\{/g,
  /(?:async\s+)?function\s+handleCanvasCommand\s*\([^)]*\)\s*\{/g,
];

/** Blanks out `//` and `/* *\/` comments (preserving line breaks/length) so comment TEXT can never be mistaken for a real call site. String/template contents are left verbatim. */
export function stripComments(source: string): string {
  const out: string[] = [];
  let i = 0;
  const n = source.length;
  while (i < n) {
    const c = source[i];
    if (c === "'" || c === '"' || c === '`') {
      const quote = c;
      out.push(c);
      i++;
      while (i < n) {
        if (source[i] === '\\') {
          out.push(source[i], source[i + 1] ?? '');
          i += 2;
          continue;
        }
        out.push(source[i]);
        const closed = source[i] === quote;
        i++;
        if (closed) break;
      }
    } else if (c === '/' && source[i + 1] === '/') {
      while (i < n && source[i] !== '\n') {
        out.push(' ');
        i++;
      }
    } else if (c === '/' && source[i + 1] === '*') {
      out.push(' ', ' ');
      i += 2;
      while (i < n && !(source[i] === '*' && source[i + 1] === '/')) {
        out.push(source[i] === '\n' ? '\n' : ' ');
        i++;
      }
      if (i < n) {
        out.push(' ', ' ');
        i += 2;
      }
    } else {
      out.push(c);
      i++;
    }
  }
  return out.join('');
}

/** Finds the index just past the `}` matching the `{` at `openBraceIndex`, skipping over string/template literals (so a stray brace inside one can't desync the count). `source` is assumed comment-stripped already. */
function findMatchingBraceEnd(source: string, openBraceIndex: number): number {
  return findMatchingDelimEnd(source, openBraceIndex, '{', '}');
}

function findMatchingParenEnd(source: string, openParenIndex: number): number {
  return findMatchingDelimEnd(source, openParenIndex, '(', ')');
}

function findMatchingDelimEnd(source: string, openIndex: number, open: string, close: string): number {
  let depth = 0;
  let i = openIndex;
  const n = source.length;
  while (i < n) {
    const c = source[i];
    if (c === open) {
      depth++;
      i++;
    } else if (c === close) {
      depth--;
      i++;
      if (depth === 0) return i;
    } else if (c === "'" || c === '"' || c === '`') {
      const quote = c;
      i++;
      while (i < n) {
        if (source[i] === '\\') {
          i += 2;
          continue;
        }
        if (source[i] === quote) {
          i++;
          break;
        }
        if (quote === '`' && source[i] === '$' && source[i + 1] === '{') {
          i += 2;
          let exprDepth = 1;
          while (i < n && exprDepth > 0) {
            if (source[i] === '{') exprDepth++;
            else if (source[i] === '}') exprDepth--;
            i++;
          }
          continue;
        }
        i++;
      }
    } else {
      i++;
    }
  }
  return n; // Unbalanced — treat as "rest of file" rather than crash the scan.
}

function lineAt(source: string, index: number): number {
  let line = 1;
  for (let i = 0; i < index && i < source.length; i++) {
    if (source[i] === '\n') line++;
  }
  return line;
}

function findExcludedRanges(source: string): Array<[number, number]> {
  const ranges: Array<[number, number]> = [];
  for (const markerRe of EXCLUDED_BLOCK_MARKERS) {
    const re = new RegExp(markerRe.source, markerRe.flags);
    let m: RegExpExecArray | null;
    while ((m = re.exec(source)) !== null) {
      const openBrace = source.indexOf('{', m.index);
      if (openBrace === -1) continue;
      const end = findMatchingBraceEnd(source, openBrace);
      ranges.push([m.index, end]);
    }
  }
  return ranges;
}

function isExcluded(index: number, ranges: Array<[number, number]>): boolean {
  return ranges.some(([start, end]) => index >= start && index < end);
}

const CALL_RE = /\b(navigateTo|canvasState\.set|canvasState\.update)\s*\(/g;
const VIEW_PROP_RE = /\bview\s*:\s*/;
const STRING_LITERAL_RE = /^(['"])((?:\\.|(?!\1).)*)\1/;

/**
 * Scans the given files for real navigation call sites and returns the
 * derived set of user-reachable views, with citations. Throws
 * `UnresolvableViewTargetError` (naming file:line) the first time it
 * finds a call whose `view` property isn't a literal string.
 */
export function scanUserReachableViews(files: ScanFile[]): ScanResult {
  const views = new Set<string>();
  const citations = new Map<string, ViewCitation[]>();

  for (const { path, source: rawSource } of files) {
    const source = stripComments(rawSource);
    const excludedRanges = findExcludedRanges(source);
    const re = new RegExp(CALL_RE.source, CALL_RE.flags);
    let m: RegExpExecArray | null;
    while ((m = re.exec(source)) !== null) {
      const callStart = m.index;
      if (isExcluded(callStart, excludedRanges)) continue;

      const openParen = source.indexOf('(', callStart);
      const callEnd = findMatchingParenEnd(source, openParen);
      const callArgs = source.slice(openParen, callEnd);

      const viewMatch = VIEW_PROP_RE.exec(callArgs);
      if (!viewMatch) continue; // Not every canvasState.update()/set() call touches `view`.

      const afterColon = callArgs.slice(viewMatch.index + viewMatch[0].length);
      const literalMatch = STRING_LITERAL_RE.exec(afterColon);
      const line = lineAt(source, callStart);
      if (!literalMatch) {
        const snippet = afterColon.slice(0, 40).replace(/\s+/g, ' ').trim();
        throw new UnresolvableViewTargetError(path, line, snippet);
      }

      const view = literalMatch[2];
      views.add(view);
      const list = citations.get(view) ?? [];
      list.push({ path, line });
      citations.set(view, list);
    }
  }

  return { views, citations };
}

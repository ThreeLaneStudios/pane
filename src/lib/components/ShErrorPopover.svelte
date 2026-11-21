<script lang="ts">
  // ShErrorPopover.svelte — the error detail popover shown when a
  // formula-error cell (`cellKind` === `'err'`) is the sole selection
  // anchor (#2339 PR 15b-alpha, §2c). Mounted by `SheetGrid.svelte` as a
  // CHILD of the selected error cell's own `.sh-c` div (same pattern
  // `.sh-mis-tip` already uses one file over) and only while that
  // condition holds — dismissed the instant the cell is no longer the
  // anchor purely by SheetGrid's own `{#if}` unmounting it, so this
  // component has no dismiss lifecycle of its own.
  //
  // Three parts, per the design's Cap prose ("the code, one plain
  // sentence, and the raw formula. No error numbers, no docs links"):
  //   1. the error code, mono red (`.sh-errpop__code`)
  //   2. one plain sentence (`.sh-errpop__msg`)
  //   3. the raw formula, mono dim (`.sh-errpop__f`)
  //
  // Hard constraint (task brief, reiterated over the design mockup's own
  // illustrated example): NO culprit-cell naming. The design's one-off
  // `#VALUE!` mockup appends "**B2** holds text" to the sentence — this
  // component never does that; `sheetErrors.ts`'s catalogue is
  // deliberately culprit-free (see that file's own doc comment).
  //
  // Content is looked up from the STATIC six-message catalogue
  // (`sheetErrors.ts`, ported from the design's own `ERRS` array) keyed
  // by the cell's `computed_value` error code — not a live per-cell
  // diagnostic (the frontend has no per-error culprit signal from the
  // backend; inventing one would be a backend change, out of scope here).
  // An error code the catalogue doesn't recognize (defensive — every code
  // the Rust evaluator currently produces is one of the six) renders
  // nothing rather than guessing.
  //
  // Positioning: boundary-flip pattern copied from `ShHeaderMenu.svelte`
  // (measure the anchor rect once per mount/resize, flip `left: 0` →
  // `right: 0` if the popover's fixed width would overflow the
  // viewport's right edge). Since this component always mounts as a
  // DIRECT CHILD of the cell it details (its own positioned ancestor,
  // `position: relative` per `.sh-c`), it self-measures off
  // `el.parentElement` rather than taking an `anchorEl` prop — one less
  // thing the caller has to plumb through, and the measured element is
  // always correct by construction (there is no other possible parent).
  // Vertical placement is always `top: 100%` ("detail card UNDER it",
  // design prose) — no vertical flip; the design gives no bottom-edge
  // treatment and a selected cell is always well above the viewport's
  // own bottom edge in every realistic grid height.

  import { getErrorInfo } from '$lib/utils/sheetErrors';

  interface Props {
    /** The error code as stored in the cell's `computed_value` (e.g.
     *  `'#VALUE!'`). */
    errorCode: string;
    /** The cell's raw formula text (`raw_value`) — shown verbatim as the
     *  "formula stays visible" part, per design prose. Falls back to the
     *  catalogue's own example formula only if the live cell somehow has
     *  no raw_value (defensive; every error cell is a formula cell by
     *  construction, so this path isn't expected to be hit). */
    rawFormula: string;
  }

  let { errorCode, rawFormula }: Props = $props();

  const POPOVER_WIDTH = 252;
  let rootEl: HTMLDivElement | null = $state(null);
  let flipRight = $state(false);

  $effect(() => {
    const parent = rootEl?.parentElement;
    if (parent && typeof parent.getBoundingClientRect === 'function') {
      const rect = parent.getBoundingClientRect();
      const viewportWidth = typeof window !== 'undefined' && (window as any).innerWidth ? (window as any).innerWidth : Infinity;
      flipRight = rect.left + POPOVER_WIDTH > viewportWidth;
    }
  });

  let info = $derived(getErrorInfo(errorCode));
</script>

{#if info}
  <div
    bind:this={rootEl}
    class="sh-errpop"
    class:flip-right={flipRight}
    role="tooltip"
    data-testid="sheet-error-popover"
  >
    <div class="sh-errpop__code" data-testid="sheet-error-popover-code">{info.code}</div>
    <div class="sh-errpop__msg" data-testid="sheet-error-popover-msg">{info.message}</div>
    <div class="sh-errpop__f" data-testid="sheet-error-popover-formula">{rawFormula || info.formula}</div>
  </div>
{/if}

<style>
  /* Ported verbatim (class names + rules) from
     docs/design/sheets/sheets.css L225-228 (`.sh-errpop*`) — this is its
     own distinct shell (red-tinted border), NOT the shared `.sh-pop`
     class, matching the design's own separate class name. Positioning
     (`top`/`left`/`flip-right`) is this component's own addition (design
     shows only a fixed illustrative placement via its `<At>` helper),
     following the exact same boundary-flip mechanism as
     `ShHeaderMenu.svelte`'s `.sh-pop.flip-right`. */
  .sh-errpop { position: absolute; z-index: 30; width: 252px; background: #1c1c20; border: 1px solid rgba(239, 68, 68, 0.4); border-radius: 10px; box-shadow: var(--shadow-pop); padding: 11px 12px; box-sizing: border-box; top: 100%; left: 0; margin-top: 4px; }
  .sh-errpop.flip-right { left: auto; right: 0; }
  .sh-errpop__code { font-family: var(--font-mono); font-size: 12px; font-weight: 500; color: var(--status-danger); margin-bottom: 5px; }
  .sh-errpop__msg { font-size: 12.5px; color: var(--fg-2); line-height: 1.45; }
  .sh-errpop__f { margin-top: 9px; padding-top: 9px; border-top: 1px solid var(--border-faint); font-family: var(--font-mono); font-size: 11.5px; color: var(--fg-3); word-break: break-all; }
</style>

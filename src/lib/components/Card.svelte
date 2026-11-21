<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import type { CardData } from '$lib/utils/cardNormalizers';
  import { getStageColor, getStageLabel } from '$lib/utils/stageColors';
  import { resolveSheetCardMode, type SheetCardMode } from '$lib/utils/sheetCardLayout';

  interface Props {
    data: CardData;
    active?: boolean;
    onSelect?: (id: string) => void;
    onContextMenu?: (event: MouseEvent, id: string) => void;
    onDragStart?: (id: string) => void;
  }

  let { data, active = false, onSelect, onContextMenu, onDragStart }: Props = $props();

  // #2339 PR 15a: sheet card face. `isSheet` gates a completely separate
  // body/pill/footer render branch below — the non-sheet markup path
  // (title/card-body/card-pills/breadcrumb) is UNTOUCHED by any of this,
  // same DOM, same classes, same conditionals as before this PR.
  const isSheet = $derived(data.noteType === 'sheet');

  // Compact-variant breakpoint (README §4 / build constraint §1: "below
  // ~190px of card width the data rows drop and the card is glyph plus
  // header strip"). Keyed to THIS card's own rendered width via
  // ResizeObserver — same width-not-viewport convention newSheetButtonLayout.ts
  // established for the New Sheet button's header-width breakpoint. Only
  // ever attached/read for sheet cards; non-sheet cards never mount the
  // observer (see onMount below), so this is zero-cost for the note-card path.
  let cardEl: HTMLDivElement | undefined = $state(undefined);
  let cardWidth = $state(280); // start wide (a board column's min width) to avoid a compact-mode flash
  let cardResizeObs: ResizeObserver | null = null;
  let sheetCardMode: SheetCardMode = $derived(resolveSheetCardMode(cardWidth));

  onMount(() => {
    if (!isSheet || !cardEl) return;
    cardResizeObs = new ResizeObserver(entries => {
      for (const entry of entries) {
        cardWidth = entry.contentRect.width;
      }
    });
    cardResizeObs.observe(cardEl);
  });

  onDestroy(() => {
    cardResizeObs?.disconnect();
  });

  /** Micro-grid header-row glyph per column type (#2339 PR 15a). Local,
   *  deliberately NOT imported from SheetGrid.svelte's own typeGlyph() —
   *  the card micro-grid and the grid editor are independent surfaces (see
   *  PR body "seams" note: no shared component between the card face and
   *  the sheet grid/version-history preview), so this is a small
   *  intentional duplication of the same 6-case mapping, not a missed
   *  reuse opportunity. */
  function sheetTypeGlyph(colType: string): string {
    switch (colType) {
      case 'text': return 'T';
      case 'number': return '#';
      case 'currency': return '$';
      case 'date': return 'D';
      case 'checkbox': return '☑';
      case 'select': return '≡';
      default: return '';
    }
  }

  function getPriorityLabel(priority: string | undefined): string {
    switch (priority) {
      case 'urgent': return 'URGENT';
      case 'high': return 'HIGH';
      case 'medium': return 'MEDIUM';
      case 'low': return 'LOW';
      default: return '';
    }
  }

  function getPriorityBg(priority: string | undefined): string {
    switch (priority) {
      case 'urgent': return 'rgba(255,68,68,0.2)';
      case 'high': return 'rgba(255,140,0,0.2)';
      case 'medium': return 'rgba(0,228,228,0.15)';
      case 'low': return 'rgba(255,255,255,0.1)';
      default: return 'rgba(255,255,255,0.1)';
    }
  }

  function getPriorityFg(priority: string | undefined): string {
    switch (priority) {
      case 'urgent': return '#FF4444';
      case 'high': return '#FF8C00';
      case 'medium': return '#00E4E4';
      case 'low': return 'rgba(255,255,255,0.3)';
      default: return 'rgba(255,255,255,0.3)';
    }
  }

  function formatDueDate(dateStr: string | undefined): string {
    if (!dateStr) return '';
    try {
      // Parse as LOCAL time to avoid off-by-one near midnight
      const date = new Date(dateStr + 'T00:00:00');
      const month = date.getMonth() + 1;
      const day = date.getDate();
      const year = date.getFullYear();
      return `${month}/${day}/${year}`;
    } catch { return ''; }
  }

  function isOverdue(dateStr: string | undefined): boolean {
    if (!dateStr) return false;
    try {
      const dueDate = new Date(dateStr + 'T00:00:00');
      const today = new Date();
      today.setHours(0, 0, 0, 0);
      return dueDate.getTime() < today.getTime();
    } catch { return false; }
  }

  function handleClick() {
    if (onSelect) onSelect(data.id);
  }

  function handleContextMenu(e: MouseEvent) {
    e.preventDefault();
    if (onContextMenu) onContextMenu(e, data.id);
  }

  function handleDragStart(e: DragEvent) {
    e.dataTransfer!.effectAllowed = 'move';
    e.dataTransfer!.setData('text/plain', data.id);
    if (onDragStart) onDragStart(data.id);
  }

  // Derived display values
  const stageLabel = $derived(getStageLabel(data.status));
  const stageColor = $derived(getStageColor(data.status));
  const dueDateText = $derived(formatDueDate(data.dueDate));
  const overdue = $derived(isOverdue(data.dueDate));

  const breadcrumb = $derived.by(() => {
    const parts: string[] = [];
    if (data.projectName) parts.push(data.projectName);
    if (data.folderName) parts.push(data.folderName);
    return parts;
  });
</script>

<div
  class="pane-card"
  class:active
  class:sheet-card={isSheet}
  style:border-left-color={stageColor}
  draggable={!!onDragStart}
  ondragstart={handleDragStart}
  onclick={handleClick}
  oncontextmenu={handleContextMenu}
  role="button"
  tabindex="0"
  onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') handleClick(); }}
  bind:this={cardEl}
>
  {#if isSheet}
    <!-- #2339 PR 15a: sheet card face — in-place branch inside Card.svelte
         (per Reed's scope §1.3.1: preserves the outer .pane-card shell,
         swaps only the body region, avoids drifting from the shared
         height/padding contract a sibling component would risk). Non-sheet
         markup lives entirely in the {:else} branch below, untouched. -->
    <div class="sh-card__head">
      <span class="sh-card__glyph" aria-hidden="true">
        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <rect x="3" y="3" width="7" height="7"/><rect x="14" y="3" width="7" height="7"/><rect x="14" y="14" width="7" height="7"/><rect x="3" y="14" width="7" height="7"/>
        </svg>
      </span>
      <h4 class="card-title sh-card__title">{data.title || 'Untitled'}</h4>
    </div>

    <!-- Micro-grid: type-glyph header row always present (drives from REAL
         column config, data.colTypes — never a fixed mock). Two data rows
         (revised down from three per README "Revisions after review"),
         hidden below the ~190px compact threshold. Grid template reflects
         the sheet's ACTUAL column count — not the design's illustrative
         4-column 1.6fr/1fr/1fr/0.7fr mock. -->
    <div class="sh-mini">
      <div class="sh-mini__h" style:grid-template-columns={`repeat(${Math.max(1, data.colTypes?.length ?? 0)}, 1fr)`}>
        {#each (data.colTypes ?? []) as colType, ci (ci)}
          <span class="sh-mini__hc">{sheetTypeGlyph(colType)}</span>
        {/each}
      </div>
      {#if sheetCardMode === 'full'}
        {#each [0, 1] as r (r)}
          <div class="sh-mini__r" style:grid-template-columns={`repeat(${Math.max(1, data.colTypes?.length ?? 0)}, 1fr)`}>
            {#each (data.colTypes ?? []) as colType, ci (ci)}
              <span class="sh-mini__c">
                <span
                  class="sh-mini__bar"
                  class:sh-mini__bar--calc={colType === 'currency' && (data.formulaCount ?? 0) > 0}
                  style:width={r === 0 ? '78%' : '58%'}
                ></span>
              </span>
            {/each}
          </div>
        {/each}
      {/if}
    </div>
  {:else}
    <!-- Title: 1 line, truncate -->
    <div class="card-title">{data.title || 'Untitled'}</div>

    <!-- Body: 3-line clamp; always rendered so layout stays uniform -->
    <div class="card-body">{data.bodyText || ''}</div>
  {/if}

  <!-- Pill row: standard task metadata, UNCHANGED between sheet and note
       cards — same stage dot/label, priority pill, due-date pill, same
       order, same markup, for both branches (README §4: "Metadata is the
       kit's dot + Pill in the note card's order"). -->
  <div class="card-pills">
    <!-- Stage dot + label -->
    <span class="pill-stage">
      <span class="stage-dot" style:background={stageColor}></span>
      <span class="stage-label">{stageLabel}</span>
    </span>

    <!-- Priority pill -->
    {#if data.priority}
      <span class="pill-priority" style="background: {getPriorityBg(data.priority)}; color: {getPriorityFg(data.priority)}">
        {getPriorityLabel(data.priority)}
      </span>
    {/if}

    <!-- Due date pill -->
    {#if dueDateText}
      <span class="pill-due" class:overdue class:sh-due={isSheet}>{dueDateText}</span>
    {/if}
  </div>

  {#if isSheet}
    <!-- Footer metric: cols × rows + computed count, then breadcrumb —
         same footer position/order as the note card's breadcrumb-only
         footer below. An empty sheet reads "N × 0", not a special empty
         state (README Cap prose). -->
    <div class="sh-card__foot">
      <b>{data.colCount ?? 0} × {data.rowCount ?? 0}</b>
      {#if (data.formulaCount ?? 0) > 0}
        <span class="sh-card__calc">· {data.formulaCount} computed</span>
      {/if}
      {#if breadcrumb.length > 0}
        <span class="sh-card__crumb">
          {#each breadcrumb as segment, i}
            {#if i > 0}<span class="breadcrumb-sep">›</span>{/if}
            <span class="breadcrumb-segment">{segment}</span>
          {/each}
        </span>
      {/if}
    </div>
  {:else}
    <!-- Breadcrumb: project › folder -->
    {#if breadcrumb.length > 0}
      <div class="card-breadcrumb">
        {#each breadcrumb as segment, i}
          {#if i > 0}<span class="breadcrumb-sep">›</span>{/if}
          <span class="breadcrumb-segment">{segment}</span>
        {/each}
      </div>
    {/if}
  {/if}
</div>

<style>
  .pane-card {
    position: relative;
    background: rgba(255,255,255,0.05);
    border: 1px solid rgba(255,255,255,0.06);
    border-left: 3px solid transparent;
    border-radius: 4px;
    cursor: pointer;
    transition: all 0.15s ease;
    overflow: hidden;
    padding: 10px 10px 8px 10px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    user-select: none;
    height: 130px;
    min-height: 130px;
    max-height: 130px;
  }

  .pane-card:hover {
    background: rgba(255,255,255,0.07);
    border-color: rgba(255,255,255,0.12);
    border-left-color: inherit;
  }

  .pane-card.active {
    background: rgba(0,228,228,0.06);
  }

  /* #2339 PR 15a: HEIGHT ONLY revision for sheet cards — two micro-grid data
     rows need a few more px than the note card's fixed 130px. Per build
     constraint §1 ("do not change board column or card widths"), NOTHING
     else about .pane-card changes for sheet cards: width, padding, radius,
     border, hover/active backgrounds are all inherited unmodified from the
     base .pane-card rule above. min-height/max-height are intentionally NOT
     overridden to a new fixed value — auto height lets the compact variant
     (glyph header only, no data rows) size shorter than the full variant
     without either one needing its own separate rule. */
  .pane-card.sheet-card {
    height: auto;
    min-height: 130px;
    max-height: none;
  }

  .card-title {
    font-size: 13px;
    font-weight: 600;
    color: #fff;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }

  .card-body {
    font-size: 11px;
    color: rgba(255,255,255,0.4);
    line-height: 1.4;
    display: -webkit-box;
    -webkit-line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
    max-height: calc(11px * 1.4 * 3); /* 46.2px = exactly 3 lines; NO flex:1 */
  }

  .card-pills {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: nowrap;
    overflow: hidden;
    margin-top: 2px;
  }

  .pill-stage {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  .stage-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .stage-label {
    font-size: 10px;
    color: rgba(255,255,255,0.4);
    white-space: nowrap;
  }

  .pill-priority {
    font-size: 9px;
    font-weight: 600;
    padding: 1px 5px;
    border-radius: 3px;
    white-space: nowrap;
    flex-shrink: 0;
    text-transform: uppercase;
    letter-spacing: 0.3px;
  }

  .pill-due {
    font-size: 10px;
    color: rgba(255,255,255,0.35);
    white-space: nowrap;
    flex-shrink: 0;
  }

  .pill-due.overdue {
    background: rgba(226,75,74,0.2);
    color: #E24B4A;
    padding: 0 4px;
    border-radius: 3px;
  }

  /* #2339 PR 15a: due-date chip adds a tone the kit's pill set doesn't
     carry (README §4) — ported from docs/design/sheets/sheets.css's
     .sh-due, remapped onto Pane's actual token names. Only applies when
     the card is a sheet card AND not overdue (.overdue's red treatment
     above still wins for an overdue sheet due-date — overdue is a more
     urgent signal than "this happens to be a sheet"). */
  .pill-due.sh-due:not(.overdue) {
    background: rgba(245,184,36,0.14);
    color: #F5B824;
    border: 1px solid rgba(245,184,36,0.28);
    padding: 0 4px;
    border-radius: 3px;
    font-variant-numeric: tabular-nums;
  }

  /* ===== Sheet card face (#2339 PR 15a) =====
     Ported from docs/design/sheets/sheets.css's .sh-card and .sh-mini classes,
     remapped from the design's kit token names onto Pane's actual CSS custom
     properties (var(--accent), var(--surface-2), var(--border-faint),
     var(--fg-3), var(--fg-4), var(--font-mono) — all confirmed present in
     src/app.css). Scoped entirely inside this component's <style> block, same
     as every other rule in this file — no new global stylesheet. */

  .sh-card__head {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }

  .sh-card__glyph {
    width: 20px;
    height: 20px;
    border-radius: 5px;
    background: var(--accent-soft);
    color: var(--accent);
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }

  .sh-card__title {
    /* Inherits .card-title's font-size/weight/color/truncation (shared
       class, applied alongside sh-card__title in the template) — this rule
       only resets the <h4> tag's default margin so it doesn't add spacing
       .card-title (a <div>) never had. */
    margin: 0;
    flex: 1;
    min-width: 0;
  }

  .sh-mini {
    border: 1px solid var(--border-faint);
    border-radius: 5px;
    overflow: hidden;
    margin-top: 2px;
  }

  .sh-mini__h {
    display: grid;
    background: var(--surface-2);
    border-bottom: 1px solid var(--border-faint);
  }

  .sh-mini__hc {
    height: 16px;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--fg-4);
    border-right: 1px solid var(--border-faint);
    font-size: 9px;
    font-family: var(--font-mono);
  }

  .sh-mini__hc:last-child {
    border-right: 0;
  }

  .sh-mini__r {
    display: grid;
    border-bottom: 1px solid var(--border-faint);
  }

  .sh-mini__r:last-child {
    border-bottom: 0;
  }

  .sh-mini__c {
    height: 15px;
    display: flex;
    align-items: center;
    padding: 0 5px;
    border-right: 1px solid var(--border-faint);
  }

  .sh-mini__c:last-child {
    border-right: 0;
  }

  .sh-mini__bar {
    height: 4px;
    border-radius: 2px;
    background: var(--surface-3);
    display: block;
  }

  .sh-mini__bar--calc {
    background: rgba(0,228,228,0.35);
  }

  .sh-card__foot {
    display: flex;
    align-items: center;
    gap: 8px;
    padding-top: 6px;
    margin-top: auto;
    border-top: 1px solid var(--border-faint);
    font-size: 11px;
    color: var(--fg-4);
    min-width: 0;
  }

  .sh-card__foot b {
    white-space: nowrap;
    flex-shrink: 0;
    font-family: var(--font-mono);
    font-size: 10.5px;
    color: var(--fg-3);
    font-weight: 400;
    font-variant-numeric: tabular-nums;
  }

  .sh-card__calc {
    white-space: nowrap;
    flex-shrink: 0;
  }

  .sh-card__crumb {
    margin-left: auto;
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .card-breadcrumb {
    display: flex;
    align-items: center;
    gap: 4px;
    overflow: hidden;
    margin-top: auto;
  }

  .breadcrumb-sep {
    font-size: 10px;
    color: rgba(255,255,255,0.15);
    flex-shrink: 0;
  }

  .breadcrumb-segment {
    font-size: 10px;
    color: rgba(255,255,255,0.25);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }

  .hidden-spacer { visibility: hidden; }
</style>

<script lang="ts">
  // ShSrcLabel.svelte — the version-history source chip (#2339 PR 15b-beta,
  // `docs/design/sheets/sheets-surfaces.jsx:136-149` `SrcLabel`,
  // `sheets.css:294-299`). Pure presentational wrapper around
  // `sheetSrcLabel.ts`'s `mapSheetVersionSource` — all four variants
  // (sheet/agent/restore/manual) render through the exact same markup,
  // differing only by the mapped CSS class/glyph/label.

  import { mapSheetVersionSource } from '$lib/utils/sheetSrcLabel';

  interface Props {
    /** Raw `note_versions.source` string. */
    source: string;
    /** Optional detail suffix (design: cell-count for sheet edits, agent
     *  name for agent edits). No current backend data source populates
     *  this (see `sheetSrcLabel.ts`'s own doc comment) — omitted/undefined
     *  renders no `· detail` suffix at all, never a fabricated placeholder. */
    n?: string;
  }

  let { source, n }: Props = $props();
  let info = $derived(mapSheetVersionSource(source));
</script>

<span class={'sh-src ' + info.cssClass} data-testid="sheet-src-label" data-kind={info.kind}>
  <span class="sh-src__ico" aria-hidden="true">{info.glyph}</span>
  {info.label}{#if n}<span class="sh-src__n">· {n}</span>{/if}
</span>

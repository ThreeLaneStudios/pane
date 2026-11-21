// sheetSrcLabel.test.ts — unit tests for the version-history source-label
// mapping (#2339 PR 15b-beta, `docs/design/sheets/sheets-surfaces.jsx:
// 129-160`, `sheets.css:294-299`).

import { describe, it, expect } from 'vitest';
import { mapSheetVersionSource, sheetSrcLabelDetail } from './sheetSrcLabel';

describe('mapSheetVersionSource', () => {
  it('maps "sheet_snapshot" (the real backend source for a grid mutation) to the sheet/cyan/grid variant', () => {
    const info = mapSheetVersionSource('sheet_snapshot');
    expect(info.kind).toBe('sheet');
    expect(info.cssClass).toBe('sh-src--sheet');
    expect(info.label).toBe('Sheet edit');
    expect(info.glyph).toBeTruthy();
  });

  it('maps "pre_restore" (the real backend source for the guaranteed pre-restore capture) to the restore/amber variant', () => {
    const info = mapSheetVersionSource('pre_restore');
    expect(info.kind).toBe('restore');
    expect(info.cssClass).toBe('sh-src--restore');
    expect(info.label).toBe('Before restore');
  });

  it('maps an agent-shaped source tag to the agent/violet variant (forward-compatible — not produced by the backend today)', () => {
    for (const tag of ['agent_edit', 'ai_edit', 'agent']) {
      const info = mapSheetVersionSource(tag);
      expect(info.kind).toBe('agent');
      expect(info.cssClass).toBe('sh-src--agent');
      expect(info.label).toBe('Agent');
    }
  });

  it('falls back to the neutral manual/no-modifier-class variant for any other source, including unknown values', () => {
    for (const tag of ['manual_edit', 'something_unrecognized', '']) {
      const info = mapSheetVersionSource(tag);
      expect(info.kind).toBe('manual');
      expect(info.cssClass).toBe('');
      expect(info.label).toBe('Manual edit');
    }
  });

  it('every one of the four variants has a distinct, truthy glyph and label', () => {
    const kinds = ['sheet_snapshot', 'pre_restore', 'agent_edit', 'manual_edit'].map(mapSheetVersionSource);
    const glyphs = new Set(kinds.map((k) => k.glyph));
    const labels = new Set(kinds.map((k) => k.label));
    expect(glyphs.size).toBe(4);
    expect(labels.size).toBe(4);
    for (const k of kinds) {
      expect(k.glyph.length).toBeGreaterThan(0);
      expect(k.label.length).toBeGreaterThan(0);
    }
  });
});

describe('sheetSrcLabelDetail', () => {
  it('returns null for every kind (no diff/change-count data source exists yet)', () => {
    expect(sheetSrcLabelDetail('sheet')).toBeNull();
    expect(sheetSrcLabelDetail('agent')).toBeNull();
    expect(sheetSrcLabelDetail('restore')).toBeNull();
    expect(sheetSrcLabelDetail('manual')).toBeNull();
  });
});

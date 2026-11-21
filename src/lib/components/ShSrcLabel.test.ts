// ShSrcLabel.test.ts — component tests for the version-history source
// chip's four variants (#2339 PR 15b-beta).

import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/svelte/svelte5';
import ShSrcLabel from './ShSrcLabel.svelte';

describe('ShSrcLabel', () => {
  it('renders the sheet variant (cyan) for source="sheet_snapshot"', () => {
    render(ShSrcLabel, { props: { source: 'sheet_snapshot' } });
    const el = screen.getByTestId('sheet-src-label');
    expect(el.getAttribute('data-kind')).toBe('sheet');
    expect(el.className).toContain('sh-src--sheet');
    expect(el.textContent).toContain('Sheet edit');
  });

  it('renders the restore variant (amber) for source="pre_restore"', () => {
    render(ShSrcLabel, { props: { source: 'pre_restore' } });
    const el = screen.getByTestId('sheet-src-label');
    expect(el.getAttribute('data-kind')).toBe('restore');
    expect(el.className).toContain('sh-src--restore');
    expect(el.textContent).toContain('Before restore');
  });

  it('renders the agent variant (violet) for an agent-shaped source tag', () => {
    render(ShSrcLabel, { props: { source: 'agent_edit' } });
    const el = screen.getByTestId('sheet-src-label');
    expect(el.getAttribute('data-kind')).toBe('agent');
    expect(el.className).toContain('sh-src--agent');
    expect(el.textContent).toContain('Agent');
  });

  it('renders the manual variant (neutral, no modifier class) for any other source', () => {
    render(ShSrcLabel, { props: { source: 'manual_edit' } });
    const el = screen.getByTestId('sheet-src-label');
    expect(el.getAttribute('data-kind')).toBe('manual');
    expect(el.className).not.toContain('sh-src--');
    expect(el.textContent).toContain('Manual edit');
  });

  it('renders the optional detail suffix when n is provided', () => {
    render(ShSrcLabel, { props: { source: 'sheet_snapshot', n: '12 cells' } });
    expect(screen.getByTestId('sheet-src-label').textContent).toContain('12 cells');
  });

  it('renders no "· " suffix when n is omitted', () => {
    render(ShSrcLabel, { props: { source: 'pre_restore' } });
    expect(screen.getByTestId('sheet-src-label').textContent).not.toContain('·');
  });
});

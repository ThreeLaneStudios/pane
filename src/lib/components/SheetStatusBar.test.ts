// SheetStatusBar.test.ts — #2339 PR 14 (Matt ruling 2026-09-05 #4):
// formulas/paused wiring.
import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/svelte/svelte5';
import SheetStatusBar from './SheetStatusBar.svelte';

describe('SheetStatusBar', () => {
  it('renders cols/rows counts with no formula segment when formulas is omitted', () => {
    render(SheetStatusBar, { props: { cols: 4, rows: 10 } });
    expect(screen.getByTestId('sheet-status-bar').textContent).toContain('4');
    expect(screen.getByTestId('sheet-status-bar').textContent).toContain('10');
    expect(screen.queryByText(/computed/)).toBeNull();
  });

  it('renders the computed-count segment when formulas is supplied', () => {
    render(SheetStatusBar, { props: { cols: 4, rows: 10, formulas: 6 } });
    expect(screen.getByTestId('sheet-status-bar').textContent).toContain('6');
    expect(screen.getByTestId('sheet-status-bar').textContent).toContain('computed');
  });

  it('renders "computation paused" when paused is true', () => {
    render(SheetStatusBar, { props: { cols: 4, rows: 600, formulas: 512, paused: true } });
    expect(screen.getByTestId('sheet-status-paused')).toBeTruthy();
    expect(screen.getByTestId('sheet-status-paused').textContent).toContain('computation paused');
  });

  it('does not render the paused segment when paused is false/omitted', () => {
    render(SheetStatusBar, { props: { cols: 4, rows: 10, formulas: 6 } });
    expect(screen.queryByTestId('sheet-status-paused')).toBeNull();
  });

  it('renders formulas AND paused together (ShPaused frame)', () => {
    render(SheetStatusBar, { props: { cols: 5, rows: 512, formulas: 512, paused: true } });
    expect(screen.getByTestId('sheet-status-bar').textContent).toContain('512');
    expect(screen.getByTestId('sheet-status-paused')).toBeTruthy();
  });
});

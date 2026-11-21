// ShErrorPopover.test.ts — component tests for the error detail popover
// (#2339 PR 15b-alpha, §2c).

import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/svelte/svelte5';
import ShErrorPopover from './ShErrorPopover.svelte';

// vitest-setup.ts replaces `globalThis.window` with a plain mock object —
// this component reads `window.innerWidth` in an `$effect`, same pattern
// as ShHeaderMenu.svelte's own test file.
if (typeof (globalThis.window as any).innerWidth !== 'number') {
  (globalThis.window as any).innerWidth = 1200;
}

describe('ShErrorPopover', () => {
  it('renders the three parts — code (mono red), plain sentence, raw formula', () => {
    render(ShErrorPopover, { props: { errorCode: '#VALUE!', rawFormula: '=D2*E2' } });
    expect(screen.getByTestId('sheet-error-popover-code').textContent).toBe('#VALUE!');
    expect(screen.getByTestId('sheet-error-popover-msg').textContent).toContain('wrong kind');
    expect(screen.getByTestId('sheet-error-popover-formula').textContent).toBe('=D2*E2');
  });

  it('renders for every one of the six known error codes', () => {
    const codes = ['#REF!', '#VALUE!', '#DIV/0!', '#NAME?', '#PARSE!', '#CYCLE!'];
    for (const code of codes) {
      const { unmount } = render(ShErrorPopover, { props: { errorCode: code, rawFormula: '=A1' } });
      expect(screen.getByTestId('sheet-error-popover-code').textContent).toBe(code);
      unmount();
    }
  });

  it('never renders any culprit-cell naming text (hard constraint)', () => {
    render(ShErrorPopover, { props: { errorCode: '#VALUE!', rawFormula: '=D2*E2' } });
    const msg = screen.getByTestId('sheet-error-popover-msg').textContent ?? '';
    expect(msg).not.toMatch(/\b[A-Z]{1,2}\d+\s+holds\b/);
  });

  it('falls back to the catalogue example formula when rawFormula is empty', () => {
    render(ShErrorPopover, { props: { errorCode: '#DIV/0!', rawFormula: '' } });
    expect(screen.getByTestId('sheet-error-popover-formula').textContent).toBe('=F5/D5');
  });

  it('shows the raw formula verbatim (not the catalogue example) when provided', () => {
    render(ShErrorPopover, { props: { errorCode: '#DIV/0!', rawFormula: '=Z9/Y9' } });
    expect(screen.getByTestId('sheet-error-popover-formula').textContent).toBe('=Z9/Y9');
  });

  it('renders nothing for an unrecognized error code', () => {
    render(ShErrorPopover, { props: { errorCode: '#WAT!', rawFormula: '=A1' } });
    expect(screen.queryByTestId('sheet-error-popover')).toBeNull();
  });

  it('uses the shared .sh-errpop shell class (not .sh-pop — its own distinct treatment)', () => {
    render(ShErrorPopover, { props: { errorCode: '#REF!', rawFormula: '=SUM(H1:H8)' } });
    const pop = screen.getByTestId('sheet-error-popover');
    expect(pop.className).toContain('sh-errpop');
  });
});

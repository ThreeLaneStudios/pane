/**
 * AttachmentToolbar.test.ts — Component tests for AttachmentToolbar.svelte
 *
 * Tests:
 *  - Hidden by default (no .is-open class without selection)
 *  - Visible when selected={true} prop
 *  - Shows correct mode label for each renderMode value
 *
 * §401 PR2 — spec-covered-by: ADR-0049 + ADR-0050
 */

import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/svelte/svelte5';
import AttachmentToolbar from './AttachmentToolbar.svelte';

// ─── Visibility ───────────────────────────────────────────────────────────────

describe('AttachmentToolbar — visibility', () => {
  it('does NOT have .is-open class when selected=false (default)', () => {
    const { container } = render(AttachmentToolbar, {
      props: { renderMode: 'card', selected: false },
    });

    const toolbar = container.querySelector('.at-toolbar');
    expect(toolbar).not.toBeNull();
    expect(toolbar!.classList.contains('is-open')).toBe(false);
  });

  it('has .is-open class when selected=true', () => {
    const { container } = render(AttachmentToolbar, {
      props: { renderMode: 'card', selected: true },
    });

    const toolbar = container.querySelector('.at-toolbar');
    expect(toolbar).not.toBeNull();
    expect(toolbar!.classList.contains('is-open')).toBe(true);
  });

  it('toolbar element is always in the DOM (CSS controls opacity)', () => {
    const { container } = render(AttachmentToolbar, {
      props: { renderMode: 'card', selected: false },
    });

    // The toolbar is always in the DOM; CSS opacity:0 hides it
    expect(container.querySelector('.at-toolbar')).not.toBeNull();
  });
});

// ─── Mode label rendering ─────────────────────────────────────────────────────

describe('AttachmentToolbar — mode label', () => {
  const MODE_CASES: Array<[string, string]> = [
    ['card',        'Card'],
    ['full-inline', 'Inline'],
    ['reader',      'Reader'],
    ['player',      'Player'],
    ['preview',     'Preview'],
    ['plain',       'Plain'],
  ];

  for (const [renderMode, expectedLabel] of MODE_CASES) {
    it(`shows "${expectedLabel}" for renderMode="${renderMode}"`, () => {
      render(AttachmentToolbar, {
        props: { renderMode, selected: false },
      });

      const modeBtn = screen.getByText(expectedLabel);
      expect(modeBtn).not.toBeNull();
    });
  }

  it('defaults to "Card" when renderMode is null', () => {
    render(AttachmentToolbar, {
      props: { renderMode: null, selected: false },
    });

    const modeBtn = screen.getByText('Card');
    expect(modeBtn).not.toBeNull();
  });

  it('passes through unknown renderMode values verbatim', () => {
    render(AttachmentToolbar, {
      props: { renderMode: 'custom-mode', selected: false },
    });

    // Unknown modes fall back to the raw value
    const modeBtn = screen.getByText('custom-mode');
    expect(modeBtn).not.toBeNull();
  });

  it('mode label is inside the mode button (.at-tbtn--mode)', () => {
    const { container } = render(AttachmentToolbar, {
      props: { renderMode: 'card', selected: false },
    });

    const modeBtn = container.querySelector('.at-tbtn--mode');
    expect(modeBtn).not.toBeNull();
    expect(modeBtn!.textContent).toContain('Card');
  });
});

// ─── Toolbar structure ────────────────────────────────────────────────────────

describe('AttachmentToolbar — structure', () => {
  it('renders a divider between mode label and action buttons', () => {
    const { container } = render(AttachmentToolbar, {
      props: { renderMode: 'card', selected: false },
    });

    expect(container.querySelector('.at-tdiv')).not.toBeNull();
  });

  it('renders open-externally button (wired in PR3, no longer disabled)', () => {
    // PR2: button was a disabled placeholder.
    // PR3: button is wired to onOpenExternally prop — no disabled attr.
    const { container } = render(AttachmentToolbar, {
      props: { renderMode: 'card', selected: false },
    });

    const openBtn = container.querySelector('[aria-label="Open externally"]');
    expect(openBtn).not.toBeNull();

    // PR3: button is interactive (not disabled)
    expect(openBtn!.hasAttribute('disabled')).toBe(false);
  });

  it('has correct aria-label on toolbar', () => {
    const { container } = render(AttachmentToolbar, {
      props: { renderMode: 'preview', selected: false },
    });

    const toolbar = container.querySelector('.at-toolbar');
    expect(toolbar!.getAttribute('role')).toBe('toolbar');
    expect(toolbar!.getAttribute('aria-label')).toBe('Attachment options');
  });
});

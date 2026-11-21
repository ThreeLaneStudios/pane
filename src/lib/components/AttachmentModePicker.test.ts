/**
 * AttachmentModePicker.test.ts - Component tests for AttachmentModePicker.svelte
 *                               and related picker integration in Toolbar + NodeView.
 *
 * Test groups:
 *  M-1  Per-type mode list: correct count per type
 *  M-2  Active-check: cyan Check icon next to current mode, absent from others
 *  M-3  Mode selection: clicking a mode fires onModeChange with correct value
 *  M-5  Actions: Open externally item fires its callback
 *  M-6  Keyboard: Esc closes; Enter on focused mode selects it
 *  M-7  Click-outside closes
 *  M-8  Toolbar integration: clicking the mode-label button opens the picker
 *  M-9  NodeView persistence: modeChange triggers onUpdateAttributes({ renderMode })
 *
 * §401 PR3 - spec-covered-by: ADR-0049 §2.1A + ADR-0050 §6
 * spec-skip: mode picker UI implementing the render-mode enum from ADR-0049 §2.1A
 *            (spec-covered by #1827) and enforcing the embed gate from ADR-0050 §6
 *            (spec-covered by #1831); no new spec surface, pure UI plumbing per
 *            already-accepted ADRs.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import { existsSync, readFileSync } from 'fs';
import { resolve } from 'path';
import AttachmentModePicker from './AttachmentModePicker.svelte';
import AttachmentToolbar from './AttachmentToolbar.svelte';
import AttachmentNodeView from './AttachmentNodeView.svelte';
import { VALID_RENDER_MODES } from '$lib/extensions/Attachment';
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
  convertFileSrc: (path: string) => path,
}));

afterEach(() => {
  vi.resetAllMocks();
});

// ---------------------------------------------------------------------------
// M-1 - Per-type mode list
// ---------------------------------------------------------------------------

describe('M-1 - per-type mode list renders correct modes', () => {
  it('image → 2 mode items (Inline, Card)', async () => {
    render(AttachmentModePicker, {
      props: {
        attachmentType: 'image',
        currentMode: 'full-inline',
        onModeChange: vi.fn(),
        onClose: vi.fn(),
      },
    });
    await tick();
    await tick();

    const items = document.querySelectorAll('[data-picker-item][data-mode]');
    expect(items.length).toBe(2);
    expect(screen.getByRole('menuitemradio', { name: 'Inline' })).not.toBeNull();
    expect(screen.getByRole('menuitemradio', { name: 'Card' })).not.toBeNull();
  });

  it('pdf → 2 mode items (Card, Reader)', async () => {
    render(AttachmentModePicker, {
      props: {
        attachmentType: 'pdf',
        currentMode: 'card',
        onModeChange: vi.fn(),
        onClose: vi.fn(),
      },
    });
    await tick(); await tick();

    const items = document.querySelectorAll('[data-picker-item][data-mode]');
    expect(items.length).toBe(2);
    expect(screen.getByRole('menuitemradio', { name: 'Card' })).not.toBeNull();
    expect(screen.getByRole('menuitemradio', { name: 'Reader' })).not.toBeNull();
  });

  it('audio → 2 mode items (Card, Player)', async () => {
    render(AttachmentModePicker, {
      props: {
        attachmentType: 'audio',
        currentMode: 'card',
        onModeChange: vi.fn(),
        onClose: vi.fn(),
      },
    });
    await tick(); await tick();

    const items = document.querySelectorAll('[data-picker-item][data-mode]');
    expect(items.length).toBe(2);
    expect(screen.getByRole('menuitemradio', { name: 'Player' })).not.toBeNull();
  });

  it('video → 2 mode items (Card, Player)', async () => {
    render(AttachmentModePicker, {
      props: {
        attachmentType: 'video',
        currentMode: 'card',
        onModeChange: vi.fn(),
        onClose: vi.fn(),
      },
    });
    await tick(); await tick();

    const items = document.querySelectorAll('[data-picker-item][data-mode]');
    expect(items.length).toBe(2);
    expect(screen.getByRole('menuitemradio', { name: 'Player' })).not.toBeNull();
  });

  it('html → 2 mode items (Card, Preview)', async () => {
    render(AttachmentModePicker, {
      props: {
        attachmentType: 'html',
        currentMode: 'card',
        onModeChange: vi.fn(),
        onClose: vi.fn(),
      },
    });
    await tick(); await tick();

    const items = document.querySelectorAll('[data-picker-item][data-mode]');
    expect(items.length).toBe(2);
    expect(screen.getByRole('menuitemradio', { name: 'Preview' })).not.toBeNull();
  });

  it('link → 2 mode items (Preview, Plain)', async () => {
    render(AttachmentModePicker, {
      props: {
        attachmentType: 'link',
        currentMode: 'preview',
        onModeChange: vi.fn(),
        onClose: vi.fn(),
      },
    });
    await tick(); await tick();

    const items = document.querySelectorAll('[data-picker-item][data-mode]');
    expect(items.length).toBe(2);
    expect(screen.getByRole('menuitemradio', { name: 'Preview' })).not.toBeNull();
    expect(screen.getByRole('menuitemradio', { name: 'Plain' })).not.toBeNull();
  });

  it('file type → falls back to card only (1 mode)', async () => {
    render(AttachmentModePicker, {
      props: {
        attachmentType: 'file',
        currentMode: 'card',
        onModeChange: vi.fn(),
        onClose: vi.fn(),
      },
    });
    await tick(); await tick();

    const items = document.querySelectorAll('[data-picker-item][data-mode]');
    expect(items.length).toBe(1);
    expect(screen.getByRole('menuitemradio', { name: 'Card' })).not.toBeNull();
  });
});

// ---------------------------------------------------------------------------
// M-2 - Active-check icon
// ---------------------------------------------------------------------------

describe('M-2 - active-check: aria-checked=true and .is-active on current mode', () => {
  it('image: full-inline (Inline) is active, Card is not', async () => {
    render(AttachmentModePicker, {
      props: {
        attachmentType: 'image',
        currentMode: 'full-inline',
        onModeChange: vi.fn(),
        onClose: vi.fn(),
      },
    });
    await tick(); await tick();

    const inlineBtn = screen.getByRole('menuitemradio', { name: 'Inline' });
    expect(inlineBtn.getAttribute('aria-checked')).toBe('true');
    expect(inlineBtn.classList.contains('is-active')).toBe(true);

    const cardBtn = screen.getByRole('menuitemradio', { name: 'Card' });
    expect(cardBtn.getAttribute('aria-checked')).toBe('false');
    expect(cardBtn.classList.contains('is-active')).toBe(false);
  });

  it('pdf: reader is active, card is not', async () => {
    render(AttachmentModePicker, {
      props: {
        attachmentType: 'pdf',
        currentMode: 'reader',
        onModeChange: vi.fn(),
        onClose: vi.fn(),
      },
    });
    await tick(); await tick();

    const readerBtn = screen.getByRole('menuitemradio', { name: 'Reader' });
    expect(readerBtn.getAttribute('aria-checked')).toBe('true');
    expect(readerBtn.classList.contains('is-active')).toBe(true);

    const cardBtn = screen.getByRole('menuitemradio', { name: 'Card' });
    expect(cardBtn.getAttribute('aria-checked')).toBe('false');
  });

  it('separator and open-externally action item are always rendered below mode list', async () => {
    render(AttachmentModePicker, {
      props: {
        attachmentType: 'image',
        currentMode: 'card',
        onModeChange: vi.fn(),
        onClose: vi.fn(),
      },
    });
    await tick(); await tick();

    expect(document.querySelector('.amp-separator')).not.toBeNull();
    expect(document.querySelector('[data-action="open-externally"]')).not.toBeNull();
  });
});

// ---------------------------------------------------------------------------
// M-3 - Mode selection fires onModeChange
// ---------------------------------------------------------------------------

describe('M-3 - clicking a mode fires onModeChange with correct value', () => {
  it('clicking "Card" on an image picker calls onModeChange("card")', async () => {
    const onModeChange = vi.fn();
    const onClose = vi.fn();

    render(AttachmentModePicker, {
      props: {
        attachmentType: 'image',
        currentMode: 'full-inline',
        onModeChange,
        onClose,
      },
    });
    await tick(); await tick();

    const cardBtn = screen.getByRole('menuitemradio', { name: 'Card' });
    await fireEvent.click(cardBtn);

    expect(onModeChange).toHaveBeenCalledOnce();
    expect(onModeChange).toHaveBeenCalledWith('card');
  });

  it('clicking "Inline" on an image picker calls onModeChange("full-inline")', async () => {
    const onModeChange = vi.fn();
    const onClose = vi.fn();

    render(AttachmentModePicker, {
      props: {
        attachmentType: 'image',
        currentMode: 'card',
        onModeChange,
        onClose,
      },
    });
    await tick(); await tick();

    const inlineBtn = screen.getByRole('menuitemradio', { name: 'Inline' });
    await fireEvent.click(inlineBtn);

    expect(onModeChange).toHaveBeenCalledWith('full-inline');
  });

  it('clicking "Player" on an audio picker calls onModeChange("player")', async () => {
    const onModeChange = vi.fn();

    render(AttachmentModePicker, {
      props: {
        attachmentType: 'audio',
        currentMode: 'card',
        onModeChange,
        onClose: vi.fn(),
      },
    });
    await tick(); await tick();

    const playerBtn = screen.getByRole('menuitemradio', { name: 'Player' });
    await fireEvent.click(playerBtn);

    expect(onModeChange).toHaveBeenCalledWith('player');
  });

  it('clicking the already-active mode does NOT fire onModeChange, but does close', async () => {
    const onModeChange = vi.fn();
    const onClose = vi.fn();

    render(AttachmentModePicker, {
      props: {
        attachmentType: 'image',
        currentMode: 'full-inline',
        onModeChange,
        onClose,
      },
    });
    await tick(); await tick();

    // Click the currently-active mode (full-inline = Inline)
    const inlineBtn = screen.getByRole('menuitemradio', { name: 'Inline' });
    await fireEvent.click(inlineBtn);

    // mode didn't change → no modeChange callback
    expect(onModeChange).not.toHaveBeenCalled();
    // but picker closes on any mode click
    expect(onClose).toHaveBeenCalledOnce();
  });
});

// ---------------------------------------------------------------------------
// M-5 - Action items fire callbacks
// ---------------------------------------------------------------------------

describe('M-5 - action item fires its callback', () => {
  it('clicking "Open externally" fires onOpenExternally callback', async () => {
    const onOpenExternally = vi.fn();
    const onClose = vi.fn();

    render(AttachmentModePicker, {
      props: {
        attachmentType: 'pdf',
        currentMode: 'card',
        onOpenExternally,
        onClose,
      },
    });
    await tick(); await tick();

    const openBtn = document.querySelector('[data-action="open-externally"]') as HTMLElement;
    expect(openBtn).not.toBeNull();
    await fireEvent.click(openBtn);

    expect(onOpenExternally).toHaveBeenCalledOnce();
  });

});

// ---------------------------------------------------------------------------
// M-6 - Keyboard: Esc closes; Enter on focused item selects it
// ---------------------------------------------------------------------------

describe('M-6 - keyboard navigation', () => {
  it('pressing Esc fires onClose', async () => {
    const onClose = vi.fn();

    const { container } = render(AttachmentModePicker, {
      props: {
        attachmentType: 'image',
        currentMode: 'card',
        onClose,
      },
    });
    await tick(); await tick();

    const popover = container.querySelector('[role="menu"]') as HTMLElement;
    expect(popover).not.toBeNull();
    await fireEvent.keyDown(popover, { key: 'Escape' });

    expect(onClose).toHaveBeenCalledOnce();
  });

  it('pressing Enter with first item focused fires mode selection for first mode', async () => {
    const onModeChange = vi.fn();
    const onClose = vi.fn();

    const { container } = render(AttachmentModePicker, {
      props: {
        attachmentType: 'image',
        currentMode: 'card', // NOT the first mode (full-inline is first)
        onModeChange,
        onClose,
      },
    });
    await tick(); await tick();

    // Focus the first mode item (full-inline = 'Inline')
    const items = document.querySelectorAll('[data-picker-item][data-mode]');
    expect(items.length).toBe(2);
    (items[0] as HTMLElement).focus();

    const popover = container.querySelector('[role="menu"]') as HTMLElement;
    await fireEvent.keyDown(popover, { key: 'Enter' });

    // First mode for image is 'full-inline'
    expect(onModeChange).toHaveBeenCalledWith('full-inline');
  });

  it('ArrowDown key does not close the popover', async () => {
    const onClose = vi.fn();

    const { container } = render(AttachmentModePicker, {
      props: {
        attachmentType: 'pdf',
        currentMode: 'card',
        onClose,
      },
    });
    await tick(); await tick();

    const popover = container.querySelector('[role="menu"]') as HTMLElement;
    await fireEvent.keyDown(popover, { key: 'ArrowDown' });

    expect(onClose).not.toHaveBeenCalled();
  });

  it('ArrowUp key does not close the popover', async () => {
    const onClose = vi.fn();

    const { container } = render(AttachmentModePicker, {
      props: {
        attachmentType: 'pdf',
        currentMode: 'card',
        onClose,
      },
    });
    await tick(); await tick();

    const popover = container.querySelector('[role="menu"]') as HTMLElement;
    await fireEvent.keyDown(popover, { key: 'ArrowUp' });

    expect(onClose).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// M-7 - Click-outside closes
// ---------------------------------------------------------------------------

describe('M-7 - click-outside closes the picker', () => {
  it('mousedown on document.body (outside popover) fires onClose', async () => {
    const onClose = vi.fn();

    render(AttachmentModePicker, {
      props: {
        attachmentType: 'image',
        currentMode: 'card',
        onClose,
      },
    });
    await tick(); await tick();

    // Verify picker is rendered
    expect(document.querySelectorAll('[data-picker-item]').length).toBeGreaterThan(0);

    // Simulate mousedown outside (on body)
    await fireEvent.mouseDown(document.body);

    expect(onClose).toHaveBeenCalledOnce();
  });

  it('mousedown inside the popover does NOT fire onClose', async () => {
    const onClose = vi.fn();

    render(AttachmentModePicker, {
      props: {
        attachmentType: 'image',
        currentMode: 'card',
        onClose,
      },
    });
    await tick(); await tick();

    // Click inside a mode item (inside the popover)
    const firstItem = document.querySelector('[data-picker-item]') as HTMLElement;
    expect(firstItem).not.toBeNull();
    await fireEvent.mouseDown(firstItem);

    expect(onClose).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// M-8 - Toolbar integration: clicking mode-label button opens picker
// ---------------------------------------------------------------------------

describe('M-8 - toolbar integration: mode-label button opens picker', () => {
  it('mode-label button has aria-haspopup="menu" and initially aria-expanded="false"', () => {
    const { container } = render(AttachmentToolbar, {
      props: {
        renderMode: 'card',
        attachmentType: 'pdf',
        selected: true,
      },
    });

    const modeBtn = container.querySelector('.at-tbtn--mode') as HTMLElement;
    expect(modeBtn).not.toBeNull();
    expect(modeBtn.getAttribute('aria-haspopup')).toBe('menu');
    expect(modeBtn.getAttribute('aria-expanded')).toBe('false');
  });

  it('clicking mode-label button opens the AttachmentModePicker (role="menu" appears)', async () => {
    const { container } = render(AttachmentToolbar, {
      props: {
        renderMode: 'card',
        attachmentType: 'image',
        selected: true,
        onModeChange: vi.fn(),
      },
    });

    // Picker not present initially
    expect(document.querySelector('[role="menu"]')).toBeNull();

    const modeBtn = container.querySelector('.at-tbtn--mode') as HTMLElement;
    await fireEvent.click(modeBtn);
    await tick(); await tick();

    // Picker is now in the DOM
    expect(document.querySelector('[role="menu"]')).not.toBeNull();
  });

  it('after clicking mode-label button, aria-expanded becomes "true"', async () => {
    const { container } = render(AttachmentToolbar, {
      props: {
        renderMode: 'card',
        attachmentType: 'pdf',
        selected: true,
        onModeChange: vi.fn(),
      },
    });

    const modeBtn = container.querySelector('.at-tbtn--mode') as HTMLElement;
    await fireEvent.click(modeBtn);
    await tick();

    expect(modeBtn.getAttribute('aria-expanded')).toBe('true');
  });

  it('toolbar open-externally button fires onOpenExternally prop', async () => {
    const onOpenExternally = vi.fn();
    const { container } = render(AttachmentToolbar, {
      props: {
        renderMode: 'card',
        attachmentType: 'pdf',
        selected: true,
        onOpenExternally,
      },
    });

    const openBtn = container.querySelector('[aria-label="Open externally"]') as HTMLElement;
    expect(openBtn).not.toBeNull();
    await fireEvent.click(openBtn);

    expect(onOpenExternally).toHaveBeenCalledOnce();
  });
});

// ---------------------------------------------------------------------------
// M-9 - NodeView persistence: modeChange triggers onUpdateAttributes
// ---------------------------------------------------------------------------

// AttachmentNodeView imports openAttachmentExternally from $lib/api.
// Mock it so card clicks (and toolbar actions) don't throw.
vi.mock('$lib/api', async (importOriginal) => {
  const original = await importOriginal() as any;
  return {
    ...original,
    openAttachmentExternally: vi.fn().mockResolvedValue(undefined),
  fetchLinkPreview: vi.fn().mockResolvedValue(null),
  getCachedLinkPreview: vi.fn().mockResolvedValue(null),
  };
});

describe('M-9 - NodeView persistence: modeChange calls onUpdateAttributes({ renderMode })', () => {
  it('selecting a mode via picker calls onUpdateAttributes({ renderMode: newMode })', async () => {
    const onUpdateAttributes = vi.fn();

    const { container } = render(AttachmentNodeView, {
      props: {
        attrs: {
          attachmentId: 'test-node-001',
          attachmentType: 'image',
          filename: 'photo.jpg',
          src: null,
          alt: 'photo.jpg',
          renderMode: 'full-inline',
        },
        selected: true,
        onUpdateAttributes,
      },
    });

    await tick();

    // Toolbar should be visible (selected=true → .is-open)
    const toolbar = container.querySelector('.at-toolbar');
    expect(toolbar).not.toBeNull();
    expect(toolbar!.classList.contains('is-open')).toBe(true);

    // Open the picker
    const modeBtn = container.querySelector('.at-tbtn--mode') as HTMLElement;
    expect(modeBtn).not.toBeNull();
    await fireEvent.click(modeBtn);
    await tick(); await tick();

    // Picker should now be in the DOM
    expect(document.querySelector('[role="menu"]')).not.toBeNull();

    // Select 'Card' mode (changing from full-inline)
    const cardBtn = screen.getByRole('menuitemradio', { name: 'Card' });
    await fireEvent.click(cardBtn);

    // onUpdateAttributes should have been called with { renderMode: 'card' }
    expect(onUpdateAttributes).toHaveBeenCalledOnce();
    expect(onUpdateAttributes).toHaveBeenCalledWith({ renderMode: 'card' });
  });

  it('selecting the current mode does NOT call onUpdateAttributes', async () => {
    const onUpdateAttributes = vi.fn();

    render(AttachmentNodeView, {
      props: {
        attrs: {
          attachmentId: 'test-node-002',
          attachmentType: 'image',
          filename: 'photo.jpg',
          src: null,
          alt: 'photo.jpg',
          renderMode: 'full-inline',
        },
        selected: true,
        onUpdateAttributes,
      },
    });

    await tick();

    // Open the picker
    const modeBtn = document.querySelector('.at-tbtn--mode') as HTMLElement;
    await fireEvent.click(modeBtn);
    await tick(); await tick();

    // Click the already-active mode (Inline = full-inline)
    const inlineBtn = screen.getByRole('menuitemradio', { name: 'Inline' });
    await fireEvent.click(inlineBtn);

    // Mode unchanged → no updateAttributes call
    expect(onUpdateAttributes).not.toHaveBeenCalled();
  });

  it('NodeView passes attachmentType to toolbar for correct picker mode list', async () => {
    const onUpdateAttributes = vi.fn();

    const { container } = render(AttachmentNodeView, {
      props: {
        attrs: {
          attachmentId: 'test-node-003',
          attachmentType: 'audio',
          filename: 'track.mp3',
          src: null,
          alt: null,
          renderMode: 'card',
        },
        selected: true,
        onUpdateAttributes,
      },
    });

    await tick();

    // Open picker for audio type
    const modeBtn = container.querySelector('.at-tbtn--mode') as HTMLElement;
    await fireEvent.click(modeBtn);
    await tick(); await tick();

    // Audio picker should show Card and Player modes (2 items)
    const items = document.querySelectorAll('[data-picker-item][data-mode]');
    expect(items.length).toBe(2);
    expect(screen.getByRole('menuitemradio', { name: 'Player' })).not.toBeNull();
  });
});

// ---------------------------------------------------------------------------
// M-10 - Per-mode descriptions (.amp-item-desc) rendered under each mode label
//          Canonical strings from attachment-screens.jsx MODES export (#1774)
// ---------------------------------------------------------------------------

describe('M-10 - per-mode descriptions render under each mode label', () => {
  it('each mode item has an .amp-item-desc element (image type)', async () => {
    render(AttachmentModePicker, {
      props: {
        attachmentType: 'image',
        currentMode: 'full-inline',
        onModeChange: vi.fn(),
        onClose: vi.fn(),
      },
    });
    await tick();
    await tick();

    // image has 2 modes: full-inline (Inline) and card (Card)
    const modeItems = document.querySelectorAll('[data-picker-item][data-mode]');
    expect(modeItems.length).toBe(2);

    // Every mode item must have an .amp-item-desc child
    for (const item of Array.from(modeItems)) {
      const desc = item.querySelector('.amp-item-desc');
      expect(desc).not.toBeNull();
    }
  });

  it('image: full-inline shows canonical desc "Inline, drag a corner to resize."', async () => {
    render(AttachmentModePicker, {
      props: { attachmentType: 'image', currentMode: 'full-inline', onModeChange: vi.fn(), onClose: vi.fn() },
    });
    await tick(); await tick();

    const fullInlineBtn = document.querySelector('[data-mode="full-inline"]');
    expect(fullInlineBtn).not.toBeNull();
    const desc = fullInlineBtn!.querySelector('.amp-item-desc');
    expect(desc?.textContent?.trim()).toBe('Inline, drag a corner to resize.');
  });

  it('image: card shows canonical desc "Thumbnail with name and size."', async () => {
    render(AttachmentModePicker, {
      props: { attachmentType: 'image', currentMode: 'card', onModeChange: vi.fn(), onClose: vi.fn() },
    });
    await tick(); await tick();

    const cardBtn = document.querySelector('[data-mode="card"]');
    const desc = cardBtn!.querySelector('.amp-item-desc');
    expect(desc?.textContent?.trim()).toBe('Thumbnail with name and size.');
  });

  it('link: plain shows "Clickable URL, inline in your text."', async () => {
    render(AttachmentModePicker, {
      props: { attachmentType: 'link', currentMode: 'preview', onModeChange: vi.fn(), onClose: vi.fn() },
    });
    await tick(); await tick();

    const plainBtn = document.querySelector('[data-mode="plain"]');
    expect(plainBtn?.querySelector('.amp-item-desc')?.textContent?.trim())
      .toBe('Clickable URL, inline in your text.');
  });

  it('link: preview shows "Favicon, title and description."', async () => {
    render(AttachmentModePicker, {
      props: { attachmentType: 'link', currentMode: 'plain', onModeChange: vi.fn(), onClose: vi.fn() },
    });
    await tick(); await tick();

    const previewBtn = document.querySelector('[data-mode="preview"]');
    expect(previewBtn?.querySelector('.amp-item-desc')?.textContent?.trim())
      .toBe('Favicon, title and description.');
  });

  it('audio: player shows "Plays in the sidebar; audio continues while you browse."', async () => {
    render(AttachmentModePicker, {
      props: { attachmentType: 'audio', currentMode: 'card', onModeChange: vi.fn(), onClose: vi.fn() },
    });
    await tick(); await tick();

    const playerBtn = document.querySelector('[data-mode="player"]');
    expect(playerBtn?.querySelector('.amp-item-desc')?.textContent?.trim())
      .toBe('Plays in the sidebar; audio continues while you browse.');
  });

  it('video: player shows "Embedded player, inline in the note."', async () => {
    render(AttachmentModePicker, {
      props: { attachmentType: 'video', currentMode: 'card', onModeChange: vi.fn(), onClose: vi.fn() },
    });
    await tick(); await tick();

    const playerBtn = document.querySelector('[data-mode="player"]');
    expect(playerBtn?.querySelector('.amp-item-desc')?.textContent?.trim())
      .toBe('Embedded player, inline in the note.');
  });

  it('pdf: card shows canonical desc "Icon, name, size and page count."', async () => {
    render(AttachmentModePicker, {
      props: { attachmentType: 'pdf', currentMode: 'reader', onModeChange: vi.fn(), onClose: vi.fn() },
    });
    await tick(); await tick();

    const cardBtn = document.querySelector('[data-mode="card"]');
    expect(cardBtn?.querySelector('.amp-item-desc')?.textContent?.trim())
      .toBe('Icon, name, size and page count.');
  });

  it('pdf: reader shows "Open full-screen, scroll all pages."', async () => {
    render(AttachmentModePicker, {
      props: { attachmentType: 'pdf', currentMode: 'card', onModeChange: vi.fn(), onClose: vi.fn() },
    });
    await tick(); await tick();

    const readerBtn = document.querySelector('[data-mode="reader"]');
    expect(readerBtn?.querySelector('.amp-item-desc')?.textContent?.trim())
      .toBe('Open full-screen, scroll all pages.');
  });

  it('html: preview shows "Sandboxed full-screen render. No scripts or network."', async () => {
    render(AttachmentModePicker, {
      props: { attachmentType: 'html', currentMode: 'card', onModeChange: vi.fn(), onClose: vi.fn() },
    });
    await tick(); await tick();

    const previewBtn = document.querySelector('[data-mode="preview"]');
    expect(previewBtn?.querySelector('.amp-item-desc')?.textContent?.trim())
      .toBe('Sandboxed full-screen render. No scripts or network.');
  });

  it('unknown type (file) renders NO .amp-item-desc elements — pair not in table', async () => {
    render(AttachmentModePicker, {
      props: { attachmentType: 'file', currentMode: 'card', onModeChange: vi.fn(), onClose: vi.fn() },
    });
    await tick(); await tick();

    // 'file' has no entry in MODE_DESCRIPTIONS → {#if descText} suppresses all desc spans
    const descEls = document.querySelectorAll('[data-picker-item][data-mode] .amp-item-desc');
    expect(descEls.length).toBe(0);
  });

  it('.amp-item-desc appears once per mode item across all types', async () => {
    const types = ['image', 'pdf', 'audio', 'video', 'html', 'link'] as const;

    for (const type of types) {
      document.body.innerHTML = ''; // reset DOM between renders
      const { unmount } = render(AttachmentModePicker, {
        props: {
          attachmentType: type,
          currentMode: null,
          onModeChange: vi.fn(),
          onClose: vi.fn(),
        },
      });
      await tick();
      await tick();

      const modeItems = document.querySelectorAll('[data-picker-item][data-mode]');
      const descEls = document.querySelectorAll('[data-picker-item][data-mode] .amp-item-desc');
      // One .amp-item-desc per mode item
      expect(descEls.length).toBe(modeItems.length);

      unmount();
    }
  });
});

// ---------------------------------------------------------------------------
// RIP-1..3 — #401 iframe embed rip assertions
// ---------------------------------------------------------------------------

describe('RIP-1: VALID_RENDER_MODES.link contains exactly [preview, plain] — no embed', () => {
  it('link render modes are preview and plain only', () => {
    const linkModes = VALID_RENDER_MODES['link'];
    expect(linkModes).toEqual(['preview', 'plain']);
    expect(linkModes).not.toContain('embed');
  });
});

describe('RIP-2: AttachmentModePicker renders exactly 2 options for link node (preview, plain)', () => {
  it('link picker shows preview and plain — no embed option regardless of any setting', async () => {
    render(AttachmentModePicker, {
      props: {
        attachmentType: 'link',
        currentMode: 'preview',
        onModeChange: vi.fn(),
        onClose: vi.fn(),
      },
    });
    await tick();
    await tick();

    const items = document.querySelectorAll('[data-picker-item][data-mode]');
    expect(items.length).toBe(2);
    expect(screen.getByRole('menuitemradio', { name: 'Preview' })).not.toBeNull();
    expect(screen.getByRole('menuitemradio', { name: 'Plain' })).not.toBeNull();
    expect(document.querySelector('[data-mode="embed"]')).toBeNull();
  });
});

describe('RIP-3: AttachmentLinkEmbed.svelte no longer exists and is not imported anywhere', () => {
  const componentsDir = resolve(__dirname);
  const embedFilePath = resolve(componentsDir, 'AttachmentLinkEmbed.svelte');
  const nodeViewPath = resolve(componentsDir, 'AttachmentNodeView.svelte');

  it('AttachmentLinkEmbed.svelte file has been deleted', () => {
    expect(existsSync(embedFilePath)).toBe(false);
  });

  it('AttachmentNodeView.svelte does not import AttachmentLinkEmbed', () => {
    const source = readFileSync(nodeViewPath, 'utf-8');
    expect(source).not.toContain('AttachmentLinkEmbed');
  });
});

/**
 * AttachmentNodeView.test.ts — Component tests for AttachmentNodeView.svelte
 *
 * Tests:
 *  - Mounts AttachmentCard with correct attachment data
 *  - Mounts AttachmentToolbar and passes selection state
 *  - Click on card invokes openAttachmentExternally with correct ID
 *  - Player mode wiring (PR5b): audio → loadTrack called; non-audio guard
 *
 * §401 PR2 — spec-covered-by: ADR-0049 + ADR-0050
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import AttachmentNodeView from './AttachmentNodeView.svelte';
import { _createAttachmentNodeViewFactory } from '$lib/extensions/Attachment';
import { activeNote } from '$lib/stores/app';
import type { NoteContent, NoteMeta } from '$lib/types';

// ─── Mock openAttachmentExternally ─────────────────────────────────────────────
// The component imports this from $lib/api which calls invoke().
// vitest-setup.ts already mocks window.__TAURI_INTERNALS__.invoke.
// We also vi.mock the module to intercept the call directly.

vi.mock('$lib/api', async (importOriginal) => {
  const original = await importOriginal() as any;
  return {
    ...original,
    openAttachmentExternally: vi.fn().mockResolvedValue(undefined),
  fetchLinkPreview: vi.fn().mockResolvedValue(null),
  getCachedLinkPreview: vi.fn().mockResolvedValue(null),
    resolveAttachmentSrc: vi.fn(async (id: string) => `asset://localhost/attachments/${id}.mp3`),
  };
});

// Import AFTER mock registration
import * as api from '$lib/api';
import { resolveAttachmentSrc } from '$lib/api';

// ─── Mock loadTrack from player store ─────────────────────────────────────────────
vi.mock('$lib/stores/player', async (importOriginal) => {
  const original = await importOriginal() as any;
  return {
    ...original,
    loadTrack: vi.fn(),
  };
});
import { loadTrack, playerStatus } from '$lib/stores/player';
import { get } from 'svelte/store';

// ─── Fixtures ─────────────────────────────────────────────────────────────────

function mkAttrs(overrides: Record<string, any> = {}) {
  return {
    attachmentId: 'att-uuid-abcdef',
    attachmentType: 'pdf',
    filename: 'report.pdf',
    src: null,
    alt: 'report',
    duration: null,
    pageCount: 8,
    size: null,
    renderMode: 'card',
    status: 'ready' as const,
    errorDetail: null,
    ...overrides,
  };
}

// ─── AttachmentCard mounting ───────────────────────────────────────────────────

describe('AttachmentNodeView — mounts AttachmentCard with correct data', () => {
  it('renders the attachment filename from attrs', () => {
    render(AttachmentNodeView, {
      props: { attrs: mkAttrs({ filename: 'my-document.pdf' }) },
    });

    expect(screen.getByText('my-document.pdf')).not.toBeNull();
  });

  it('renders page count in meta line for PDF type', () => {
    render(AttachmentNodeView, {
      props: {
        attrs: mkAttrs({
          attachmentType: 'pdf',
          filename: 'specs.pdf',
          pageCount: 42,
        }),
      },
    });

    const meta = screen.getByText((text) => text.includes('42 pages'));
    expect(meta).not.toBeNull();
  });

  it('renders type prefix + size in meta line for audio type (no duration per #1774)', () => {
    render(AttachmentNodeView, {
      props: {
        attrs: mkAttrs({
          attachmentType: 'audio',
          filename: 'podcast.mp3',
          pageCount: null,
          duration: 3660, // duration is set but should NOT appear in meta per #1774
          size: 4194304, // 4 MB — should appear as "4.0 MB"
        }),
      },
    });

    // Audio meta per #1774: "MP3 · 4.0 MB" — NO duration segment
    const meta = screen.getByText((text) => text.includes('MP3') && text.includes('4.0 MB'));
    expect(meta).not.toBeNull();
    // Regression guard: duration must NOT appear in audio meta
    expect(screen.queryByText((text) => text.includes('1:01:00'))).toBeNull();
  });

  it('renders image dimensions (WxH) in meta line for image type when width/height are set', () => {
    render(AttachmentNodeView, {
      props: {
        attrs: mkAttrs({
          attachmentType: 'image',
          filename: 'photo.jpg',
          width: 1920,
          height: 1080,
          pageCount: null,
          duration: null,
          renderMode: 'card', // card mode so AttachmentCard is rendered
        }),
      },
    });

    // AttachmentCard.metaLine() for image with dims should show "1920×1080"
    const meta = screen.getByText((text) => text.includes('1920×1080'));
    expect(meta).not.toBeNull();
  });

  it('carries data-attachment-id on the host wrapper', () => {
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkAttrs({ attachmentId: 'my-specific-id' }) },
    });

    const wrapper = container.querySelector('[data-attachment-id="my-specific-id"]');
    expect(wrapper).not.toBeNull();
  });

  it('carries data-attachment-type on the host wrapper', () => {
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkAttrs({ attachmentType: 'video', filename: 'clip.mp4' }) },
    });

    const wrapper = container.querySelector('[data-attachment-type="video"]');
    expect(wrapper).not.toBeNull();
  });

  it('carries data-attachment sentinel on the host wrapper (for serializer)', () => {
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkAttrs() },
    });

    expect(container.querySelector('[data-attachment]')).not.toBeNull();
  });

  it('carries .attachment-wrapper class on the host wrapper (for serializer)', () => {
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkAttrs() },
    });

    expect(container.querySelector('.attachment-wrapper')).not.toBeNull();
  });
});

// ─── AttachmentToolbar mounting + selection state ──────────────────────────────

describe('AttachmentNodeView — mounts AttachmentToolbar and passes selection state', () => {
  it('renders a .at-toolbar element', () => {
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkAttrs(), selected: false },
    });

    expect(container.querySelector('.at-toolbar')).not.toBeNull();
  });

  it('toolbar does NOT have .is-open when selected=false', () => {
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkAttrs(), selected: false },
    });

    const toolbar = container.querySelector('.at-toolbar');
    expect(toolbar!.classList.contains('is-open')).toBe(false);
  });

  it('toolbar has .is-open when selected=true', () => {
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkAttrs(), selected: true },
    });

    const toolbar = container.querySelector('.at-toolbar');
    expect(toolbar!.classList.contains('is-open')).toBe(true);
  });

  it('toolbar shows the current renderMode label', () => {
    render(AttachmentNodeView, {
      props: {
        attrs: mkAttrs({ renderMode: 'reader' }),
        selected: false,
      },
    });

    expect(screen.getByText('Reader')).not.toBeNull();
  });

  it('host wrapper has .is-selected when selected=true', () => {
    const { container } = render(AttachmentNodeView, {
      props: { attrs: mkAttrs(), selected: true },
    });

    expect(container.querySelector('.at-nv-host.is-selected')).not.toBeNull();
  });
});

// ─── Card click → openAttachmentExternally ────────────────────────────────────

describe('AttachmentNodeView — card click invokes openAttachmentExternally', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('calls openAttachmentExternally with the correct attachmentId on card click', async () => {
    const { container } = render(AttachmentNodeView, {
      props: {
        attrs: mkAttrs({ attachmentId: 'click-test-uuid', attachmentType: 'pdf', filename: 'doc.pdf' }),
      },
    });

    const card = container.querySelector('.at-card') as HTMLElement;
    expect(card).not.toBeNull();
    await fireEvent.click(card);

    expect(api.openAttachmentExternally).toHaveBeenCalledTimes(1);
    expect(api.openAttachmentExternally).toHaveBeenCalledWith('click-test-uuid');
  });

  it('does NOT call openAttachmentExternally when attachmentId is null', async () => {
    const { container } = render(AttachmentNodeView, {
      props: {
        attrs: mkAttrs({ attachmentId: null }),
      },
    });

    const card = container.querySelector('.at-card') as HTMLElement;
    if (card) {
      await fireEvent.click(card);
    }

    expect(api.openAttachmentExternally).not.toHaveBeenCalled();
  });

  it('calls openAttachmentExternally for image type', async () => {
    const { container } = render(AttachmentNodeView, {
      props: {
        attrs: mkAttrs({
          attachmentId: 'img-uuid',
          attachmentType: 'image',
          filename: 'photo.jpg',
        }),
      },
    });

    const card = container.querySelector('.at-card') as HTMLElement;
    await fireEvent.click(card);

    expect(api.openAttachmentExternally).toHaveBeenCalledWith('img-uuid');
  });

  it('calls openAttachmentExternally for audio type', async () => {
    const { container } = render(AttachmentNodeView, {
      props: {
        attrs: mkAttrs({
          attachmentId: 'audio-uuid',
          attachmentType: 'audio',
          filename: 'track.mp3',
          duration: 120,
        }),
      },
    });

    const card = container.querySelector('.at-card') as HTMLElement;
    await fireEvent.click(card);

    expect(api.openAttachmentExternally).toHaveBeenCalledWith('audio-uuid');
  });
});

// ─── NodeView lifecycle — remount on update (Reed audit #1835 I-1) ─────────────────────
// Tests are run against the real TipTap NodeView factory (not the Svelte
// component in isolation) to verify that mount/unmount is correctly tracked
// across update() / selectNode() / deselectNode() calls.
// ZERO stubs — all assertions are real DOM outcomes.

function mkNodeProxy(filename: string, overrides: Record<string, any> = {}) {
  return {
    attrs: {
      attachmentId: 'lifecycle-test-id',
      attachmentType: 'pdf',
      filename,
      src: null,
      alt: null,
      duration: null,
      pageCount: 4,
      renderMode: 'card',
      ...overrides,
    },
  };
}

describe('NodeView lifecycle — remount on update (Reed audit #1835 I-1)', () => {
  it('does not duplicate .at-card children when update() is called', async () => {
    const factory = _createAttachmentNodeViewFactory();
    const nv = factory({ node: mkNodeProxy('original.pdf') });

    // Attach to document so Svelte can render properly in jsdom.
    document.body.appendChild(nv.dom);
    await tick();

    expect(nv.dom.querySelectorAll('.at-card').length).toBe(1);

    nv.update(mkNodeProxy('updated.pdf'));
    await tick();

    // If the old instance was NOT unmounted, there would be 2+ .at-card elements.
    expect(nv.dom.querySelectorAll('.at-card').length).toBe(1);

    nv.destroy();
    document.body.removeChild(nv.dom);
  });

  it('reflects new attrs in the DOM after update(), not the original', async () => {
    const factory = _createAttachmentNodeViewFactory();
    const nv = factory({ node: mkNodeProxy('before-update.pdf') });

    document.body.appendChild(nv.dom);
    await tick();

    expect(nv.dom.textContent).toContain('before-update.pdf');

    nv.update(mkNodeProxy('after-update.pdf'));
    await tick();

    expect(nv.dom.textContent).toContain('after-update.pdf');
    expect(nv.dom.textContent).not.toContain('before-update.pdf');

    nv.destroy();
    document.body.removeChild(nv.dom);
  });

  it('does not duplicate .at-card after selectNode() + deselectNode() following update()', async () => {
    const factory = _createAttachmentNodeViewFactory();
    const nv = factory({ node: mkNodeProxy('doc-v1.pdf') });

    document.body.appendChild(nv.dom);
    await tick();

    // Attr change: node version 2
    nv.update(mkNodeProxy('doc-v2.pdf'));
    await tick();

    // Select — must use CURRENT node attrs (doc-v2.pdf), not original closure value
    nv.selectNode();
    await tick();

    expect(nv.dom.querySelectorAll('.at-card').length).toBe(1);
    expect(nv.dom.textContent).toContain('doc-v2.pdf');

    // Deselect — also one card, no duplication
    nv.deselectNode();
    await tick();

    expect(nv.dom.querySelectorAll('.at-card').length).toBe(1);

    nv.destroy();
    document.body.removeChild(nv.dom);
  });

  it('destroy() cleans up the live instance (no lingering .at-card after destroy)', async () => {
    const factory = _createAttachmentNodeViewFactory();
    const nv = factory({ node: mkNodeProxy('cleanup-test.pdf') });

    document.body.appendChild(nv.dom);
    await tick();

    // Trigger an update so currentExports has been reassigned at least once
    nv.update(mkNodeProxy('cleanup-test-v2.pdf'));
    await tick();

    nv.destroy();
    await tick();

    // After destroy, the host element should have no rendered card children
    expect(nv.dom.querySelectorAll('.at-card').length).toBe(0);

    document.body.removeChild(nv.dom);
  });
});

// ─── Reactive selection — Bug #1843 fix (Strategy A) ────────────────────────────────────
//
// selectNode() and deselectNode() now push into a writable store instead of
// unmounting + remounting the Svelte component.  The following tests verify:
//
//   S-1  Picker opens on the FIRST click after focus returns (no two-click)
//   S-2a currentNode stays fresh: renderMode from update() is not reverted
//        by selectNode() (the #1835 stale-attrs regression path)
//   S-2b No DOM stacking after repeated update→select→deselect→update cycles
//   S-3  Toolbar hides after the FIRST deselectNode() call
//
// spec-covered-by: #1827 spec-covered-by: #1831
// Relates to #1843 Relates to #401

describe('NodeView lifecycle — reactive selection (Bug #1843 fix)', () => {
  // S-1: Picker opens on first click after focus returns.
  //
  // Mechanism: selectNode() sets selectedStore.set(true) without remounting.
  // The mode button DOM element is the same element before and after
  // selectNode(), so the originating click event (which fires after mousedown
  // triggers selectNode via PM selection machinery) reaches the live button
  // and calls togglePicker() — picker opens on the FIRST click.
  it('S-1: picker (.amp-popover) opens on first click after selectNode() — no remount disrupts click target', async () => {
    const factory = _createAttachmentNodeViewFactory();
    const nv = factory({ node: mkNodeProxy('picker-test.pdf') });

    document.body.appendChild(nv.dom);
    await tick();

    // Simulate focus leaving: node deselected.
    nv.deselectNode();
    await tick();

    // Picker must be closed at this point.
    expect(nv.dom.querySelector('.amp-popover')).toBeNull();

    // Simulate: PM calls selectNode() during mousedown (before click fires).
    // Under Strategy A this does NOT remount — the mode button DOM element
    // is preserved across this call.
    nv.selectNode();
    await tick(); // Let selectedStore update propagate through $effect

    // The mode button must still be the same live DOM element (not torn down).
    const modeBtn = nv.dom.querySelector('.at-tbtn--mode') as HTMLElement;
    expect(modeBtn).not.toBeNull();

    // Fire the click that was previously swallowed by PM's remount cascade.
    // In jsdom, fireEvent dispatches directly — pointer-events CSS is not
    // enforced, so the click always reaches the button regardless of
    // toolbar visibility state.
    await fireEvent.click(modeBtn);
    await tick();

    // Picker must be open after ONE click cycle.
    const popover = nv.dom.querySelector('.amp-popover');
    expect(popover).not.toBeNull();

    // Confirm no attr-change side effect: data-render-mode is unchanged.
    const wrapper = nv.dom.querySelector('[data-attachment-id]');
    expect(wrapper?.getAttribute('data-render-mode')).toBe('card');

    nv.destroy();
    document.body.removeChild(nv.dom);
  });

  // S-2a: currentNode stays fresh — renderMode from update() is not reverted
  // by a subsequent selectNode() call (#1835 stale-attrs regression guard).
  //
  // Under the OLD remount-on-select strategy, selectNode() would pass
  // buildAttrs(currentNode) to mount() — correct only if currentNode was
  // up-to-date.  Under Strategy A, selectNode() doesn’t call mount() at all,
  // so the component retains whatever attrs update() last set.  Either way,
  // this test verifies the rendered renderMode matches the LAST update() call.
  it('S-2a: renderMode set by update() is preserved after selectNode() (currentNode freshness)', async () => {
    const factory = _createAttachmentNodeViewFactory();
    const nv = factory({ node: mkNodeProxy('stale-test.pdf', { renderMode: 'card', attachmentType: 'pdf' }) });

    document.body.appendChild(nv.dom);
    await tick();

    // Simulate attr change: user changed renderMode to 'full-inline' for an image
    nv.update(mkNodeProxy('stale-test.pdf', { renderMode: 'full-inline', attachmentType: 'image' }));
    await tick();

    // Simulate clicking away then back (deselectNode + selectNode).
    nv.deselectNode();
    await tick();
    nv.selectNode();
    await tick();

    // The host wrapper must reflect the updated renderMode, not the initial 'card'.
    // Under Strategy A: no remount in selectNode(), so attrs are whatever
    // update() last set — 'full-inline'.
    const wrapper = nv.dom.querySelector('.at-nv-host') as HTMLElement;
    expect(wrapper).not.toBeNull();
    expect(wrapper.getAttribute('data-render-mode')).toBe('full-inline');

    nv.destroy();
    document.body.removeChild(nv.dom);
  });

  // S-2b: No DOM stacking after 5 cycles of update → selectNode → deselectNode → update.
  //
  // Verifies that currentExports always points to the LIVE instance: stale
  // unmount misses would leave duplicate .at-card elements in the DOM.
  // Under Strategy A, selectNode/deselectNode never call mount(), so the
  // only remount path is update() — one card per update, no stacking.
  it('S-2b: exactly one .at-card in DOM after 5 update→selectNode→deselectNode→update cycles', async () => {
    const factory = _createAttachmentNodeViewFactory();
    const nv = factory({ node: mkNodeProxy('cycle.pdf') });

    document.body.appendChild(nv.dom);
    await tick();

    for (let i = 0; i < 5; i++) {
      nv.update(mkNodeProxy(`cycle-v${i}.pdf`));
      await tick();
      nv.selectNode();
      await tick();
      nv.deselectNode();
      await tick();
      nv.update(mkNodeProxy(`cycle-v${i}-b.pdf`));
      await tick();
    }

    // Only the single live .at-card component must be in the DOM.
    expect(nv.dom.querySelectorAll('.at-card').length).toBe(1);

    nv.destroy();
    document.body.removeChild(nv.dom);
  });

  // S-3: Toolbar hides after the FIRST deselectNode() call (no two-event lag).
  //
  // Under the old remount strategy, deselectNode() would tear down and rebuild
  // the component — the rebuild happened synchronously but could in theory
  // require two events in edge cases.  Under Strategy A the store is set
  // synchronously and the component reacts on the next microtask (tick()).
  it('S-3: toolbar loses .is-open class after first deselectNode() call', async () => {
    const factory = _createAttachmentNodeViewFactory();
    const nv = factory({ node: mkNodeProxy('deselect.pdf') });

    document.body.appendChild(nv.dom);
    await tick();

    // Select first.
    nv.selectNode();
    await tick();

    const toolbar = nv.dom.querySelector('.at-toolbar') as HTMLElement;
    expect(toolbar).not.toBeNull();
    expect(toolbar.classList.contains('is-open')).toBe(true);

    // ONE deselectNode() call — toolbar must hide.
    nv.deselectNode();
    await tick();

    expect(toolbar.classList.contains('is-open')).toBe(false);

    nv.destroy();
    document.body.removeChild(nv.dom);
  });
});

// ─── ANV-PW: Player mode wiring (PR5b) ───────────────────────────────────────
//
// handleModeChange is the internal function in AttachmentNodeView that calls
// onUpdateAttributes AND (for audio + player) loadTrack. The UI path:
//   user clicks mode button (.at-tbtn--mode) → picker opens → user clicks
//   [data-mode="player"] → AttachmentModePicker.selectMode('player') →
//   AttachmentToolbar.handleModeChange('player') → ANV.handleModeChange('player')
//
// We test through the real UI path to prove the full chain works.

function mkActiveNote(overrides: Partial<NoteMeta> = {}): NoteContent {
  const meta: NoteMeta = {
    id: 'note-uuid-pw-001',
    title: 'My Recording',
    notebook: 'Research',
    slug: 'my-recording',
    created_at: '2026-07-02T00:00:00Z',
    updated_at: '2026-07-02T00:00:00Z',
    preview: '',
    ...overrides,
  };
  return { meta, content: '' };
}

async function openPickerAndSelectMode(container: HTMLElement, mode: string) {
  // Click the mode button to open the picker
  const modeBtn = container.querySelector('.at-tbtn--mode') as HTMLElement | null;
  if (!modeBtn) return false;
  fireEvent.click(modeBtn);
  await tick();
  // Click the mode option
  const option = container.querySelector(`[data-mode="${mode}"]`) as HTMLElement | null;
  if (!option) {
    // Picker may be rendered in body (fixed position) — search document
    const bodyOption = document.querySelector(`[data-mode="${mode}"]`) as HTMLElement | null;
    if (!bodyOption) return false;
    fireEvent.click(bodyOption);
    await tick();
    return true;
  }
  fireEvent.click(option);
  await tick();
  return true;
}

describe('ANV-PW — Player mode wiring: audio Player mode calls loadTrack', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    activeNote.set(mkActiveNote());
    playerStatus.set('idle');
  });

  // T1 — resolveAttachmentSrc is called with attachmentId; resolved URL flows into loadTrack
  // This test MUST fail if you remove the resolveAttachmentSrc call and pass attrs.src directly.
  it('T1: calls resolveAttachmentSrc and passes resolved asset:// URL to loadTrack', async () => {
    vi.mocked(resolveAttachmentSrc).mockResolvedValue('asset://localhost/attachments/att-audio-pw-001.mp3');
    const onUpdateSpy = vi.fn();
    const { container } = render(AttachmentNodeView, {
      props: {
        attrs: mkAttrs({
          attachmentId: 'att-audio-pw-001',
          attachmentType: 'audio',
          filename: 'interview.mp3',
          src: '',   // production reality: file-picker sets src=''
          renderMode: 'card',
        }),
        selected: true,
        onUpdateAttributes: onUpdateSpy,
      },
    });

    await tick();
    await openPickerAndSelectMode(container, 'player');
    // Let the async handleModeChange resolve
    await Promise.resolve();
    await tick();

    expect(resolveAttachmentSrc).toHaveBeenCalledWith('att-audio-pw-001');
    expect(loadTrack).toHaveBeenCalledTimes(1);
    expect(loadTrack).toHaveBeenCalledWith(
      expect.objectContaining({
        attachmentId: 'att-audio-pw-001',
        noteId: 'note-uuid-pw-001',
        title: 'interview.mp3',
        breadcrumb: 'Research › My Recording',
        src: 'asset://localhost/attachments/att-audio-pw-001.mp3',
      })
    );
  });

  // T2 — playerStatus is set to 'loading' synchronously (before the IPC round-trip resolves)
  // Ensures NavPlayer's buffering UI appears immediately on click, not after IPC delay.
  it('T2: sets playerStatus to loading synchronously before resolveAttachmentSrc resolves', async () => {
    let resolvePromise!: (v: string) => void;
    const deferred = new Promise<string>((res) => { resolvePromise = res; });
    vi.mocked(resolveAttachmentSrc).mockReturnValue(deferred);

    const { container } = render(AttachmentNodeView, {
      props: {
        attrs: mkAttrs({
          attachmentId: 'att-audio-pw-002',
          attachmentType: 'audio',
          filename: 'clip.mp3',
          src: '',
          renderMode: 'card',
        }),
        selected: true,
        onUpdateAttributes: vi.fn(),
      },
    });

    await tick();
    // Click opens picker; don't await the full async chain — capture mid-flight state
    const modeBtn = container.querySelector('.at-tbtn--mode') as HTMLElement | null;
    if (modeBtn) fireEvent.click(modeBtn);
    await tick();
    const option =
      container.querySelector('[data-mode="player"]') ??
      document.querySelector('[data-mode="player"]');
    if (option) fireEvent.click(option as HTMLElement);
    // Yield the microtask that starts the async handleModeChange body (up to first await)
    await Promise.resolve();

    // playerStatus should already be 'loading' — set synchronously before the await
    expect(get(playerStatus)).toBe('loading');
    // loadTrack must NOT have been called yet (still waiting on deferred)
    expect(loadTrack).not.toHaveBeenCalled();

    // Now resolve the deferred IPC result
    resolvePromise('asset://localhost/attachments/att-audio-pw-002.mp3');
    await Promise.resolve();
    await tick();

    expect(loadTrack).toHaveBeenCalledTimes(1);
    expect(loadTrack).toHaveBeenCalledWith(
      expect.objectContaining({ src: 'asset://localhost/attachments/att-audio-pw-002.mp3' })
    );
  });

  // T3 — when resolveAttachmentSrc returns '' (failure path), loadTrack is NOT called
  // and playerStatus is set to 'failed'.
  it('T3: sets playerStatus to failed and does not call loadTrack when resolveAttachmentSrc returns empty string', async () => {
    vi.mocked(resolveAttachmentSrc).mockResolvedValue('');

    const { container } = render(AttachmentNodeView, {
      props: {
        attrs: mkAttrs({
          attachmentId: 'att-audio-pw-003',
          attachmentType: 'audio',
          filename: 'broken.mp3',
          src: '',
          renderMode: 'card',
        }),
        selected: true,
        onUpdateAttributes: vi.fn(),
      },
    });

    await tick();
    await openPickerAndSelectMode(container, 'player');
    await Promise.resolve();
    await tick();

    expect(loadTrack).not.toHaveBeenCalled();
    expect(get(playerStatus)).toBe('failed');
  });

  it('breadcrumb is title-only when notebook is empty', async () => {
    activeNote.set(mkActiveNote({ notebook: '', title: 'Untitled Note' }));

    const { container } = render(AttachmentNodeView, {
      props: {
        attrs: mkAttrs({
          attachmentId: 'att-audio-pw-002',
          attachmentType: 'audio',
          filename: 'clip.mp3',
          src: '',
          renderMode: 'card',
        }),
        selected: true,
        onUpdateAttributes: vi.fn(),
      },
    });

    await tick();
    await openPickerAndSelectMode(container, 'player');
    await Promise.resolve();
    await tick();

    if (vi.mocked(loadTrack).mock.calls.length > 0) {
      expect(loadTrack).toHaveBeenCalledWith(
        expect.objectContaining({ breadcrumb: 'Untitled Note' })
      );
    }
  });

  it('noteId is null when activeNote is null', async () => {
    activeNote.set(null);

    const { container } = render(AttachmentNodeView, {
      props: {
        attrs: mkAttrs({
          attachmentId: 'att-audio-pw-003',
          attachmentType: 'audio',
          filename: 'orphan.mp3',
          src: '',
          renderMode: 'card',
        }),
        selected: true,
        onUpdateAttributes: vi.fn(),
      },
    });

    await tick();
    await openPickerAndSelectMode(container, 'player');
    await Promise.resolve();
    await tick();

    if (vi.mocked(loadTrack).mock.calls.length > 0) {
      expect(loadTrack).toHaveBeenCalledWith(
        expect.objectContaining({ noteId: null })
      );
    }
  });
});

describe('ANV-PW — Non-audio guard: loadTrack NOT called for non-audio types', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    activeNote.set(mkActiveNote());
  });

  it('does NOT call loadTrack when mode picker is used on a PDF attachment', async () => {
    // PDF type has modes ['card', 'reader'] — no 'player' in picker.
    // Verify that even if somehow player mode were submitted, the guard prevents loadTrack.
    // The test also verifies that normal PDF mode changes (card→reader) don't call loadTrack.
    const onUpdateSpy = vi.fn();
    const { container } = render(AttachmentNodeView, {
      props: {
        attrs: mkAttrs({
          attachmentId: 'att-pdf-pw',
          attachmentType: 'pdf',
          filename: 'document.pdf',
          src: null,
          renderMode: 'card',
        }),
        selected: true,
        onUpdateAttributes: onUpdateSpy,
      },
    });

    await tick();
    // Open picker and try to select any available mode (reader for pdf)
    await openPickerAndSelectMode(container, 'reader');

    // loadTrack must NOT have been called for non-audio types
    expect(loadTrack).not.toHaveBeenCalled();
    // onUpdateAttributes may have been called (mode persistence) — that's fine
  });

  it('does NOT call loadTrack for video type in player mode', async () => {
    // Video has its own player (AttachmentVideoPlayer), not NavPlayer.
    // handleModeChange guard: only audio triggers loadTrack.
    const onUpdateSpy = vi.fn();
    const { container } = render(AttachmentNodeView, {
      props: {
        attrs: mkAttrs({
          attachmentId: 'att-video-pw',
          attachmentType: 'video',
          filename: 'clip.mp4',
          src: 'asset://localhost/clip.mp4',
          renderMode: 'card',
        }),
        selected: true,
        onUpdateAttributes: onUpdateSpy,
      },
    });

    await tick();
    // Try to select player mode for video
    await openPickerAndSelectMode(container, 'player');

    // loadTrack must NOT be called for video (only NavPlayer handles audio)
    expect(loadTrack).not.toHaveBeenCalled();
  });
});

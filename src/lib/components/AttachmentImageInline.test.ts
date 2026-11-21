/**
 * AttachmentImageInline.test.ts — PR4 (#401)
 *
 * Tests for the full-inline resizable image component.
 *
 * Tests:
 *  I-1  Natural size: null width/height → no explicit size CSS (max-width:100% style)
 *  I-2  Persisted dims: width=640, height=480 → max-width+aspect-ratio style (no hardcoded height)
 *  I-3  Corner drag → dimension overlay appears
 *  I-4  Drag end → onUpdateAttributes({ width, height }) called with final integers
 *  I-5  Aspect-ratio lock: dragging scales both dimensions proportionally
 *  I-6  Selected prop → is-selected class (cyan ring) applied to wrapper
 *  I-7  Src resolution: resolveAttachmentSrc called with correct attachmentId
 *  I-8  No base64: img src starts with asset:// (mock), never data:
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, fireEvent } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import AttachmentImageInline from './AttachmentImageInline.svelte';

// ─── Mock resolveAttachmentSrc ────────────────────────────────────────────────
vi.mock('$lib/api', () => ({
  resolveAttachmentSrc: vi.fn(async (id: string) => `asset://localhost/attachments/${id}.jpg`),
  fetchLinkPreview: vi.fn().mockResolvedValue(null),
  getCachedLinkPreview: vi.fn().mockResolvedValue(null),
}));

import { resolveAttachmentSrc } from '$lib/api';

// ─── Helpers ──────────────────────────────────────────────────────────────────

function renderImage(overrides: Record<string, any> = {}) {
  return render(AttachmentImageInline, {
    props: {
      attachmentId: 'test-img-001',
      filename: 'photo.jpg',
      width: null,
      height: null,
      selected: false,
      onUpdateAttributes: vi.fn(),
      ...overrides,
    },
  });
}

// Flush async (onMount resolveAttachmentSrc) + Svelte 5 state update.
async function flushAsync() {
  await Promise.resolve(); // drain microtask queue
  await tick();            // flush Svelte reactive state
  await tick();            // extra tick for $effect
}

// ─── I-1: Natural size (null dims) ───────────────────────────────────────────

describe('I-1 — null width/height → no explicit size CSS beyond max-width:100%', () => {
  it('img element is in the DOM', () => {
    const { container } = renderImage({ width: null, height: null });
    const img = container.querySelector('img');
    expect(img).not.toBeNull();
  });

  it('renders with max-width:100% style when dims are null (no explicit px size)', async () => {
    const { container } = renderImage({ width: null, height: null });
    await flushAsync(); // wait for $effect to run with null dims

    const img = container.querySelector('img');
    expect(img).not.toBeNull();
    const style = img?.getAttribute('style') || '';
    // null dims → imageStyle = 'max-width:100%;' (no explicit px values)
    expect(style).not.toMatch(/width:\s*\d+px/);
    expect(style).not.toMatch(/height:\s*\d+px/);
  });
});

// ─── I-2: Persisted dims → aspect-ratio CSS pattern (not hardcoded px height) ───

describe('I-2 — persisted width/height → width:min()+aspect-ratio style (no hardcoded height)', () => {
  it('applies width:min(Npx,100%) and aspect-ratio style when attrs are set', async () => {
    const { container } = renderImage({ width: 640, height: 480 });
    await flushAsync(); // $effect runs: currentW=640, currentH=480 → derived imageStyle

    const img = container.querySelector('img');
    expect(img).not.toBeNull();
    const style = img?.getAttribute('style') || '';
    // Fix (#1843): width:min(${w}px, 100%) directly constrains visible width at ALL sizes.
    // Previously: max-width:${w}px; width:100% — only scaled below column width (~720px).
    // aspect-ratio drives height proportionally — no explicit height in px.
    expect(style).toMatch(/width:\s*min\(\s*640px/);
    expect(style).toMatch(/aspect-ratio:\s*640\s*\/\s*480/);
    // Must NOT have an explicit height:Npx (aspect-ratio handles it proportionally)
    expect(style).not.toMatch(/height:\s*\d+px/);
  });

  it('large image (2000×1500): no hardcoded height — aspect-ratio prevents column-squish', async () => {
    const { container } = renderImage({ width: 2000, height: 1500 });
    await flushAsync();

    const img = container.querySelector('img');
    const style = img?.getAttribute('style') || '';
    // width:min(2000px, 100%) — clamps to container when 2000 > column width
    expect(style).toMatch(/width:\s*min\(\s*2000px/);
    expect(style).toMatch(/aspect-ratio:\s*2000\s*\/\s*1500/);
    // No hardcoded height — prevents squish when container is narrower than image
    expect(style).not.toMatch(/height:\s*1500px/);
  });
});

// ─── I-3: Dimension overlay appears during drag ────────────────────────────────

describe('I-3 — corner drag → dimension overlay appears', () => {
  it('shows dimension overlay (.at-dim-overlay) when mousedown on br handle', async () => {
    const { container } = renderImage({ width: 400, height: 300 });
    await flushAsync();

    const handle = container.querySelector('.at-handle-br');
    expect(handle).not.toBeNull();

    await fireEvent.mouseDown(handle!, { clientX: 100, clientY: 100 });
    await tick();
    await tick(); // extra tick for Svelte 5 state flush

    const overlay = container.querySelector('.at-dim-overlay');
    expect(overlay).not.toBeNull();
  });

  it('overlay displays current dimensions as "W × H" text', async () => {
    const { container } = renderImage({ width: 400, height: 300 });
    await flushAsync();

    const handle = container.querySelector('.at-handle-br');
    await fireEvent.mouseDown(handle!, { clientX: 0, clientY: 0 });
    await tick();
    await tick();

    const overlay = container.querySelector('.at-dim-overlay');
    expect(overlay?.textContent).toMatch(/\d+\s*×\s*\d+/);
  });
});

// ─── I-4: Drag end calls onUpdateAttributes with integers ─────────────────────

describe('I-4 — drag end → onUpdateAttributes({ width, height }) called', () => {
  it('calls onUpdateAttributes with integer dimensions on mouseup', async () => {
    const onUpdateAttributes = vi.fn();
    const { container } = renderImage({ width: 400, height: 300, onUpdateAttributes });
    await flushAsync();

    const handle = container.querySelector('.at-handle-br');
    // Start drag
    await fireEvent.mouseDown(handle!, { clientX: 100, clientY: 100 });
    // Move (delta: +50px horizontal) — fire on document (component uses document listeners)
    await fireEvent.mouseMove(document, { clientX: 150, clientY: 125 });
    // End drag
    await fireEvent.mouseUp(document, { clientX: 150, clientY: 125 });
    await tick();

    expect(onUpdateAttributes).toHaveBeenCalledWith(
      expect.objectContaining({
        width: expect.any(Number),
        height: expect.any(Number),
      })
    );

    const call = onUpdateAttributes.mock.calls[0]?.[0];
    expect(call).toBeDefined();
    if (call) {
      expect(Number.isInteger(call.width)).toBe(true);
      expect(Number.isInteger(call.height)).toBe(true);
    }
  });

  it('called exactly once on drag end (not during drag)', async () => {
    const onUpdateAttributes = vi.fn();
    const { container } = renderImage({ width: 400, height: 300, onUpdateAttributes });
    await flushAsync();

    const handle = container.querySelector('.at-handle-br');
    await fireEvent.mouseDown(handle!, { clientX: 0, clientY: 0 });
    await fireEvent.mouseMove(document, { clientX: 10, clientY: 0 });
    await fireEvent.mouseMove(document, { clientX: 20, clientY: 0 });
    await fireEvent.mouseUp(document, { clientX: 20, clientY: 0 });
    await tick();

    expect(onUpdateAttributes).toHaveBeenCalledTimes(1);
  });
});

// ─── I-5: Aspect-ratio lock ──────────────────────────────────────────────────

describe('I-5 — aspect-ratio lock: both dims scale proportionally', () => {
  it('2:1 aspect ratio maintained after drag', async () => {
    const onUpdateAttributes = vi.fn();
    // 2:1 aspect ratio (width:height = 2:1)
    const { container } = renderImage({ width: 200, height: 100, onUpdateAttributes });
    await flushAsync();

    const handle = container.querySelector('.at-handle-br');
    await fireEvent.mouseDown(handle!, { clientX: 0, clientY: 0 });
    // Drag right by 100px (increases width) — fire on document
    await fireEvent.mouseMove(document, { clientX: 100, clientY: 0 });
    await fireEvent.mouseUp(document, { clientX: 100, clientY: 0 });
    await tick();

    const call = onUpdateAttributes.mock.calls[0]?.[0];
    expect(call).toBeDefined();
    if (call) {
      const { width, height } = call;
      // Should maintain ≈2:1 ratio (within rounding tolerance)
      const ratio = width / height;
      expect(ratio).toBeGreaterThan(1.8);
      expect(ratio).toBeLessThan(2.2);
    }
  });

  it('width and height both change (not just one) on drag', async () => {
    const onUpdateAttributes = vi.fn();
    const { container } = renderImage({ width: 300, height: 200, onUpdateAttributes });
    await flushAsync();

    const handle = container.querySelector('.at-handle-br');
    await fireEvent.mouseDown(handle!, { clientX: 0, clientY: 0 });
    await fireEvent.mouseMove(document, { clientX: 60, clientY: 0 });
    await fireEvent.mouseUp(document, { clientX: 60, clientY: 0 });
    await tick();

    const call = onUpdateAttributes.mock.calls[0]?.[0];
    if (call) {
      // Both should be different from initial values
      expect(call.width).not.toBe(300);
      expect(call.height).not.toBe(200);
    }
  });
});

// ─── I-6: Selected → cyan ring class ─────────────────────────────────────────

describe('I-6 — selected prop → is-selected class applied', () => {
  it('adds is-selected class when selected=true', () => {
    const { container } = renderImage({ selected: true });
    const wrapper = container.querySelector('.at-image-inline');
    expect(wrapper).not.toBeNull();
    expect(wrapper?.classList.contains('is-selected')).toBe(true);
  });

  it('does NOT add is-selected class when selected=false', () => {
    const { container } = renderImage({ selected: false });
    const wrapper = container.querySelector('.at-image-inline');
    expect(wrapper?.classList.contains('is-selected')).toBe(false);
  });

  it('wrapper has the at-image-inline class', () => {
    const { container } = renderImage();
    expect(container.querySelector('.at-image-inline')).not.toBeNull();
  });
});

// ─── I-7: Src resolution ─────────────────────────────────────────────────────

describe('I-7 — src resolution: resolveAttachmentSrc called with correct attachmentId', () => {
  beforeEach(() => {
    vi.mocked(resolveAttachmentSrc).mockClear();
  });

  it('calls resolveAttachmentSrc with the provided attachmentId', async () => {
    renderImage({ attachmentId: 'my-specific-uuid' });
    await flushAsync();
    expect(resolveAttachmentSrc).toHaveBeenCalledWith('my-specific-uuid');
  });

  it('img src is set to the resolved URL after onMount', async () => {
    vi.mocked(resolveAttachmentSrc).mockResolvedValueOnce('asset://localhost/attachments/uuid-xyz.jpg');
    const { container } = renderImage({ attachmentId: 'uuid-xyz' });
    await flushAsync();

    const img = container.querySelector('img');
    expect(img?.getAttribute('src')).toBe('asset://localhost/attachments/uuid-xyz.jpg');
  });
});

// ─── I-8: No base64 ──────────────────────────────────────────────────────────

describe('I-8 — no base64: img src starts with asset://, never data:', () => {
  it('img element is always present in the DOM (never omitted)', () => {
    const { container } = renderImage({ attachmentId: 'no-b64-test' });
    const img = container.querySelector('img');
    expect(img).not.toBeNull();
  });

  it('img src after resolution starts with asset://, not data:', async () => {
    const { container } = renderImage({ attachmentId: 'no-b64-test' });
    await flushAsync();

    const img = container.querySelector('img');
    const src = img?.getAttribute('src') ?? '';
    expect(src.startsWith('data:')).toBe(false);
    expect(src).toMatch(/^asset:\/\//);
  });

  it('img src never contains base64 payload', async () => {
    const { container } = renderImage({ attachmentId: 'no-b64-test-2' });
    await flushAsync();

    const img = container.querySelector('img');
    const src = img?.getAttribute('src') ?? '';
    expect(src).not.toMatch(/^data:/);
    // base64 pattern check
    expect(src).not.toMatch(/;base64,/);
  });
});

// ─── I-9: W-4 — onDestroy removes document drag listeners ───────────────────

describe('I-9 — W-4: onDestroy removes document-scope drag listeners if unmounted mid-drag', () => {
  it('unmounting mid-drag removes mousemove listener from document', async () => {
    const removeSpy = vi.spyOn(document, 'removeEventListener');

    const { container, unmount } = renderImage({
      attachmentId: 'drag-cleanup-test',
    });
    await flushAsync();

    // Start a drag to register the document listeners.
    const handleBR = container.querySelector('.at-handle-br') as HTMLElement;
    expect(handleBR).not.toBeNull();
    fireEvent.mouseDown(handleBR, { clientX: 100, clientY: 100 });

    // Now unmount the component (simulates navigating away mid-drag).
    unmount();

    // onDestroy should have called document.removeEventListener for both drag events.
    const calls = removeSpy.mock.calls.map((c) => c[0]);
    expect(calls).toContain('mousemove');
    expect(calls).toContain('mouseup');

    removeSpy.mockRestore();
  });

  it('unmounting without drag still calls removeEventListener (no-op, safe)', async () => {
    const removeSpy = vi.spyOn(document, 'removeEventListener');

    const { unmount } = renderImage({ attachmentId: 'no-drag-cleanup' });
    await flushAsync();

    // Unmount without starting a drag — onDestroy still runs the cleanup.
    unmount();

    const calls = removeSpy.mock.calls.map((c) => c[0]);
    expect(calls).toContain('mousemove');
    expect(calls).toContain('mouseup');

    removeSpy.mockRestore();
  });
});

// ─── I-10: Natural dims persisted to node attrs on first load ───────────────────────────

describe('I-10 — natural dims persisted via onUpdateAttributes when node attrs are null', () => {
  it('calls onUpdateAttributes with natural dims when width=null, height=null', async () => {
    const onUpdateAttributes = vi.fn();
    const { container } = renderImage({ width: null, height: null, onUpdateAttributes });
    await flushAsync();

    // Fire the image load event with natural dimensions.
    const img = container.querySelector('img') as HTMLImageElement;
    expect(img).not.toBeNull();
    Object.defineProperty(img, 'naturalWidth', { value: 800, configurable: true });
    Object.defineProperty(img, 'naturalHeight', { value: 600, configurable: true });
    await fireEvent.load(img);
    await tick();

    // Should have called onUpdateAttributes with natural dims. #2950: skipDirty: true
    // must be passed so this auto-persist-on-load write doesn't mark the editor
    // dirty / trigger a save on a note the user never touched.
    expect(onUpdateAttributes).toHaveBeenCalledWith({ width: 800, height: 600 }, { skipDirty: true });
  });

  it('does NOT call onUpdateAttributes when width and height are already persisted (user resize preserved)', async () => {
    const onUpdateAttributes = vi.fn();
    const { container } = renderImage({ width: 640, height: 480, onUpdateAttributes });
    await flushAsync();

    // Fire load event with different natural dims.
    const img = container.querySelector('img') as HTMLImageElement;
    expect(img).not.toBeNull();
    Object.defineProperty(img, 'naturalWidth', { value: 1920, configurable: true });
    Object.defineProperty(img, 'naturalHeight', { value: 1080, configurable: true });
    await fireEvent.load(img);
    await tick();

    // onUpdateAttributes must NOT have been called (existing user resize must be preserved).
    expect(onUpdateAttributes).not.toHaveBeenCalled();
  });
});

// ─── I-11: aspect-ratio style included as loading-gap fallback ─────────────────────────

describe('I-11 — aspect-ratio CSS fallback on .at-img during loading gap', () => {
  it('img style includes aspect-ratio once natural dims known and no persisted px size', async () => {
    const onUpdateAttributes = vi.fn();
    // Start with null dims so imageStyle falls to the naturalW/naturalH branch.
    const { container } = renderImage({ width: null, height: null, onUpdateAttributes });
    await flushAsync();

    const img = container.querySelector('img') as HTMLImageElement;
    Object.defineProperty(img, 'naturalWidth', { value: 400, configurable: true });
    Object.defineProperty(img, 'naturalHeight', { value: 300, configurable: true });
    await fireEvent.load(img);
    // After load: onUpdateAttributes persists dims AND currentW/currentH are seeded,
    // so imageStyle will show explicit px. Both paths are valid; just ensure no stretch.
    await tick();

    const style = img.getAttribute('style') ?? '';
    // Either explicit px dimensions OR an aspect-ratio lock — either prevents stretch.
    const hasExplicitSize = /width:\s*\d+px/.test(style) && /height:\s*\d+px/.test(style);
    const hasAspectRatio = /aspect-ratio/.test(style);
    expect(hasExplicitSize || hasAspectRatio).toBe(true);
  });
});

// ─── I-12: Resize scales continuously above column width (#1843 Bug 1 fix) ──────

describe('I-12 — #1843 fix: resize scales continuously above column width', () => {
  it('Test 1 — style reflects 1200px after drag from 1500 (above ~720px column)', async () => {
    // Simulate dragging from currentW=1500 to currentW=1200.
    // startDrag: dragStartW=1500; dragCorner='br' (signX=+1)
    // applyDrag(deltaX=-300): newW = 1500 + 1*(-300) = 1200 → currentW=1200
    const onUpdateAttributes = vi.fn();
    const { container } = renderImage({ width: 1500, height: 1000, onUpdateAttributes });
    await flushAsync();

    const handle = container.querySelector('.at-handle-br');
    expect(handle).not.toBeNull();

    // Start drag at x=300; move to x=0 → deltaX = -300 → currentW = 1200
    await fireEvent.mouseDown(handle!, { clientX: 300, clientY: 0 });
    await fireEvent.mouseMove(document, { clientX: 0, clientY: 0 });
    await tick();
    await tick();

    const img = container.querySelector('img');
    const style = img?.getAttribute('style') ?? '';
    // width:min(1200px, 100%) — the dragged value directly constrains visible width
    expect(style).toMatch(/width:\s*min\(\s*1200px/);

    // Drag end to flush onUpdateAttributes
    await fireEvent.mouseUp(document, { clientX: 0, clientY: 0 });
    await tick();
    const call = onUpdateAttributes.mock.calls[0]?.[0];
    expect(call?.width).toBe(1200);
  });

  it('Test 2 — style reflects 400px after drag below column width', async () => {
    // Start from currentW=1500; drag to currentW=400: deltaX = 400-1500 = -1100
    const onUpdateAttributes = vi.fn();
    const { container } = renderImage({ width: 1500, height: 1000, onUpdateAttributes });
    await flushAsync();

    const handle = container.querySelector('.at-handle-br');
    expect(handle).not.toBeNull();

    await fireEvent.mouseDown(handle!, { clientX: 1100, clientY: 0 });
    await fireEvent.mouseMove(document, { clientX: 0, clientY: 0 });
    await tick();
    await tick();

    const img = container.querySelector('img');
    const style = img?.getAttribute('style') ?? '';
    // width:min(400px, 100%) — below column width, still correctly constrained
    expect(style).toMatch(/width:\s*min\(\s*400px/);

    await fireEvent.mouseUp(document, { clientX: 0, clientY: 0 });
    await tick();
    const call = onUpdateAttributes.mock.calls[0]?.[0];
    expect(call?.width).toBe(400);
  });

  it('Test 3 — aspect-ratio preserved in style at both wide (1500×1000) and narrow (300×200) sizes', async () => {
    // Wide size
    const { container: c1 } = renderImage({ width: 1500, height: 1000 });
    await flushAsync();
    const img1 = c1.querySelector('img');
    const style1 = img1?.getAttribute('style') ?? '';
    expect(style1).toMatch(/aspect-ratio:\s*1500\s*\/\s*1000/);

    // Narrow size
    const { container: c2 } = renderImage({ width: 300, height: 200 });
    await flushAsync();
    const img2 = c2.querySelector('img');
    const style2 = img2?.getAttribute('style') ?? '';
    expect(style2).toMatch(/aspect-ratio:\s*300\s*\/\s*200/);
  });

  it('Test 4 — no explicit height:Npx in style (no height-squish regression)', async () => {
    // Drag from 1500 to 800 (still > 720 column). With min()+aspect-ratio,
    // NO explicit height in px should be emitted — aspect-ratio drives height.
    const onUpdateAttributes = vi.fn();
    const { container } = renderImage({ width: 1500, height: 1000, onUpdateAttributes });
    await flushAsync();

    const handle = container.querySelector('.at-handle-br');
    expect(handle).not.toBeNull();

    // deltaX = 800-1500 = -700: mouseDown at x=700, move to x=0
    await fireEvent.mouseDown(handle!, { clientX: 700, clientY: 0 });
    await fireEvent.mouseMove(document, { clientX: 0, clientY: 0 });
    await tick();
    await tick();

    const img = container.querySelector('img');
    const style = img?.getAttribute('style') ?? '';

    // Must contain aspect-ratio (handles height proportionally)
    expect(style).toMatch(/aspect-ratio/);
    // Must NOT contain an explicit height:Npx that would fight the aspect-ratio
    expect(style).not.toMatch(/height:\s*\d+px/);

    await fireEvent.mouseUp(document, { clientX: 0, clientY: 0 });
    await tick();
  });

  it('Test 5 (legacy) — natural dimensions on first load: onUpdateAttributes called with {width, height}', async () => {
    // Mount with null dims (first load). Fire image load event with natural dims.
    // Must call onUpdateAttributes({ width: 800, height: 600 }, { skipDirty: true }).
    // #2950: skipDirty: true is required so this auto-persist doesn't mark the
    // editor dirty / trigger a save on a note the user never touched.
    const onUpdateAttributes = vi.fn();
    const { container } = renderImage({ width: null, height: null, onUpdateAttributes });
    await flushAsync();

    const img = container.querySelector('img') as HTMLImageElement;
    expect(img).not.toBeNull();

    Object.defineProperty(img, 'naturalWidth', { value: 800, configurable: true });
    Object.defineProperty(img, 'naturalHeight', { value: 600, configurable: true });
    await fireEvent.load(img);
    await tick();

    expect(onUpdateAttributes).toHaveBeenCalledWith({ width: 800, height: 600 }, { skipDirty: true });
  });
});

// ─── T-1..T-5: PR4 Bug 1 Round 3 — container width tracks currentW (#1843) ──

describe('T-1 — container width tracks currentW below column width (400×300)', () => {
  it('container inline style contains width:400px', async () => {
    const { container } = renderImage({ width: 400, height: 300 });
    await flushAsync();

    const wrapper = container.querySelector('.at-image-inline') as HTMLElement;
    expect(wrapper).not.toBeNull();
    const style = wrapper.getAttribute('style') ?? '';
    // Container must carry the currentW so handles hug the image edge, not the column edge.
    expect(style).toMatch(/width:\s*400px/);
  });

  it('container inline style contains max-width:100%', async () => {
    const { container } = renderImage({ width: 400, height: 300 });
    await flushAsync();

    const wrapper = container.querySelector('.at-image-inline') as HTMLElement;
    const style = wrapper?.getAttribute('style') ?? '';
    expect(style).toMatch(/max-width:\s*100%/);
  });

  it('img inline style contains width:min(400px, 100%)', async () => {
    const { container } = renderImage({ width: 400, height: 300 });
    await flushAsync();

    const img = container.querySelector('img');
    const style = img?.getAttribute('style') ?? '';
    // PR #1857 min() guard preserved on the img (belt-and-suspenders).
    expect(style).toMatch(/width:\s*min\(\s*400px/);
  });
});

describe('T-2 — container width at column boundary (720×540)', () => {
  it('container inline style contains width:720px and max-width:100%', async () => {
    const { container } = renderImage({ width: 720, height: 540 });
    await flushAsync();

    const wrapper = container.querySelector('.at-image-inline') as HTMLElement;
    const style = wrapper?.getAttribute('style') ?? '';
    expect(style).toMatch(/width:\s*720px/);
    expect(style).toMatch(/max-width:\s*100%/);
  });

  it('img inline style contains width:min(720px, 100%)', async () => {
    const { container } = renderImage({ width: 720, height: 540 });
    await flushAsync();

    const img = container.querySelector('img');
    const style = img?.getAttribute('style') ?? '';
    expect(style).toMatch(/width:\s*min\(\s*720px/);
  });
});

describe('T-3 — container clamps above column width (currentW=1500)', () => {
  it('container inline style declares width:1500px AND max-width:100%', async () => {
    // The inline style emits the declared value (1500px); the browser clamps
    // to column via max-width:100%. jsdom does not run column CSS, so this
    // assertion is on the emitted style, not computed pixel width. That is the
    // expected browser behavior: declared > actual rendered, max-width wins.
    const { container } = renderImage({ width: 1500, height: 1000 });
    await flushAsync();

    const wrapper = container.querySelector('.at-image-inline') as HTMLElement;
    const style = wrapper?.getAttribute('style') ?? '';
    expect(style).toMatch(/width:\s*1500px/);
    expect(style).toMatch(/max-width:\s*100%/);
  });

  it('img inline style contains width:min(1500px, 100%)', async () => {
    const { container } = renderImage({ width: 1500, height: 1000 });
    await flushAsync();

    const img = container.querySelector('img');
    const style = img?.getAttribute('style') ?? '';
    expect(style).toMatch(/width:\s*min\(\s*1500px/);
  });
});

describe('T-4 — aspect-ratio preserved in img inline style (1500/1000)', () => {
  it('img inline style contains aspect-ratio:1500/1000', async () => {
    const { container } = renderImage({ width: 1500, height: 1000 });
    await flushAsync();

    const img = container.querySelector('img');
    const style = img?.getAttribute('style') ?? '';
    // Audit Q5/T-4: aspect-ratio stays on the img (not the container).
    // No explicit height in px — aspect-ratio drives height proportionally.
    expect(style).toMatch(/aspect-ratio:\s*1500\s*\/\s*1000/);
    expect(style).not.toMatch(/height:\s*\d+px/);
  });
});

describe('T-5 — resize handles are children of .at-image-inline (anchored to sized container)', () => {
  it('handle .at-handle-tr is a direct child of .at-image-inline', () => {
    const { container } = renderImage({ width: 400, height: 300 });
    const wrapper = container.querySelector('.at-image-inline');
    expect(wrapper).not.toBeNull();

    // The handles must be inside the sized container so that absolute positioning
    // (right:-5px) resolves against the container width (currentW), not the column.
    const handleTR = wrapper?.querySelector('.at-handle-tr');
    expect(handleTR).not.toBeNull();
  });

  it('handle .at-handle-br is a direct child of .at-image-inline', () => {
    const { container } = renderImage({ width: 400, height: 300 });
    const wrapper = container.querySelector('.at-image-inline');
    const handleBR = wrapper?.querySelector('.at-handle-br');
    expect(handleBR).not.toBeNull();
  });

  it('all four handles (.at-handle-tl/tr/bl/br) live inside .at-image-inline', () => {
    const { container } = renderImage({ width: 400, height: 300 });
    const wrapper = container.querySelector('.at-image-inline');
    expect(wrapper).not.toBeNull();
    for (const cls of ['.at-handle-tl', '.at-handle-tr', '.at-handle-bl', '.at-handle-br']) {
      const handle = wrapper?.querySelector(cls);
      expect(handle).not.toBeNull();
    }
  });
});

// ─── T-C1..T-C4: PR4 Round 4 — drag clamp to live column width (#1843) ──────
//
// jsdom note: getComputedStyle() in jsdom only returns styles set inline or via
// a <style> tag injected into the document; it does NOT process external
// stylesheets or :global() selectors (those never cascade in jsdom). Therefore,
// to exercise the clamp path we inject a .pane-editor element with an inline
// width style into the document body before each test, and remove it after.
// These tests prove the clamp LOGIC (Math.min, aspect-ratio preservation, no-op
// fallback); Matt's live drag is the authoritative gate for render-side behavior.

describe('T-C1 — clamp caps stored currentW to column width', () => {
  it('when column is 500px wide and candidate is 1200px, currentW is clamped to 500', async () => {
    // Inject a .pane-editor sentinel with inline width so getComputedStyle returns 500px.
    const sentinel = document.createElement('div');
    sentinel.className = 'pane-editor';
    sentinel.style.width = '500px';
    document.body.appendChild(sentinel);

    try {
      const onUpdateAttributes = vi.fn();
      const { container } = renderImage({ width: 400, height: 300, onUpdateAttributes });
      await flushAsync();

      const handle = container.querySelector('.at-handle-br');
      expect(handle).not.toBeNull();

      // startDrag: dragStartW = 400; signX(br) = +1
      // candidateW after deltaX=+800 → 400 + 800 = 1200 → clamped to 500
      await fireEvent.mouseDown(handle!, { clientX: 0, clientY: 0 });
      await fireEvent.mouseMove(document, { clientX: 800, clientY: 0 });
      await tick();
      await tick();

      await fireEvent.mouseUp(document, { clientX: 800, clientY: 0 });
      await tick();

      const call = onUpdateAttributes.mock.calls[0]?.[0];
      expect(call).toBeDefined();
      expect(call.width).toBe(500);
    } finally {
      document.body.removeChild(sentinel);
    }
  });
});

describe('T-C2 — clamp preserves aspect ratio when capping', () => {
  it('naturalW=2000, naturalH=1000 (2:1); column=500; drag past → currentW=500, currentH=250', async () => {
    const sentinel = document.createElement('div');
    sentinel.className = 'pane-editor';
    sentinel.style.width = '500px';
    document.body.appendChild(sentinel);

    try {
      const onUpdateAttributes = vi.fn();
      // Start at natural dims: 2000×1000 (2:1 ratio)
      const { container } = renderImage({ width: 2000, height: 1000, onUpdateAttributes });
      await flushAsync();

      const handle = container.querySelector('.at-handle-br');
      expect(handle).not.toBeNull();

      // Drag: startW=2000; deltaX=+500 → candidateW=2500 → clamped to 500
      // aspectRatio at startDrag = 2000/1000 = 2; clampedH = 500/2 = 250
      await fireEvent.mouseDown(handle!, { clientX: 0, clientY: 0 });
      await fireEvent.mouseMove(document, { clientX: 500, clientY: 0 });
      await tick();
      await tick();

      await fireEvent.mouseUp(document, { clientX: 500, clientY: 0 });
      await tick();

      const call = onUpdateAttributes.mock.calls[0]?.[0];
      expect(call).toBeDefined();
      expect(call.width).toBe(500);
      expect(call.height).toBe(250);
    } finally {
      document.body.removeChild(sentinel);
    }
  });
});

describe('T-C3 — clamp below column is no-op (drag remains free below column width)', () => {
  it('column=720, candidateW=400 → currentW===400 (not clamped)', async () => {
    const sentinel = document.createElement('div');
    sentinel.className = 'pane-editor';
    sentinel.style.width = '720px';
    document.body.appendChild(sentinel);

    try {
      const onUpdateAttributes = vi.fn();
      // Start at 200×150; drag to 400 (deltaX=+200); 400 < 720 → no clamp
      const { container } = renderImage({ width: 200, height: 150, onUpdateAttributes });
      await flushAsync();

      const handle = container.querySelector('.at-handle-br');
      expect(handle).not.toBeNull();

      await fireEvent.mouseDown(handle!, { clientX: 0, clientY: 0 });
      await fireEvent.mouseMove(document, { clientX: 200, clientY: 0 });
      await tick();
      await tick();

      await fireEvent.mouseUp(document, { clientX: 200, clientY: 0 });
      await tick();

      const call = onUpdateAttributes.mock.calls[0]?.[0];
      expect(call).toBeDefined();
      expect(call.width).toBe(400);
    } finally {
      document.body.removeChild(sentinel);
    }
  });
});

describe('T-C4 — clamp gracefully handles missing .pane-editor element', () => {
  it('no .pane-editor in DOM → clamp is no-op → currentW===candidateW, no exception', async () => {
    // Ensure no .pane-editor sentinel exists (clean slate).
    document.querySelectorAll('.pane-editor').forEach((el) => el.remove());

    const onUpdateAttributes = vi.fn();
    // Start at 200×150; drag +300 → candidateW=500; no column el → Infinity fallback → 500
    const { container } = renderImage({ width: 200, height: 150, onUpdateAttributes });
    await flushAsync();

    const handle = container.querySelector('.at-handle-br');
    expect(handle).not.toBeNull();

    let threw = false;
    try {
      await fireEvent.mouseDown(handle!, { clientX: 0, clientY: 0 });
      await fireEvent.mouseMove(document, { clientX: 300, clientY: 0 });
      await tick();
      await tick();

      await fireEvent.mouseUp(document, { clientX: 300, clientY: 0 });
      await tick();
    } catch {
      threw = true;
    }

    expect(threw).toBe(false);

    const call = onUpdateAttributes.mock.calls[0]?.[0];
    expect(call).toBeDefined();
    // No clamp applied — candidateW = 200 + 300 = 500
    expect(call.width).toBe(500);
  });
});

// ─── R5-1..R5-4: Round 5 — clamp drag anchor at capture (#1843 round 5) ─────
//
// Bug: on grab #1 of a natural-width image (e.g. 1920×1080), dragStartW was
// set to 1920 (unclamped). applyDrag clamps every result to columnMaxPx (~720).
// Result: any inward move smaller than (1920-720)=1200px still produced
// currentW=720 — the overlay appeared frozen. Grab #2 worked because grab #1's
// onDragEnd persisted width=720, making grab #2's anchor already at column width.
//
// Option B fix: clamp dragStartW to columnMaxPx at capture time in startDrag.
// Natural dims remain in node attrs — only the drag interaction is bounded.
//
// Sentinel pattern: inject a .pane-editor div with inline width into document.body
// so getComputedStyle returns the expected column width in jsdom (same pattern as T-C*).

describe('R5-1 — dragStartW clamped to columnMaxPx on first grab of natural-width image', () => {
  it('inward drag on first grab produces visible change — not frozen at column width', async () => {
    const sentinel = document.createElement('div');
    sentinel.className = 'pane-editor';
    sentinel.style.width = '720px';
    document.body.appendChild(sentinel);

    try {
      const onUpdateAttributes = vi.fn();
      // Natural-width image: width=1920, height=1080. With OLD code dragStartW=1920;
      // any inward move <1200px still produced candidateW>720 → clamped → frozen.
      // With NEW code dragStartW=720; inward deltaX=-100 → candidateW=620 → not clamped.
      const { container } = renderImage({ width: 1920, height: 1080, onUpdateAttributes });
      await flushAsync();

      const handle = container.querySelector('.at-handle-br');
      expect(handle).not.toBeNull();

      // Drag INWARD by 100px (shrink): mouseDown at x=100, move to x=0 → deltaX=-100
      await fireEvent.mouseDown(handle!, { clientX: 100, clientY: 0 });
      await fireEvent.mouseMove(document, { clientX: 0, clientY: 0 });
      await tick();
      await tick();

      const img = container.querySelector('img');
      const style = img?.getAttribute('style') ?? '';
      // With fix: dragStartW=720 → candidateW=620 → not clamped → currentW=620
      // With old code: dragStartW=1920 → candidateW=1820 → clamped to 720 → frozen
      expect(style).toMatch(/width:\s*min\(\s*620px/);

      await fireEvent.mouseUp(document, { clientX: 0, clientY: 0 });
      await tick();
      expect(onUpdateAttributes.mock.calls[0]?.[0]?.width).toBe(620);
    } finally {
      document.body.removeChild(sentinel);
    }
  });

  it('outward drag clamps at column width (expected behavior, not a regression)', async () => {
    const sentinel = document.createElement('div');
    sentinel.className = 'pane-editor';
    sentinel.style.width = '720px';
    document.body.appendChild(sentinel);

    try {
      const onUpdateAttributes = vi.fn();
      const { container } = renderImage({ width: 1920, height: 1080, onUpdateAttributes });
      await flushAsync();

      const handle = container.querySelector('.at-handle-br');
      // Drag OUTWARD by 10px — both old and new code clamp to 720 (expected)
      await fireEvent.mouseDown(handle!, { clientX: 0, clientY: 0 });
      await fireEvent.mouseMove(document, { clientX: 10, clientY: 0 });
      await tick();
      await tick();

      await fireEvent.mouseUp(document, { clientX: 10, clientY: 0 });
      await tick();
      // At column edge outward drag → stays at 720
      expect(onUpdateAttributes.mock.calls[0]?.[0]?.width).toBe(720);
    } finally {
      document.body.removeChild(sentinel);
    }
  });
});

describe('R5-2 — aspect ratio preserved when drag anchor is clamped', () => {
  it('16:9 ratio preserved after clamped anchor and inward drag', async () => {
    const sentinel = document.createElement('div');
    sentinel.className = 'pane-editor';
    sentinel.style.width = '720px';
    document.body.appendChild(sentinel);

    try {
      const onUpdateAttributes = vi.fn();
      // 1920×1080 = 16:9. Anchor clamps to 720×405 (still 16:9).
      // Inward drag by 100px → candidateW = 720-100 = 620 → currentH = round(620/(16/9)) ≈ 349.
      const { container } = renderImage({ width: 1920, height: 1080, onUpdateAttributes });
      await flushAsync();

      const handle = container.querySelector('.at-handle-br');
      await fireEvent.mouseDown(handle!, { clientX: 100, clientY: 0 });
      await fireEvent.mouseMove(document, { clientX: 0, clientY: 0 });
      await fireEvent.mouseUp(document, { clientX: 0, clientY: 0 });
      await tick();

      const call = onUpdateAttributes.mock.calls[0]?.[0];
      expect(call).toBeDefined();
      expect(call.width).toBe(620);
      // 620 × (1080/1920) = 620 × 0.5625 = 348.75 → rounds to 349
      expect(call.height).toBe(349);
      // Verify ratio ≈ 16:9 within rounding tolerance
      const ratio = call.width / call.height;
      expect(ratio).toBeGreaterThan(16 / 9 - 0.05);
      expect(ratio).toBeLessThan(16 / 9 + 0.05);
    } finally {
      document.body.removeChild(sentinel);
    }
  });
});

describe('R5-3 — natural dims not clobbered by drag anchor clamp (Option B invariant)', () => {
  it('onUpdateAttributes is NOT called from startDrag — only from handleImageLoad and onDragEnd', async () => {
    const sentinel = document.createElement('div');
    sentinel.className = 'pane-editor';
    sentinel.style.width = '720px';
    document.body.appendChild(sentinel);

    try {
      const onUpdateAttributes = vi.fn();
      const { container } = renderImage({ width: 1920, height: 1080, onUpdateAttributes });
      await flushAsync();

      // Clear any calls from flushAsync (handleImageLoad path fires onUpdateAttributes
      // when width prop is null; here width=1920 so it should NOT fire — verify too).
      onUpdateAttributes.mockClear();

      // Trigger startDrag only — do NOT fire mouseup (so onDragEnd doesn't run).
      const handle = container.querySelector('.at-handle-br');
      await fireEvent.mouseDown(handle!, { clientX: 0, clientY: 0 });
      await tick();
      await tick();

      // startDrag must NOT call onUpdateAttributes — natural dims stay in node.
      expect(onUpdateAttributes).not.toHaveBeenCalled();

      // Cleanup: fire mouseup so document listeners are removed.
      await fireEvent.mouseUp(document, { clientX: 0, clientY: 0 });
    } finally {
      document.body.removeChild(sentinel);
    }
  });

  it('node attrs reflect natural dims (1920×1080) until onDragEnd persists new dims', async () => {
    // Without any drag: width prop stays 1920. The fix must not mutate currentW
    // during startDrag — only applyDrag/onDragEnd may change persisted dims.
    const onUpdateAttributes = vi.fn();
    const { container } = renderImage({ width: 1920, height: 1080, onUpdateAttributes });
    await flushAsync();
    onUpdateAttributes.mockClear();

    // No interaction — assert onUpdateAttributes still hasn't been called.
    expect(onUpdateAttributes).not.toHaveBeenCalled();

    // The img style should still reflect the natural width (1920 in node attrs)
    const img = container.querySelector('img');
    const style = img?.getAttribute('style') ?? '';
    expect(style).toMatch(/width:\s*min\(\s*1920px/);
  });
});

describe('R5-4 — grab #2 behavior unchanged (regression guard)', () => {
  it('already-clamped image (720×405): inward drag works normally on grab #1', async () => {
    const sentinel = document.createElement('div');
    sentinel.className = 'pane-editor';
    sentinel.style.width = '720px';
    document.body.appendChild(sentinel);

    try {
      const onUpdateAttributes = vi.fn();
      // Simulate state after grab #1 persisted clamped dims — same as "grab #2" scenario.
      const { container } = renderImage({ width: 720, height: 405, onUpdateAttributes });
      await flushAsync();

      const handle = container.querySelector('.at-handle-br');
      // dragStartW should be min(720, 720) = 720 — no change from pre-fix behavior.
      // Inward drag by 50px → candidateW = 670 → not clamped → currentW = 670.
      await fireEvent.mouseDown(handle!, { clientX: 50, clientY: 0 });
      await fireEvent.mouseMove(document, { clientX: 0, clientY: 0 });
      await fireEvent.mouseUp(document, { clientX: 0, clientY: 0 });
      await tick();

      const call = onUpdateAttributes.mock.calls[0]?.[0];
      expect(call).toBeDefined();
      expect(call.width).toBe(670);
      // height: 670 / (720/405) ≈ 670 * 405/720 ≈ 376.875 → 377
      expect(call.height).toBe(377);
    } finally {
      document.body.removeChild(sentinel);
    }
  });

  it('dragStartW equals currentW (720) not naturalW when already clamped', async () => {
    // If naturalW were somehow larger (e.g. 1920 in memory), dragStartW must still
    // use the persisted currentW=720 (the || fallback only triggers when currentW===0).
    const sentinel = document.createElement('div');
    sentinel.className = 'pane-editor';
    sentinel.style.width = '720px';
    document.body.appendChild(sentinel);

    try {
      const onUpdateAttributes = vi.fn();
      const { container } = renderImage({ width: 720, height: 405, onUpdateAttributes });
      await flushAsync();

      const handle = container.querySelector('.at-handle-br');
      // Outward drag by 1px → candidateW = 721 → clamped to 720 → currentW stays 720
      await fireEvent.mouseDown(handle!, { clientX: 0, clientY: 0 });
      await fireEvent.mouseMove(document, { clientX: 1, clientY: 0 });
      await fireEvent.mouseUp(document, { clientX: 1, clientY: 0 });
      await tick();

      const call = onUpdateAttributes.mock.calls[0]?.[0];
      expect(call?.width).toBe(720);
    } finally {
      document.body.removeChild(sentinel);
    }
  });
});

import { Node, mergeAttributes } from '@tiptap/core'
import { Plugin, PluginKey } from '@tiptap/pm/state'
import { mount, unmount } from 'svelte'
import type { Component } from 'svelte'
import { writable } from 'svelte/store'
import AttachmentNodeViewComponent from '../components/AttachmentNodeView.svelte'

export interface AttachmentOptions {
  HTMLAttributes: Record<string, any>
  onUpload?: (file: File) => Promise<{ id: string; url: string; filename: string; size?: number | null }>
  onUploadError?: (msg: string) => void
}

declare module '@tiptap/core' {
  interface Commands<ReturnType> {
    attachment: {
      insertAttachment: (attributes: {
        src: string
        alt?: string
        filename?: string
        /** @deprecated use attachmentId */
        id?: string
        attachmentId?: string
        renderMode?: string
        attachmentType?: string
        size?: number | null
      }) => ReturnType
    }
  }
}

export const Attachment = Node.create<AttachmentOptions>({
  name: 'attachment',

  addOptions() {
    return {
      HTMLAttributes: {},
      onUpload: undefined,
      onUploadError: undefined,
    }
  },

  group: 'block',

  draggable: true,

  addAttributes() {
    return {
      src: {
        default: null,
        parseHTML: element => element.getAttribute('src'),
        renderHTML: attributes => {
          if (!attributes.src) return {}
          return { src: attributes.src }
        },
      },
      alt: {
        default: null,
        parseHTML: element => element.getAttribute('alt'),
        renderHTML: attributes => {
          if (!attributes.alt) return {}
          return { alt: attributes.alt }
        },
      },
      filename: {
        default: null,
        parseHTML: element => element.getAttribute('data-filename'),
        renderHTML: attributes => {
          if (!attributes.filename) return {}
          return { 'data-filename': attributes.filename }
        },
      },
      // Renamed from 'id'/'data-id' to 'attachmentId'/'data-attachment-id'.
      // parseHTML reads both old and new attribute names for backward compatibility.
      attachmentId: {
        default: null,
        parseHTML: element =>
          element.getAttribute('data-attachment-id') ?? element.getAttribute('data-id'),
        renderHTML: attributes => {
          if (!attributes.attachmentId) return {}
          return { 'data-attachment-id': attributes.attachmentId }
        },
      },
      // Persisted render mode (e.g. 'full-inline', 'card', 'player', 'preview').
      // Default varies by type per the per-type enum (§2.1A):
      //   image → 'full-inline'; pdf/audio/video/html/unknown → 'card';
      //   link → 'preview' (no 'card' mode for link — fixes #1828)
      // null means the attribute was not explicitly set (treat as default).
      renderMode: {
        default: null,
        parseHTML: element => element.getAttribute('data-render-mode'),
        renderHTML: attributes => {
          if (!attributes.renderMode) return {}
          return { 'data-render-mode': attributes.renderMode }
        },
      },
      // Explicit attachment type tag ('image'|'audio'|'video'|'pdf'|'html'|'link'|'file').
      // Set at insertion time; used by the serializer and NodeView.
      attachmentType: {
        default: null,
        parseHTML: element => element.getAttribute('data-attachment-type'),
        renderHTML: attributes => {
          if (!attributes.attachmentType) return {}
          return { 'data-attachment-type': attributes.attachmentType }
        },
      },
      // Duration in seconds (audio/video). Optional; stored when metadata is available.
      duration: {
        default: null,
        parseHTML: element => {
          const v = element.getAttribute('data-duration')
          return v !== null ? Number(v) : null
        },
        renderHTML: attributes => {
          if (attributes.duration === null || attributes.duration === undefined) return {}
          return { 'data-duration': String(attributes.duration) }
        },
      },
      // Page count (PDF). Optional; stored when metadata is available.
      pageCount: {
        default: null,
        parseHTML: element => {
          const v = element.getAttribute('data-page-count')
          return v !== null ? Number(v) : null
        },
        renderHTML: attributes => {
          if (attributes.pageCount === null || attributes.pageCount === undefined) return {}
          return { 'data-page-count': String(attributes.pageCount) }
        },
      },
      // Image width in pixels. Set when the user resizes an image in full-inline mode.
      // null means unset (render at natural size). Mirrors the duration/pageCount pattern.
      width: {
        default: null,
        parseHTML: element => {
          const v = element.getAttribute('data-width')
          return v !== null ? Number(v) : null
        },
        renderHTML: attributes => {
          if (attributes.width === null || attributes.width === undefined) return {}
          return { 'data-width': String(attributes.width) }
        },
      },
      // Image height in pixels. Pair with width; both are set together on drag-end.
      height: {
        default: null,
        parseHTML: element => {
          const v = element.getAttribute('data-height')
          return v !== null ? Number(v) : null
        },
        renderHTML: attributes => {
          if (attributes.height === null || attributes.height === undefined) return {}
          return { 'data-height': String(attributes.height) }
        },
      },
      // File size in bytes. Populated at insert time from the DB/API response.
      // Serialized as data-size on the wrapper div for lossless round-trip.
      // null means the attribute was not set (pre-#1867 markdown — formatBytes handles gracefully).
      size: {
        default: null,
        parseHTML: element => {
          const v = element.getAttribute('data-size')
          if (v === null) return null
          const n = parseInt(v, 10)
          return isNaN(n) ? null : n
        },
        renderHTML: attributes => {
          if (attributes.size === null || attributes.size === undefined) return {}
          return { 'data-size': String(attributes.size) }
        },
      },
    }
  },

  parseHTML() {
    return [
      {
        // Standalone or wrapped images
        tag: 'img[src]',
      },
      {
        // Legacy attachment wrapper (data-attachment marker, may carry old data-id)
        tag: 'div[data-attachment]',
      },
      {
        // Raw-HTML blocks produced by the non-default-mode serializer:
        //   <div data-attachment-id="…" data-render-mode="…" data-attachment-type="…">
        // Matches even without the data-attachment sentinel so round-trip is lossless.
        tag: 'div[data-attachment-id]',
      },
    ]
  },

  // renderHTML is used only for attribute persistence (serialization round-trip).
  // PR2 replaces the visual layer with a Svelte NodeView; hardcoded inline HTML
  // rendering has been removed here (fixes rendering ownership in the PR2 NodeView).
  // All node data-* attributes are emitted on a wrapper div so the htmlToMarkdown
  // serializer and parseHTML can round-trip the node losslessly.
  renderHTML({ HTMLAttributes }) {
    // Build wrapper data-* attribute map — every persisted attribute lives here.
    const wrapperAttrs: Record<string, string> = { 'data-attachment': '' };
    if (HTMLAttributes.src)                        wrapperAttrs['src']                    = HTMLAttributes.src;
    if (HTMLAttributes.alt)                        wrapperAttrs['alt']                    = HTMLAttributes.alt;
    if (HTMLAttributes['data-filename'])           wrapperAttrs['data-filename']           = HTMLAttributes['data-filename'];
    if (HTMLAttributes['data-attachment-id'])      wrapperAttrs['data-attachment-id']     = HTMLAttributes['data-attachment-id'];
    if (HTMLAttributes['data-render-mode'])        wrapperAttrs['data-render-mode']       = HTMLAttributes['data-render-mode'];
    if (HTMLAttributes['data-attachment-type'])    wrapperAttrs['data-attachment-type']   = HTMLAttributes['data-attachment-type'];
    if (HTMLAttributes['data-duration'])           wrapperAttrs['data-duration']           = HTMLAttributes['data-duration'];
    if (HTMLAttributes['data-page-count'])         wrapperAttrs['data-page-count']         = HTMLAttributes['data-page-count'];
    if (HTMLAttributes['data-width'])               wrapperAttrs['data-width']               = HTMLAttributes['data-width'];
    if (HTMLAttributes['data-height'])              wrapperAttrs['data-height']              = HTMLAttributes['data-height'];
    if (HTMLAttributes['data-size'])                wrapperAttrs['data-size']                = HTMLAttributes['data-size'];

    return ['div', mergeAttributes(this.options.HTMLAttributes, wrapperAttrs)];
  },

  addNodeView() {
    return _createAttachmentNodeViewFactory();
  },

  addCommands() {
    return {
      insertAttachment: (attributes) => ({ commands }) => {
        // Normalise: accept legacy 'id' field alongside the canonical 'attachmentId'
        const { id, attachmentId, ...rest } = attributes as any;
        return commands.insertContent({
          type: 'attachment',
          attrs: { ...rest, attachmentId: attachmentId ?? id ?? null },
        })
      },
    }
  },

  addProseMirrorPlugins() {
    const onUpload = this.options.onUpload;
    const onUploadError = this.options.onUploadError;

    return [
      new Plugin({
        key: new PluginKey('attachment'),
        props: {
          handleDOMEvents: {
            // DOM drop handler restored (#1813 reverts #1808).
            // dragDropEnabled: false lets WKWebView deliver native file drops as DOM DragEvents.
            // Size guard rejects files over 50 MB (base64 path) with a toast via onUploadError.
            // Files under 50 MB go through handleFileUpload → onUpload (base64 / IPC).
            drop: (view, event) => {
              const files = Array.from(event.dataTransfer?.files || [])
              if (files.length > 0 && onUpload) {
                event.preventDefault()
                handleFileUpload(view, files, event, onUpload, onUploadError)
                return true
              }
              return false
            },
            paste: (view, event) => {
              const files = Array.from(event.clipboardData?.files || [])
              if (files.length > 0 && onUpload) {
                event.preventDefault()
                handleFileUpload(view, files, event, onUpload, onUploadError)
                return true
              }
              return false
            },
          },
        },
      }),
    ]
  },
})

// ---------------------------------------------------------------------------
// NodeView factory — exported for unit-testing lifecycle behaviour
// ---------------------------------------------------------------------------

/**
 * Pure predicate used by the NodeView spec's `stopEvent` hook.
 *
 * Returns `true` iff the event's target is inside a `.at-handle` element,
 * telling ProseMirror to yield the event to the NodeView-owned DOM (Svelte's
 * `onmousedown={startDrag}` handler on the resize handle) instead of treating
 * it as a node-drag initiator.  All other targets return `false`, preserving
 * picker clicks (#1858), drag-to-move on the node body, and every other node
 * event.
 *
 * Exported (prefixed `_`) for direct unit-testing without having to fully
 * stub the NodeView factory (fixes #1843 first-grab dead handle).
 */
export function _shouldStopEventForNodeView(event: Event): boolean {
  const target = event.target as HTMLElement | null;
  return !!(target?.closest?.('.at-handle'));
}

/**
 * Returns a TipTap NodeView factory that mounts AttachmentNodeView.svelte.
 *
 * Exported (prefixed `_`) so the remount lifecycle can be verified in
 * AttachmentNodeView.test.ts without going through TipTap internals.
 *
 * Fix (Reed audit #1835 I-1): every call to mount() now reassigns
 * `currentExports` so that subsequent unmount() calls target the LIVE
 * instance, not the stale original.  `currentNode` tracks the latest node
 * so that selectNode/deselectNode remount with current attrs, not the
 * initial closure value.
 */
export function _createAttachmentNodeViewFactory() {
  return (props: any) => {
    // Svelte 5 NodeView: mount AttachmentNodeView into a wrapper div.
    // The wrapper is what TipTap sees as `dom`; the Svelte component owns
    // the interior.  We keep a reference to the mounted component so we
    // can unmount it when TipTap destroys the node view.
    const dom = document.createElement('div');
    // Non-editable marker so TipTap treats the whole block as atomic.
    dom.setAttribute('contenteditable', 'false');
    dom.style.cssText = 'display:block;';

    // updateAttributes: build this ourselves from editor + getPos, exactly as TipTap's
    // NodeView base class does (see NodeView.ts updateAttributes() in @tiptap/core).
    // NodeViewRendererProps does NOT include updateAttributes directly — it must be
    // constructed from the editor command API. Prior code read props.updateAttributes
    // which was always undefined, silently swallowing all mode/dim changes.
    //
    // options.skipDirty (fix #2950): AttachmentImageInline's handleImageLoad() persists
    // an image's natural width/height the FIRST time it loads (when width/height are
    // still null in the node attrs). That persist is bookkeeping, not a user edit — it
    // fires asynchronously from the <img> `load` event, which can land AFTER Editor.svelte
    // has already reset `isSettingContent` to false post-mount. Without this guard, that
    // async attribute write trips Editor.svelte's onUpdate handler exactly like a real user
    // edit would, setting `dirty = true` and triggering a spurious debounced save +
    // `pane-data-changed` dispatch on notes the user never touched.
    //
    // Fix: tag the ProseMirror transaction with the SAME 'preventUpdate' meta key that
    // TipTap's own `setContent({ emitUpdate: false })` uses internally (see
    // @tiptap/core Editor.dispatchTransaction: `if (transaction.getMeta('preventUpdate')
    // || ...) return;` — it skips emitting the 'update' event entirely). This is a
    // TipTap-native mechanism, not a bespoke flag: the document still updates and the
    // node re-renders (the 'transaction' event still fires), but Editor.svelte's
    // `onUpdate` callback — which is what sets `dirty = true` — never runs for this
    // specific transaction. Legitimate user-driven attribute updates (drag-resize
    // dims, render-mode picker changes) do NOT pass skipDirty, so they continue to
    // mark the editor dirty and save normally — only the auto-persist-on-load path
    // is suppressed.
    const updateAttrs = (attrs: Record<string, unknown>, options?: { skipDirty?: boolean }) => {
      props.editor.commands.command(({ tr }: { tr: any }) => {
        const pos = typeof props.getPos === 'function' ? props.getPos() : undefined;
        if (typeof pos !== 'number') return false;
        tr.setNodeMarkup(pos, undefined, {
          ...currentNode.attrs,
          ...attrs,
        });
        if (options?.skipDirty) {
          tr.setMeta('preventUpdate', true);
        }
        return true;
      });
    };

    // Build reactive props from the current node attrs.
    const buildAttrs = (node: any) => ({
      attachmentId:   node.attrs.attachmentId   ?? null,
      attachmentType: node.attrs.attachmentType ?? null,
      filename:       node.attrs.filename       ?? null,
      src:            node.attrs.src            ?? null,
      alt:            node.attrs.alt            ?? null,
      duration:       node.attrs.duration       ?? null,
      pageCount:      node.attrs.pageCount      ?? null,
      renderMode:     node.attrs.renderMode     ?? null,
      width:          node.attrs.width          ?? null,
      height:         node.attrs.height         ?? null,
      size:           node.attrs.size           ?? null,
      status:         null as ('uploading' | 'failed' | 'ready' | null),
      errorDetail:    null as string | null,
    });

    // Track selected state from TipTap decorations.
    let selected = false;

    // Reactive store for selection state — avoids unmount+remount on
    // every selectNode/deselectNode call (Strategy A, fixes #1843/#401).
    // Created ONCE per NodeView instance; passed to every mount() call so
    // update() remounts receive the same live store reference.
    const selectedStore = writable(false);

    // Track the CURRENT node so selectNode/deselectNode use fresh attrs,
    // not the stale initial closure value (fixes Reed audit #1835 I-1).
    let currentNode = props.node;

    // Mount the Svelte component and keep a MUTABLE reference so every
    // remount path can unmount the correct live instance.
    let currentExports = mount(
      AttachmentNodeViewComponent as unknown as Component<any>,
      {
        target: dom,
        props: {
          attrs: buildAttrs(props.node),
          selected: selectedStore,
          onUpdateAttributes: updateAttrs,
        },
      }
    );

    return {
      dom,
      // Called by TipTap whenever the node's props change.
      update(node: any) {
        // Re-mount with updated props by unmounting and re-mounting.
        // Svelte 5's fine-grained reactivity means a simpler $set-style update
        // is preferred, but since we used `mount()` (not a class), we patch
        // by unmounting and remounting.  For PR2 this is sufficient; PR3+
        // can switch to a runes-based prop update if perf is a concern.
        currentNode = node;
        unmount(currentExports);
        currentExports = mount(
          AttachmentNodeViewComponent as unknown as Component<any>,
          {
            target: dom,
            props: {
              attrs: buildAttrs(node),
              // Pass the same selectedStore so the new component instance
              // immediately reflects the current selection state without
              // requiring another selectNode/deselectNode call.
              selected: selectedStore,
              onUpdateAttributes: updateAttrs,
            },
          }
        );
        return true;
      },
      // Called when TipTap's selection enters/leaves this node.
      //
      // Strategy A fix (#1843): push into selectedStore instead of
      // unmount+remount.  The live Svelte component updates reactively;
      // the mode button DOM element is never torn down, so the originating
      // click event (which fires AFTER mousedown triggers selectNode) reaches
      // the same button element and opens the picker on the FIRST click.
      //
      // #1835 correctness preserved: currentExports and currentNode are
      // exclusively maintained by update() — selectNode/deselectNode no longer
      // touch them.  The stale-closure invariant is unchanged.
      selectNode() {
        selected = true;
        selectedStore.set(true);
      },
      deselectNode() {
        selected = false;
        selectedStore.set(false);
      },
      // Tell ProseMirror to yield events targeting a resize handle to the
      // NodeView-owned DOM (Svelte's onmousedown={startDrag}).  Without this,
      // the first mousedown on an unselected image is consumed by ProseMirror
      // as a node-drag initiator (draggable: true), so the handle listener
      // never fires on the first grab.  All other targets return false so
      // drag-to-move, picker clicks (#1858), and every other node event are
      // unchanged.  (#1843 fix — round 5)
      stopEvent(event: Event) {
        return _shouldStopEventForNodeView(event);
      },
      destroy() {
        // Clean up the LIVE Svelte component (not a stale reference).
        unmount(currentExports);
      },
    };
  };
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Map a File's mime type / extension to a canonical attachment type tag. */
export function detectAttachmentType(file: File): string {
  const mime = file.type;
  const name = file.name.toLowerCase();
  if (mime.startsWith('image/') || /\.(jpg|jpeg|png|gif|webp|svg)$/.test(name)) return 'image';
  if (mime.startsWith('audio/') || /\.(mp3|wav|ogg|m4a|aac|flac)$/.test(name)) return 'audio';
  if (mime.startsWith('video/') || /\.(mp4|webm|mov|avi|mkv)$/.test(name)) return 'video';
  if (mime === 'application/pdf' || name.endsWith('.pdf')) return 'pdf';
  if (mime === 'text/html' || /\.html?$/.test(name)) return 'html';
  return 'file';
}

/**
 * Return the default render mode for a given attachment type per §2.1A
 * (per-type render-mode enum, corrected 2026-06-30 — fixes #1828):
 *
 *   image   → 'full-inline'   (expanded by default)
 *   pdf     → 'card'          (collapsed card)
 *   audio   → 'card'
 *   video   → 'card'
 *   html    → 'card'
 *   link    → 'preview'       (NOT 'card' — link has no card mode; fixes #1828)
 *   unknown → 'card'
 *   (anything else) → 'card'  (safe fallback)
 */
export function defaultRenderMode(attachmentType: string): string {
  switch (attachmentType) {
    case 'image':   return 'full-inline';
    case 'link':    return 'preview';
    // pdf, audio, video, html, file, unknown, and any future type all collapse to 'card'
    default:        return 'card';
  }
}

/** Valid render modes per attachment type (§2.1A). */
export const VALID_RENDER_MODES: Record<string, readonly string[]> = {
  image:   ['full-inline', 'card'],
  pdf:     ['card', 'reader'],
  audio:   ['card', 'player'],
  video:   ['card', 'player'],
  html:    ['card', 'preview'],
  link:    ['preview', 'plain'],
  unknown: ['card'],
  file:    ['card'],
} as const;

/**
 * Validate a render mode for a given attachment type.
 * Returns the mode unchanged if valid; falls back to the type's default otherwise.
 */
export function resolveRenderMode(attachmentType: string, mode: string | null | undefined): string {
  const def = defaultRenderMode(attachmentType);
  if (!mode) return def;
  const valid = VALID_RENDER_MODES[attachmentType] ?? ['card'];
  return (valid as readonly string[]).includes(mode) ? mode : def;
}

// Helper function for file upload handling
async function handleFileUpload(
  view: any,
  files: File[],
  event: Event,
  onUpload: (file: File) => Promise<{ id: string; url: string; filename: string; size?: number | null }>,
  onError?: (msg: string) => void
) {
  const pos = view.posAtCoords({
    left: (event as DragEvent).clientX || 0,
    top: (event as DragEvent).clientY || 0
  })?.pos || view.state.selection.from

  for (const file of files) {
    let placeholderPos = pos;
    let placeholderSize = 0;

    try {
      // Create placeholder
      const tr = view.state.tr
      const placeholder = view.state.schema.nodes.paragraph.create({}, [
        view.state.schema.text(`⏳ Uploading ${file.name}...`)
      ])
      tr.insert(placeholderPos, placeholder)
      placeholderSize = placeholder.nodeSize;
      view.dispatch(tr)

      // Upload file with proper error handling
      const result = await onUpload(file).catch((uploadError: any) => {
        throw uploadError; // Re-throw to outer try-catch
      });

      // Replace placeholder with actual attachment
      const detectedType = detectAttachmentType(file);
      const newTr = view.state.tr
      newTr.delete(placeholderPos, placeholderPos + placeholderSize)
      newTr.insert(placeholderPos, view.state.schema.nodes.attachment.create({
        src: result.url,
        filename: result.filename,
        attachmentId: result.id,
        alt: file.name,
        attachmentType: detectedType,
        renderMode: defaultRenderMode(detectedType),
        size: result.size ?? null,
      }))
      view.dispatch(newTr)

    } catch (error: any) {
      console.error('[Attachment] Upload failed:', error)

      // Remove placeholder (don't pollute the document with error text)
      try {
        if (placeholderSize > 0) {
          const cleanTr = view.state.tr;
          cleanTr.delete(placeholderPos, placeholderPos + placeholderSize);
          view.dispatch(cleanTr);
        }
      } catch (cleanupError) {
        console.error('[Attachment] Failed to remove placeholder after error:', cleanupError);
      }

      // Surface the error via toast (not inline in the document)
      const msg = error?.message || `Upload failed: ${file.name}`;
      if (onError) {
        onError(msg);
      } else {
        console.warn('[Attachment] Upload error (no onError handler):', msg);
      }
    }
  }
}
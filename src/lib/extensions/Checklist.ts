// Pane — Checklist TipTap Node Extension (§31.3, ADR-0057)
//
// This node is a placeholder/anchor that tells the editor "render a checklist here."
// No item data is stored on the TipTap node — the DB (subtasks table) is the
// source of truth per §31.3.3.
//
// Pattern mirrors Attachment.ts exactly (mount/unmount Svelte 5 component,
// raw-HTML-block serialization, atom: true).

import { Node, mergeAttributes } from '@tiptap/core'
import { mount, unmount } from 'svelte'
import type { Component } from 'svelte'
import ChecklistNodeViewComponent from '../components/ChecklistNodeView.svelte'

export interface ChecklistOptions {
  HTMLAttributes: Record<string, any>
}

declare module '@tiptap/core' {
  interface Commands<ReturnType> {
    checklist: {
      /**
       * Insert a checklist anchor block at the current cursor position.
       * No data is stored on the node — the note's subtasks table rows
       * are rendered dynamically by the Svelte component.
       */
      insertChecklist: () => ReturnType
    }
  }
}

export const Checklist = Node.create<ChecklistOptions>({
  name: 'checklist',

  addOptions() {
    return {
      HTMLAttributes: {},
    }
  },

  // Block-level, non-editable atom — the Svelte component owns its interior.
  group: 'block',
  atom: true,
  selectable: false,  // Prevents NodeSelection (fixes #2111 teal wash). WikiLink uses same pattern.
  draggable: true,

  addAttributes() {
    return {
      // Sentinel attribute so parseHTML and the serializer can round-trip
      // the node. No item data lives here — DB is source of truth.
      component: {
        default: 'checklist',
        parseHTML: element => element.getAttribute('data-component'),
        renderHTML: () => ({ 'data-component': 'checklist' }),
      },
    }
  },

  parseHTML() {
    return [
      {
        // Raw-HTML blocks produced by the serializer and stored in markdown.
        tag: 'div[data-component="checklist"]',
      },
    ]
  },

  renderHTML({ HTMLAttributes }) {
    return [
      'div',
      mergeAttributes(this.options.HTMLAttributes, HTMLAttributes, {
        'data-component': 'checklist',
      }),
    ]
  },

  addNodeView() {
    return _createChecklistNodeViewFactory()
  },

  addCommands() {
    return {
      insertChecklist: () => ({ commands }) => {
        return commands.insertContent({
          type: 'checklist',
          attrs: { component: 'checklist' },
        })
      },
    }
  },
})

// ---------------------------------------------------------------------------
// NodeView factory
// ---------------------------------------------------------------------------

/**
 * Returns a TipTap NodeView factory that mounts ChecklistNodeView.svelte.
 * Follows the same mount/unmount pattern as _createAttachmentNodeViewFactory()
 * in Attachment.ts.
 *
 * Exported (prefixed `_`) for unit testing.
 */
export function _createChecklistNodeViewFactory() {
  return (_props: any) => {
    const dom = document.createElement('div')
    dom.setAttribute('contenteditable', 'false')
    dom.style.cssText = 'display:block;'

    let currentExports = mount(
      ChecklistNodeViewComponent as unknown as Component<any>,
      {
        target: dom,
        props: {},
      },
    )

    return {
      dom,
      update(_node: any) {
        // Return true: no per-node attrs to sync. Svelte's $derived reactivity
        // tracks $activeNote changes without requiring DOM remount.
        // (Unmount/remount on every TipTap transaction was wasteful and caused
        // checklist state loss on every keystroke — fix/checklist-noteid-source)
        return true
      },
      stopEvent(event: Event) {
        // §31.3.4: Block must be draggable in document. Pass drag events through to ProseMirror
        // for block-level repositioning; stop internal interaction events (clicks, inputs on
        // checklist items) to prevent editor interference. Mirrors Attachment.ts pattern.
        const type = event.type
        if (type === 'dragstart' || type === 'dragend' || type === 'dragover' || type === 'drop') {
          return false
        }
        return true
      },
      destroy() {
        unmount(currentExports)
      },
    }
  }
}

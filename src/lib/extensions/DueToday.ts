// Pane — DueToday TipTap Node Extension (§31.2, ADR-0057, #2091)
//
// This node is an anchor block that renders a snapshot of all tasks/subtasks
// due on a specific date. The `date` attribute (YYYY-MM-DD) is the source of
// truth — it is stamped at insert time and persists forever (§31.2.3
// anchored-not-live rule: blocks never auto-roll to today's date).
//
// Pattern mirrors Checklist.ts exactly (mount/unmount Svelte 5 component,
// raw-HTML-block serialization, atom: true, selectable: false).

import { Node, mergeAttributes } from '@tiptap/core'
import { mount, unmount } from 'svelte'
import type { Component } from 'svelte'
import DueTodayNodeViewComponent from '../components/DueTodayNodeView.svelte'

export interface DueTodayOptions {
  HTMLAttributes: Record<string, any>
}

declare module '@tiptap/core' {
  interface Commands<ReturnType> {
    dueToday: {
      /**
       * Insert a Due Today block stamped with the current local date (YYYY-MM-DD).
       * §31.2.3: the date is anchored at insert time — it never auto-updates.
       */
      insertDueToday: () => ReturnType
    }
  }
}

export const DueToday = Node.create<DueTodayOptions>({
  name: 'dueToday',

  addOptions() {
    return {
      HTMLAttributes: {},
    }
  },

  // Block-level, non-editable atom — the Svelte component owns its interior.
  group: 'block',
  atom: true,
  selectable: false,  // Prevents NodeSelection teal wash (same fix as Checklist.ts).
  draggable: true,

  addAttributes() {
    return {
      // `component` sentinel for round-trip serialization.
      component: {
        default: 'dueToday',
        parseHTML: element => element.getAttribute('data-component'),
        renderHTML: () => ({ 'data-component': 'dueToday' }),
      },
      // `date` is stamped at insert time as the local YYYY-MM-DD.
      // §31.2.3: null date → error state in the NodeView; never fall back to now().
      date: {
        default: null,
        parseHTML: element => element.getAttribute('data-date') ?? null,
        renderHTML: attributes => {
          if (!attributes.date) return {}
          return { 'data-date': attributes.date }
        },
      },
    }
  },

  parseHTML() {
    return [
      {
        // Raw-HTML blocks produced by the serializer and stored in markdown.
        tag: 'div[data-component="dueToday"]',
      },
    ]
  },

  renderHTML({ HTMLAttributes }) {
    return [
      'div',
      mergeAttributes(this.options.HTMLAttributes, HTMLAttributes, {
        'data-component': 'dueToday',
      }),
    ]
  },

  addNodeView() {
    return _createDueTodayNodeViewFactory()
  },

  addCommands() {
    return {
      insertDueToday: () => ({ commands }) => {
        // §31.2.3: stamp local date at insert time — never now() fallback after the fact.
        const today = new Date().toLocaleDateString('en-CA') // YYYY-MM-DD in local time
        return commands.insertContent({
          type: 'dueToday',
          attrs: { component: 'dueToday', date: today },
        })
      },
    }
  },
})

// ---------------------------------------------------------------------------
// NodeView factory
// ---------------------------------------------------------------------------

/**
 * Returns a TipTap NodeView factory that mounts DueTodayNodeView.svelte.
 * Follows the same mount/unmount pattern as Checklist.ts.
 *
 * Exported (prefixed `_`) for unit testing.
 */
export function _createDueTodayNodeViewFactory() {
  return (props: any) => {
    const dom = document.createElement('div')
    dom.setAttribute('contenteditable', 'false')
    dom.style.cssText = 'display:block;'

    let currentExports = mount(
      DueTodayNodeViewComponent as unknown as Component<any>,
      {
        target: dom,
        props: {
          date: props.node?.attrs?.date ?? null,
          getPos: props.getPos,
          editor: props.editor,
        },
      },
    )

    return {
      dom,
      update(node: any) {
        // Sync date attr changes (e.g. user picks a new date via the block's date chip).
        if (currentExports && typeof (currentExports as any).setDate === 'function') {
          ;(currentExports as any).setDate(node?.attrs?.date ?? null)
        }
        return true
      },
      stopEvent(event: Event) {
        // §31.2: block is draggable — pass drag events through to ProseMirror for
        // block-level repositioning; stop all other internal interaction events.
        // Mirrors Checklist.ts pattern exactly.
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

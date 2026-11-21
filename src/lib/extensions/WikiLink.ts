import { Node, mergeAttributes } from '@tiptap/core'
import { Plugin, PluginKey } from '@tiptap/pm/state'
import * as api from '$lib/api'
import type { NoteSearchResult } from '$lib/types'

export interface WikiLinkOptions {
  HTMLAttributes: Record<string, any>
  onNavigate?: (title: string) => void
  getCurrentProject?: () => string | null
}

declare module '@tiptap/core' {
  interface Commands<ReturnType> {
    wikiLink: {
      insertWikiLink: (title: string) => ReturnType
    }
  }
}

// WikiLink suggestion plugin key
const wikiLinkPluginKey = new PluginKey('wikiLinkSuggestion')

// Suggestion state interface (exported for test use — see renderWidget export above)
export interface SuggestionState {
  active: boolean
  range: { from: number; to: number } | null
  query: string
  items: Array<{ title: string; id: string }>
  selectedIndex: number
}

// Helper functions
function fetchSuggestions(query: string): Promise<Array<{ title: string; id: string }>> {
  if (query.length < 1) return Promise.resolve([])
  
  return api.searchNotesByTitle(query)
    .then((notes: NoteSearchResult[]) => notes.map(note => ({ title: note.title, id: note.id })))
    .catch(() => [])
}

// Exported for direct unit testing of the empty-state guard (#2419 round-2
// Finding 2 Part B) without needing to drive a full ProseMirror view/plugin
// lifecycle. Not part of the extension's public API surface used by callers.
export function renderWidget(view: any, state: SuggestionState) {
  if (!state.active || !state.range) return null

  // #2419 round-2 fix (Finding 2 Part B, per Matt's ruling 2026-08-05): bare
  // `[[` with zero typed characters must show NO popup at all — there is
  // nothing to say "not found" about yet. "No notes found" is only meaningful
  // once the user has actually typed a query and it matched nothing. Without
  // this guard, `state.items.length === 0` was true both for "just typed [["
  // AND "typed a query with zero matches", so both cases rendered the same
  // confusing "No notes found" message immediately on `[[`.
  if (state.items.length === 0 && (!state.query || state.query.length === 0)) return null

  const widget = document.createElement('div')
  widget.className = 'wiki-link-autocomplete'
  
  const popup = document.createElement('div')
  popup.className = 'autocomplete-popup'
  widget.appendChild(popup)
  
  if (state.items.length === 0) {
    const emptyItem = document.createElement('div')
    emptyItem.className = 'autocomplete-item'
    emptyItem.textContent = 'No notes found'
    popup.appendChild(emptyItem)
  } else {
    state.items.forEach((item, index) => {
      const itemEl = document.createElement('div')
      itemEl.className = `autocomplete-item${index === state.selectedIndex ? ' selected' : ''}`
      itemEl.textContent = item.title
      itemEl.onclick = () => selectSuggestion(view, state, index)
      popup.appendChild(itemEl)
    })
  }

  // Position the popup
  const coords = view.coordsAtPos(state.range.from)
  widget.style.position = 'absolute'
  widget.style.top = `${coords.bottom + 8}px`
  widget.style.left = `${coords.left}px`
  widget.style.zIndex = '1000'
  
  return widget
}

function selectSuggestion(view: any, state: SuggestionState, index: number) {
  if (!state.range || !state.items[index]) return
  
  const { from, to } = state.range
  const item = state.items[index]
  
  // Create wiki link node
  const wikiLinkNode = view.state.schema.nodes.wikiLink.create({ title: item.title })
  
  // Replace the suggestion text with the wiki link node
  const tr = view.state.tr.replaceWith(from, to, wikiLinkNode)
  view.dispatch(tr)
  
  // Clear suggestion state
  const newState: SuggestionState = {
    active: false,
    range: null,
    query: '',
    items: [],
    selectedIndex: 0
  }
  view.dispatch(view.state.tr.setMeta(wikiLinkPluginKey, { newState }))
}

// Simple WikiLink node - renders [[Title]] as clickable spans
export const WikiLink = Node.create<WikiLinkOptions>({
  name: 'wikiLink',

  addOptions() {
    return {
      HTMLAttributes: {},
      onNavigate: undefined,
      getCurrentProject: undefined,
    }
  },

  group: 'inline',
  inline: true,
  selectable: false,
  atom: true,

  addAttributes() {
    return {
      title: {
        default: null,
        parseHTML: element => element.getAttribute('data-title') || element.textContent || '',
        renderHTML: attributes => {
          if (!attributes.title) return {}
          return { 'data-title': attributes.title }
        },
      },
    }
  },

  parseHTML() {
    return [
      {
        tag: 'span[data-wiki-link]',
        priority: 100,
        getAttrs: (node: HTMLElement) => {
          const title = node.getAttribute('data-title') || node.textContent || '';
          return title ? { title } : false;
        },
      },
    ]
  },

  renderHTML({ HTMLAttributes, node }) {
    const title = node?.attrs?.title || '';
    return [
      'span',
      mergeAttributes(
        { 
          class: 'wiki-link', 
          'data-wiki-link': 'true',
          'data-title': title,
        },
        this.options.HTMLAttributes || {},
      ),
      title,
    ]
  },

  addCommands() {
    return {
      insertWikiLink:
        title =>
        ({ commands }) => {
          return commands.insertContent({
            type: this.name,
            attrs: { title },
          })
        },
    }
  },

  addProseMirrorPlugins() {
    const extensionThis = this

    return [
      // Click handler for wiki links
      new Plugin({
        key: new PluginKey('wikiLinkClick'),
        props: {
          handleClick: (view: any, pos: number, event: MouseEvent) => {
            const target = event.target as HTMLElement
            if (target.matches('[data-wiki-link]') || target.closest('[data-wiki-link]')) {
              event.preventDefault()
              const el = target.matches('[data-wiki-link]') ? target : target.closest('[data-wiki-link]') as HTMLElement
              const title = el?.getAttribute('data-title')
              if (title && extensionThis.options.onNavigate) {
                extensionThis.options.onNavigate(title)
              }
              return true
            }
            return false
          },
        },
      }),
      
      // Custom suggestion plugin
      new Plugin({
        key: wikiLinkPluginKey,
        
        state: {
          init(): SuggestionState {
            return {
              active: false,
              range: null,
              query: '',
              items: [],
              selectedIndex: 0
            }
          },
          
          apply(tr, prevState: SuggestionState): SuggestionState {
            const meta = tr.getMeta(wikiLinkPluginKey)
            if (meta && meta.newState) {
              return meta.newState
            }
            
            // Check if we should activate suggestion
            if (!tr.docChanged && !tr.selectionSet) {
              return prevState
            }
            
            const { from, to } = tr.selection
            if (from !== to) {
              // Clear suggestions if there's a selection
              return prevState.active ? { ...prevState, active: false, range: null } : prevState
            }
            
            // Look for [[ trigger
            const textBefore = tr.doc.textBetween(Math.max(0, from - 10), from)
            const match = textBefore.match(/\[\[([^\]]*)$/)
            
            if (match) {
              const query = match[1]
              const rangeFrom = from - match[1].length - 2
              const rangeTo = from
              
              if (query !== prevState.query || !prevState.active) {
                // New or changed query - fetch suggestions
                fetchSuggestions(query).then(items => {
                  const newState: SuggestionState = {
                    active: true,
                    range: { from: rangeFrom, to: rangeTo },
                    query,
                    items,
                    selectedIndex: 0
                  }
                  const view = (tr as any).getMeta('view') || (window as any).currentEditorView
                  if (view) {
                    view.dispatch(view.state.tr.setMeta(wikiLinkPluginKey, { newState }))
                  }
                })
                
                return {
                  active: true,
                  range: { from: rangeFrom, to: rangeTo },
                  query,
                  items: prevState.query === query ? prevState.items : [],
                  selectedIndex: 0
                }
              }
              
              return {
                ...prevState,
                range: { from: rangeFrom, to: rangeTo }
              }
            } else if (prevState.active) {
              // No longer matching [[ pattern - deactivate
              return { ...prevState, active: false, range: null }
            }
            
            return prevState
          }
        },
        
        props: {
          handleKeyDown: (view, event) => {
            const state = wikiLinkPluginKey.getState(view.state)
            if (!state?.active || !state.range) return false
            
            if (event.key === 'Escape') {
              const newState: SuggestionState = { ...state, active: false, range: null }
              view.dispatch(view.state.tr.setMeta(wikiLinkPluginKey, { newState }))
              return true
            }
            
            if (event.key === 'ArrowUp') {
              const newIndex = Math.max(0, state.selectedIndex - 1)
              const newState: SuggestionState = { ...state, selectedIndex: newIndex }
              view.dispatch(view.state.tr.setMeta(wikiLinkPluginKey, { newState }))
              return true
            }
            
            if (event.key === 'ArrowDown') {
              const newIndex = Math.min(state.items.length - 1, state.selectedIndex + 1)
              const newState: SuggestionState = { ...state, selectedIndex: newIndex }
              view.dispatch(view.state.tr.setMeta(wikiLinkPluginKey, { newState }))
              return true
            }
            
            if (event.key === 'Enter') {
              event.preventDefault()
              selectSuggestion(view, state, state.selectedIndex)
              return true
            }
            
            return false
          }
        },
        
        view: () => {
          let widget: HTMLElement | null = null
          
          return {
            update: (view, prevState) => {
              // Store view reference for async suggestions
              ;(window as any).currentEditorView = view
              
              const state = wikiLinkPluginKey.getState(view.state)
              const prevPluginState = wikiLinkPluginKey.getState(prevState)
              
              if (state?.active && (!prevPluginState?.active || state !== prevPluginState)) {
                // Remove old widget
                if (widget) {
                  widget.remove()
                  widget = null
                }
                
                // Create new widget
                if (state.range) {
                  widget = renderWidget(view, state)
                  if (widget) {
                    document.body.appendChild(widget)
                  }
                }
              } else if (!state?.active && widget) {
                // Remove widget
                widget.remove()
                widget = null
              }
            },
            
            destroy: () => {
              if (widget) {
                widget.remove()
                widget = null
              }
            }
          }
        }
      })
    ]
  },
})
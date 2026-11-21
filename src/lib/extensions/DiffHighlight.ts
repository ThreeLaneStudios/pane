import { Extension } from '@tiptap/core';
import { Plugin, PluginKey } from '@tiptap/pm/state';
import { Decoration, DecorationSet } from '@tiptap/pm/view';
import fastDiff from 'fast-diff';

const diffHighlightKey = new PluginKey('diffHighlight');

function computeDiffDecorations(
  oldText: string,
  newDoc: import('@tiptap/pm/model').Node
): DecorationSet {
  const newText = newDoc.textContent;
  if (oldText === newText) return DecorationSet.empty;

  const diffs = fastDiff(oldText, newText);
  const decorations: Decoration[] = [];

  // Build char-index -> doc-pos mapping
  // In ProseMirror, text positions are absolute. pos from descendants
  // points to the start of the text node content.
  const posMap: number[] = [];
  newDoc.descendants((node, pos) => {
    if (node.isText && node.text) {
      for (let i = 0; i < node.text.length; i++) {
        posMap.push(pos + i);
      }
    }
    return true;
  });
  console.log('[DIFF] posMap length:', posMap.length, 'newText length:', newText.length, 'oldText length:', oldText.length);

  let newOffset = 0;
  for (const [type, text] of diffs) {
    if (type === fastDiff.EQUAL) {
      newOffset += text.length;
    } else if (type === fastDiff.INSERT) {
      const startIdx = newOffset;
      const endIdx = newOffset + text.length;
      if (startIdx < posMap.length && endIdx - 1 < posMap.length) {
        decorations.push(
          Decoration.inline(posMap[startIdx], posMap[endIdx - 1] + 1, { class: 'diff-added' })
        );
      }
      newOffset += text.length;
    }
    // DELETE: text no longer in doc, skip
  }

  console.log('[DIFF] Created', decorations.length, 'decorations from', diffs.filter(d => d[0] !== 0).length, 'diff segments');
  if (decorations.length === 0) return DecorationSet.empty;
  return DecorationSet.create(newDoc, decorations);
}

export const DiffHighlight = Extension.create({
  name: 'diffHighlight',
  addProseMirrorPlugins() {
    return [
      new Plugin({
        key: diffHighlightKey,
        state: {
          init() { return DecorationSet.empty; },
          apply(tr, value) {
            const meta = tr.getMeta(diffHighlightKey);
            if (meta === 'clear') return DecorationSet.empty;
            if (meta instanceof DecorationSet) return meta;
            return value.map(tr.mapping, tr.doc);
          },
        },
        props: {
          decorations(state) { return this.getState(state); },
        },
      }),
    ];
  },
});

export function applyDiffHighlight(
  editor: import('@tiptap/core').Editor,
  oldText: string
): void {
  const { state, view } = editor;
  const decorations = computeDiffDecorations(oldText, state.doc);
  if (decorations === DecorationSet.empty) return;

  view.dispatch(state.tr.setMeta(diffHighlightKey, decorations));

  setTimeout(() => {
    view.dom.querySelectorAll('.diff-added').forEach((el) => el.classList.add('diff-fading'));
  }, 2500);

  setTimeout(() => {
    if (editor && !editor.isDestroyed) {
      editor.view.dispatch(editor.state.tr.setMeta(diffHighlightKey, 'clear'));
    }
  }, 3000);
}

export { diffHighlightKey };

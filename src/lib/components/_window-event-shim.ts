// Test-only shim — NOT a deliverable component, not exported/used by any
// production code.
//
// vitest-setup.ts replaces `globalThis.window` with a plain mock object
// lacking addEventListener/removeEventListener/dispatchEvent. Several
// stores (syncStore.ts, authStore.ts) call `window.addEventListener` at
// MODULE LOAD TIME (top-level, not inside a function) — CanvasRenderer.svelte
// transitively imports Settings.svelte, which imports authStore/syncStore,
// so simply mutating `globalThis.window` from plain top-level statements
// inside the test file (after `import CanvasRenderer from './CanvasRenderer.svelte'`)
// is too late: ES module semantics evaluate ALL of a module's `import`
// declarations (recursively, in source order) before any of that module's
// own top-level non-import code runs. Import declarations are hoisted
// ahead of other statements regardless of textual interleaving, so a plain
// statement placed textually above an `import` line still does not run
// first.
//
// The only way to guarantee this shim's side effects run before
// CanvasRenderer's transitive chain evaluates is to make it itself an
// `import` — and the FIRST one in the test file's source order, so its
// module (and this file's top-level code) is fully evaluated before the
// `import CanvasRenderer from './CanvasRenderer.svelte'` statement's target
// module begins evaluating.
const win = globalThis.window as any;
if (typeof win.addEventListener !== 'function') {
  win.addEventListener = globalThis.addEventListener?.bind(globalThis) ?? (() => {});
}
if (typeof win.removeEventListener !== 'function') {
  win.removeEventListener = globalThis.removeEventListener?.bind(globalThis) ?? (() => {});
}
win.dispatchEvent = globalThis.dispatchEvent?.bind(globalThis) ?? (() => true);

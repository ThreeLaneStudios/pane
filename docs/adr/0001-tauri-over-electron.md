# ADR-0001: Tauri over Electron

**Status:** Accepted  
**Date:** 2026-02-08  
**Author:** Matt Carano

## Context

Pane is a desktop notes/tasks app with AI integration. It needs to run natively on macOS (primary), with Linux and Windows planned within months of launch. The app handles sensitive user data (notes, tasks, AI conversations) and must be fast, lightweight, and privacy-respecting.

The two realistic options for cross-platform desktop apps with web-based UIs are Electron and Tauri.

## Decision

Use Tauri (Rust backend + SvelteKit frontend) instead of Electron.

## Consequences

### Positive
- **Smaller binaries** — Tauri apps are ~10-30MB vs Electron's 150-300MB. Uses the system WebView instead of bundling Chromium.
- **Lower memory footprint** — No dedicated Chromium process per window.
- **Rust backend** — SQLite integration, encryption, file I/O, and AI orchestration all happen in Rust. Type-safe, memory-safe, fast.
- **Local-first alignment** — Tauri's architecture naturally supports our local-first philosophy. No server required.
- **Apple code signing** — Tauri builds produce proper .app bundles that sign and notarize cleanly.

### Negative
- **No web deployment** — Tauri apps can't run in a browser. We can't offer a web version without a separate codebase. (Pane mobile will be React Native, not web.)
- **Smaller ecosystem** — Fewer plugins, fewer examples, smaller community than Electron.
- **WebView inconsistencies** — System WebView varies by OS. macOS WebKit ≠ Windows WebView2 ≠ Linux WebKitGTK. CSS/JS behavior may differ.
- **Rust learning curve** — Non-trivial for contributors who only know TypeScript.
- **Headless testing is hard** — Tauri windows need a display server. E2E tests can't run headless on macOS (no Xvfb equivalent). See #514.

### Risks
- WebView2 (Windows) or WebKitGTK (Linux) may have rendering bugs we can't control.
- Tauri is younger than Electron — breaking changes between major versions are possible.

## Alternatives Considered

**Electron** — Rejected. Too heavy (150MB+), bundles Chromium, high memory usage. Doesn't align with the lightweight, privacy-first product vision. Would work for web deployment but we don't want that.

**Flutter** — Considered briefly. Desktop support is maturing but not as stable as Tauri for macOS. Dart is less useful than Rust for our backend needs (SQLite, encryption, AI).

**Native Swift (macOS only)** — Would give the best macOS experience but locks us out of cross-platform. Matt wants Linux and Windows within months of launch.

## References

- Platform decision locked: MEMORY.md
- Tauri docs: https://tauri.app
- Related: #514 (headless E2E testing limitation)

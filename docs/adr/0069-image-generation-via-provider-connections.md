# ADR-0069: Image generation goes through the user's provider connection, into Assets, placed by the brief

**Status:** Proposed (Matt, 2026-11-03)
**Date:** 2026-11-03
**Author:** Matt Carano (decisions)
**Supersedes:** None
**Related:** ADR-0044 (keychain-only secrets), ADR-0065 (Venice provider), §36.5 (proactive withholding), §39 (`docs/spec/runtime.md`)

## Context

The content machine needs headers. Pane has provider connections (OpenRouter, Venice, Ollama, Anthropic, OpenClaw), an attachments store rooted at `Projects/<name>/Assets/`, and a note editor that embeds a fresh image attachment as plain markdown and a resized one as a raw-HTML block (confirmed against `Editor.svelte::serializeAttachmentWrapper`'s two branches — the prior "embeds image attachments as a raw-HTML block" wording overgeneralized the resize-only path, the same error class #3160's audit flagged as non-blocking and this PR corrects). Two providers Pane already talks to can generate images. Nothing in Pane can ask them to.

## Decision

1. **Reuse the connection, never a new key.** Image generation is a capability of an existing provider connection. Settings adds a provider + model picker and nothing else.
2. **The user picks the model.** No default. Unset means the tool refuses with a pointer to Settings. Pane does not choose a paid model on anyone's behalf.
3. **Output is an attachment.** Generated images are ordinary attachments: file in Assets, row in `attachments`, embedded in notes with the same block the drop path writes. No new storage, node type, or sync rule.
4. **Placement is the brief's.** `generate_image` takes `note_id` and `position`; the agent reads the brief and decides. The tool has no opinion about headers.
5. **Spend is capped on background turns.** Job turns get `max_images_per_turn` (default 4); interactive turns are uncapped. Cost is logged per call and summarised in the job receipt.
6. **OpenClaw agents use Pane's adapter** (C28(b)). No OC-side image path.
7. **No local provider this sprint.** Ollama has no image HTTP API; local servers that do (Automatic1111, ComfyUI) are a post-launch "OpenAI-compatible image endpoint" connection.

## Consequences

- One new tool, two settings keys, two thin adapters, no migration.
- The content machine's 7.x walk becomes the acceptance for both this sprint and the jobs UX.
- C50 (triple key entry on the AI tab) stays open; this sprint must not make it worse.

## Alternatives Considered

- A Pane-hosted image service: contradicts local-first and bring-your-own-keys.
- Default model chosen by Pane: silently spends the user's money on a model they never picked; also goes stale.
- A dedicated `images` table / new note node: duplicates attachments for no gain; breaks backup/sync parity.
- Hardcoding "header" placement: wrong for social posts, sheets, and anything not an article.
- Image editing / vision in the same sprint: separate capabilities, separate risks; vision is roadmap 2.4.

## References

- Spec section: §39 (`docs/spec/runtime.md`)
- Related ADRs: ADR-0044 (keychain-only secrets), ADR-0065 (Venice provider)
- Spec review: `docs/audits/2026-11-03-review-spec-39-image-gen.md` (Reed, 2026-11-03 — corrections from this review are applied in §39 and in this ADR's Context/Decision text)

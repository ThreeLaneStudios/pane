# ADR-0011: System Notes are Read-Only in the Pane Editor; Agents Settings is the View Surface

**Status:** Accepted
**Date:** 2026-04-24
**Author:** Matt Carano, Claude (session)

## Context

Cluster C of the ADR-0010 rollout drafted §5.7.2 to say that memory files and daily logs open read-only in the Pane editor while "Soul, Rules, Identity, User, Tools — remain user-editable in the Pane editor." That asymmetry had a clean architectural justification for the read-only side (under ADR-0010, `ai_memory` is the source of truth for memory content; an editable surface on a rendered view would silently lose data at the next consolidation). It had no architectural justification for the editable side — it was simply preserved as-was while we focused the cluster on memory.

Investigation during the Cluster C dev-session review revealed that the existing code does not match the drafted spec. Commit `48f5939` (March 14, v11.21.0) locked **all seven** `note_type`-tagged system notes read-only in the Pane editor: memory, daily, soul, rules, identity, user, tools. The change was bundled with an unrelated synthesis revert. There is no linked issue, no Why This Exists, no decision doc, and no architectural rationale recorded anywhere. The behavior was drifted into, not decided.

Two paths forward were possible:

1. **Match the spec to the code.** Lock all seven types read-only deliberately, with reasoning recorded.
2. **Match the code to the spec.** Open Soul, Rules, Identity, User, Tools back up to editor editing.

Brit framed the underlying tradeoff in the dev session: a **uniform mental model** ("system notes are AI-managed, settings is where you view them") versus **customizability** ("power users want to hand-edit their Soul or Rules without leaving the editor"). Customizability has been a stated future direction, but no concrete UI design exists for it, no issue is open, and the code has been read-only since March 14 without any user complaint about Soul/Rules editability. The compelling failure mode for memory and daily files (silent overwrite on consolidation) does not exist for Soul/Rules/Identity/User/Tools — those are written by user actions through the onboarding wizard and settings flows, not by a pipeline that re-renders them. But the absence of a failure mode is not the same as a reason to keep two surfaces.

Matt's call: pick the uniform model. Lock all seven read-only. Make Agents settings the view surface for system notes, with per-file token cost displayed alongside content. Document the HD filesystem (`~/Pane/.pane/system/{agent}/`) as the escape valve for power users. Defer in-editor system-note editing to a future ADR if and when a real customization need emerges.

## Decision

All notes with a non-null `note_type` open **read-only** in the Pane editor. This applies uniformly to:

- `note_type = 'memory'` — agent MEMORY.md, project Memory notes
- `note_type = 'daily'` — daily logs
- `note_type = 'soul'`
- `note_type = 'rules'`
- `note_type = 'identity'`
- `note_type = 'user'`
- `note_type = 'tools'`

The **Agents settings page** is the supported view surface for system notes. It displays the rendered content of each system file alongside its **per-file token cost**, giving users visibility into a number they currently cannot see — the cost their system files contribute to every conversation context.

The HD filesystem at `~/Pane/.pane/system/{agent}/` is the **documented escape valve** for power users who need to edit a system note's flat-file source directly. This is unsupported by the UI but not blocked by it; the files exist, they are readable and writable, and a user who knows what they are doing can edit them outside Pane.

User notes (any note with `note_type IS NULL`) remain fully editable in the Pane editor. This decision concerns only `note_type`-tagged system notes.

Future work to introduce in-editor editing for any system note type requires its own ADR. This decision deliberately closes that door for now rather than leaving it ambiguous.

## Consequences

### Positive

- **Uniform mental model.** Users learn one rule about system notes — they are AI-managed, viewed in Agents settings, edited via their respective specialized surfaces (Memory Settings for memory, onboarding wizard for Soul, etc.). No per-type rule to remember.
- **No silent overwrite risk on any system file.** Even though the failure mode is currently specific to memory and daily, locking all system notes read-only forecloses the possibility that a future pipeline operating on Soul/Rules/Identity could introduce the same trap.
- **Clear surface for viewing system files.** Agents settings becomes the canonical "what does my AI currently see about itself" page, replacing the implicit "Open in Editor" workflow that mixed view and edit affordances.
- **Token cost visibility.** Per-file token cost is information users currently lack. Surfacing it in Agents settings turns "is my Soul too long" from a guess into a number, which is useful both for users on token-constrained tiers (OpenClaw 12K, BYOK budgets) and for diagnosing consolidation behavior.
- **Cluster C internal inconsistency resolved.** §5.7.2 no longer says one thing and the code does another.

### Negative

- **No in-editor Soul/Rules customization without HD access.** A user who today wants to hand-edit their Soul to add a single line cannot do it from the Pane editor; they must use the onboarding wizard (which is not designed for incremental edits) or open the flat file outside Pane. This is a real loss in flexibility for power users who want to tinker.
- **Onboarding-time edits remain wizard-only.** The wizard is the only Soul/Rules write surface inside Pane today. Until a dedicated edit UI is built, users who want to refine these files post-onboarding have a clunky path.
- **Future UI edit work is deferred.** This decision says "not now" to in-editor system-note editing. If a customization need emerges, building it will require its own ADR plus the UI work; we are explicitly trading near-term simplicity for a known future cost.
- **HD filesystem as escape valve is unsupported.** Users who edit flat files directly bypass any validation, formatting, or token-cost checks. They can produce malformed Soul or Rules content that the AI then reads. We document the path but do not protect against its consequences.

### Risks

- **"View-only Agents settings" is more useful than "Edit Memory Settings."** If users start treating Agents settings as the place to do all system-note operations and find it lacking (because it doesn't edit Soul/Rules), the discoverability of the wizard and Memory Settings as the actual write surfaces could degrade. Mitigated by clear UI labels in Agents settings explicitly directing users to the right place for changes.
- **Token cost display becomes a feature people want to act on but cannot.** A user seeing "your Soul is 8,400 tokens" with no in-app way to trim it may experience the visibility as frustrating rather than useful. Mitigated by ensuring the wizard or a future Soul-edit surface lands before this becomes a sustained complaint, and by linking from the token cost display to the relevant edit surface (wizard for Soul, Memory Settings for memory, etc.).
- **Drift recurrence.** The original problem here was that code drifted from spec without a decision trail. Locking the policy in an ADR mitigates that for this specific decision, but does not prevent future drift. The general-purpose mitigation is the dev-hygiene playbook's spec-first rule, which this ADR reinforces by example.

## What if we didn't decide

- **Cluster C ships as-drafted, code stays stricter than spec.** §5.7.2 says "Soul/Rules/Identity/User/Tools remain user-editable" while commit `48f5939` keeps them read-only. Future engineers reading the spec see one thing and observe another in the code, and someone eventually opens an issue to "fix" the discrepancy — without knowing which side is correct or who decided it.
- **Or someone "fixes" the code to match the spec** by removing the read-only check on the five non-memory system note types. This re-opens Soul/Rules/Identity/User/Tools to editor edits with no architectural decision behind it, just a perceived inconsistency. The code change ships, no ADR exists, and we are back in the same state we started in — a behavior that drifted into existence with no rationale.
- **Either path leaves the underlying question unresolved:** is the uniform read-only rule a mistake to undo, or a deliberate choice to keep? Without this ADR, that question is answered by whoever happens to push a fix first.

## Alternatives considered

**All editable in editor.** Match the spec to the original Cluster C draft by opening all seven system note types to editor edits. Rejected: the editor would silently overwrite memory and daily file edits on the next consolidation pass. ADR-0010's source-of-truth model only works if the rendered files are not also edit surfaces.

**Memory/daily read-only, others editable (Cluster C as-drafted).** Lock the two types where silent overwrite is a real risk; allow editor edits on Soul/Rules/Identity/User/Tools. Rejected: a split mental model with no compelling architectural reason for the asymmetry. The code already enforces uniform read-only as of March 14, and reverting that without a decision trail recreates the original drift problem in the opposite direction.

**Per-note-type policy with mixed rules.** Define independent edit/view rules for each `note_type` based on its specific use case (e.g., daily logs strictly read-only, Soul read-only-with-warning, Rules read-write-with-validation). Rejected: complexity not justified. None of the system note types currently has a use case that demands its own rule, and a per-type policy creates seven different mental models for users to learn.

**Editable with a "this will be overwritten" warning banner.** Open system notes for editing but display a warning that the content may be regenerated. Rejected: warning fatigue solves nothing — users either learn to dismiss the warning and lose data anyway, or learn to never use the feature, in which case it should not exist. The earlier Cluster C draft used this pattern for "Open in Editor" and it was tightened away during dev-session review for exactly this reason.

## References

- §5.7.2 (Cluster C: Pane editor read-only for system notes)
- ADR-0010 (Three-writer memory pipeline; established `ai_memory` as source of truth, which is the architectural foundation for read-only memory and daily files)
- Commit `48f5939` (March 14, v11.21.0) — the original drifted-into uniform read-only behavior in code
- Cluster C investigation thread (dev-session 2026-04-24) — surfaced the spec/code inconsistency and produced this decision
- #682 — Future Agents settings page issue (owns per-file token cost display, view-surface UI, and links to write surfaces)
- `docs/spec/memory-pipeline.md` §5.7, §5.12 — the specs this ADR clarifies
- INVARIANTS.md §5 — "Pane editor opens system notes read-only" bullet (already merged with ADR-0010, broader application now codified here)

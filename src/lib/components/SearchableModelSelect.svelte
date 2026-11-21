<script lang="ts">
  import { highlightText } from '$lib/utils/markdown';
  import PromptDialog from './PromptDialog.svelte';
  import SearchableSelect from './SearchableSelect.svelte';

  interface Model {
    id: string;
    name: string;
    provider: string;
    tier_label?: string;  // Venice per-model privacy label (ADR-0065)
    // #2988 PR2: Venice capability flags, Rust-literal field names (no
    // camelCase translation at the Rust↔TS boundary — see ModelInfo's own
    // doc comments in provider.rs). Absent/undefined renders identically to
    // `false` below — never "supported." `supports_function_calling` is
    // intentionally NOT in this interface: it's not rendered as a picker tag
    // in this PR (Matt's ruling — carried through the backend struct for a
    // future tool-eligibility filter, not surfaced here).
    supports_tee_attestation?: boolean;
    supports_e2ee?: boolean;
  }

  interface Props {
    models: Model[];
    selectedModel: string;
    placeholder?: string;
    onSelect: (modelId: string) => void;
    // #3083: the canonical lowercase runtime/provider id for the models in
    // `models` (e.g. 'openrouter', 'venice', 'anthropic', 'ollama') — NOT the
    // same thing as `Model.provider` above, which is a free-form per-model
    // display string (existing fixtures/rows use display casing like
    // "OpenRouter"/"Venice"). Every call site already tracks this value in a
    // state variable to fetch/filter `models` in the first place (e.g.
    // Settings.svelte's `internalProvider`/`ssProvider`, Onboarding.svelte's
    // `connectProvider`/`sfProvider`, AgentCreateOverlay.svelte's
    // `selectedRuntimeType`, Settings.svelte's `agent.runtime_type`) — this
    // just threads that existing typed value through as a prop instead of
    // inferring it from array contents. Used solely to decide whether the
    // custom-model-entry box is shown (see `showCustomModelBox` below).
    // Optional + defaults to hidden (see below) so a call site that omits it
    // fails safe (no box) rather than silently exposing one.
    provider?: string;
  }

  let { models, selectedModel = '', placeholder = 'Search models...', onSelect, provider }: Props = $props();

  // #3083: custom-model free-text entry is only safe where the provider's own
  // model list is itself incomplete by design — OpenRouter aggregates a huge,
  // constantly-growing third-party catalog, so a model Pane hasn't indexed
  // yet is a real, common case ("open namespace"). Venice/Anthropic/Ollama
  // each return their own complete, authoritative live list on every fetch —
  // there, the fetched list IS the full truth, so a typed name that isn't in
  // it is almost certainly a typo, not a real unlisted model. No existing
  // boolean/config signal distinguishes these today (ModelInfo carries
  // capability flags, not a "catalog is open" flag), so this is a single
  // named, documented allowlist rather than a magic string scattered at each
  // call site. Unknown/omitted provider defaults closed (see Props.provider
  // doc above) — the safer failure mode is "no box" (nothing the user can't
  // already do another way) over "box shown where it can create a typo'd
  // ghost model."
  const OPEN_NAMESPACE_PROVIDERS = new Set(['openrouter']);

  let showCustomModelBox = $derived(provider !== undefined && OPEN_NAMESPACE_PROVIDERS.has(provider));

  let customModelPromptOpen = $state(false);
  // Set at the moment the custom-entry row is clicked (inside the `footer`
  // snippet below, which SearchableSelect calls with its own
  // `closeDropdown` each time the dropdown is open) — `submitCustomModel`/
  // cancellation use it to close SearchableSelect's open+search state the
  // same way a normal row click already does via its own `selectItem`.
  let pendingClose: (() => void) | null = null;

  // #3171 PR2 (C51/C57): core search/positioning/selection mechanics now
  // live in the generic SearchableSelect.svelte — this component owns only
  // the model-specific shape: capability-tag rendering/search, the
  // provider+name sort, and the open-namespace custom-model box. See
  // SearchableSelect.svelte's own header comment for the split rationale.

  // #3083: the rendered capability-tag text for a model, as search terms.
  // Mirrors the exact render guards in `renderItem` below — same two
  // independent `if (flag)` checks, same Matt-ruled, test-pinned label
  // strings ("TEE" / "E2EE not supported in Pane", never a "Private" badge).
  // `tier_label` is pushed verbatim (it's already a free-text field, not a
  // boolean, so there's no label mapping to do). `supports_function_calling`
  // is deliberately NOT included: #2988 PR2 ruled it is not rendered as a
  // picker tag in this PR (carried through the struct for a future
  // tool-eligibility filter, not surfaced here) — making it a *search* term
  // while it is invisible everywhere else in the row would let a user search
  // for a capability Pane never shows them, which is its own kind of lie.
  // If a future PR adds a rendered tag for it, add it here the same way.
  //
  // `if (m.supports_tee_attestation)` / `if (m.supports_e2ee)` are false for
  // both `undefined` (absent/never fetched) and `false` (explicitly
  // unsupported) — identical truthiness rule as the render guards — so an
  // absent flag structurally cannot contribute a search term. This is the
  // same absent-never-means-capable guarantee #3075/#2988 established for
  // rendering, now also holding for search (the #3060 failure mode one layer
  // down, per #3083's own framing).
  function capabilityLabels(m: Model): string[] {
    const labels: string[] = [];
    if (m.tier_label) labels.push(m.tier_label);
    if (m.supports_tee_attestation) labels.push('TEE');
    if (m.supports_e2ee) labels.push('E2EE not supported in Pane');
    return labels;
  }

  // Resolves the closed-state input value for `selectedModel` — a listed
  // model's own name, or the raw typed id verbatim for a custom selection
  // not present in `models` (unchanged behavior from pre-extraction).
  function getDisplayText(id: string): string {
    if (!id) return '';
    const found = models.find((m) => m.id === id);
    if (found) return found.name;
    return id; // Custom model
  }

  // #3083: tag-aware search — matches name/provider/id exactly as before,
  // PLUS tier_label and the rendered capability-tag strings (TEE/E2EE), so
  // typing "TEE" finds models carrying that flag even when id/name don't
  // contain the substring.
  function filterItem(item: { id: string }, lowerSearch: string): boolean {
    const m = item as Model;
    return (
      m.name.toLowerCase().includes(lowerSearch) ||
      m.provider.toLowerCase().includes(lowerSearch) ||
      m.id.toLowerCase().includes(lowerSearch) ||
      capabilityLabels(m).some((label) => label.toLowerCase().includes(lowerSearch))
    );
  }

  function sortItems(items: { id: string }[]): { id: string }[] {
    return [...(items as Model[])].sort((a, b) => {
      const providerCompare = a.provider.localeCompare(b.provider);
      if (providerCompare !== 0) return providerCompare;
      return a.name.localeCompare(b.name);
    });
  }

  function handleCustomModelClick(close: () => void) {
    pendingClose = close;
    customModelPromptOpen = true;
  }

  function submitCustomModel(name: string) {
    customModelPromptOpen = false;
    onSelect(name);
    pendingClose?.();
    pendingClose = null;
  }

  function cancelCustomModel() {
    customModelPromptOpen = false;
    pendingClose = null;
  }
</script>

{#snippet renderItem(item: unknown, searchText: string)}
  {@const model = item as Model}
  <div class="provider">{model.provider}</div>
  <div class="model-name">{@html highlightText(model.name, searchText)}</div>
  {#if model.tier_label}
    <span class="model-tier-tag">{model.tier_label}</span>
  {/if}
  <!-- #2988 PR2: two independent {#if} blocks, not an else-chain —
       per Venice's schema, supportsE2EE requires supportsTeeAttestation
       to also be true, so a model can legitimately show both tags.
       An else-chain would silently hide one. Copy is Matt-ruled,
       deliberately conservative, and not up for rewording: "TEE"
       is honest today because Pane's existing plain Venice call
       already benefits from TEE's hardware-enclave protection
       regardless of headers sent. "E2EE not supported in Pane" —
       never a "Private"/E2EE-implying badge — because Pane sends
       zero of Venice's E2EE handshake (no X-Venice-TEE-* headers
       anywhere in this codebase); Matt's e2ee-glm-5-3-p tool-use
       test proved tools work on a PLAIN request, which proves
       nothing about E2EE itself. -->
  {#if model.supports_tee_attestation}
    <span class="model-tier-tag">TEE</span>
  {/if}
  {#if model.supports_e2ee}
    <span class="model-tier-tag">E2EE not supported in Pane</span>
  {/if}
{/snippet}

{#snippet customModelFooter(close: () => void)}
  <button
    type="button"
    class="dropdown-item custom"
    onmousedown={(e) => {
      e.preventDefault();
      handleCustomModelClick(close);
    }}
  >
    Enter custom model name...
  </button>
{/snippet}

<SearchableSelect
  items={models}
  selectedId={selectedModel}
  placeholder={placeholder}
  onSelect={onSelect}
  {getDisplayText}
  {filterItem}
  {sortItems}
  {renderItem}
  footer={showCustomModelBox ? customModelFooter : undefined}
  containerClass="searchable-model-select"
  inputClass="model-input"
/>

{#if customModelPromptOpen}
  <PromptDialog
    title="Custom Model"
    message="Enter the model identifier to use."
    placeholder="e.g. gpt-4o-custom"
    confirmLabel="Select"
    onConfirm={submitCustomModel}
    onCancel={cancelCustomModel}
  />
{/if}

<style>
  .dropdown-item .provider {
    color: #666666;
    font-size: 12px;
    margin-bottom: 2px;
  }

  .dropdown-item .model-name {
    color: #F0EDE8;
  }

  .dropdown-item .model-name :global(mark) {
    background: rgba(0, 228, 228, 0.3);
    color: #00E4E4;
  }

  :global(.dropdown-item.selected) .model-name {
    color: #00E4E4;
  }

  .dropdown-item.custom {
    border-top: 2px solid rgba(255, 255, 255, 0.08);
    color: #A0A0A0;
    font-size: 13px;
    padding: 12px;
  }

  .dropdown-item.custom:hover {
    color: #00E4E4;
  }
</style>

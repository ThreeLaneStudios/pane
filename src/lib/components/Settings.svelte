<script lang="ts">
  import { getVersion } from '@tauri-apps/api/app';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { open } from '@tauri-apps/plugin-dialog';
  import { check, type Update } from '@tauri-apps/plugin-updater';
  import { relaunch } from '@tauri-apps/plugin-process';
  import * as api from '$lib/api';
  import type { CategoryWithColor, Template } from '$lib/types';
  import { DEFAULT_CAPTAIN_LABEL } from '$lib/constants';
  import { canvasState } from '$lib/stores/canvas';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import { isAuthenticated, userEmail, signIn, signUp, signOut, isAccountEnabled } from '$lib/stores/authStore';
  import { autoSyncEnabled, startAutoSync, stopAutoSync, lastSyncTime } from '$lib/stores/syncStore';
  import { subscribeSyncStatus, getSyncHistory, syncWithPeer } from '$lib/sync';
  import { projects, activeProject, activeFolder, activeNote, activeTaskId } from '$lib/stores/app';
  import { renderMarkdown } from '$lib/utils/markdown';
  import SearchableModelSelect from './SearchableModelSelect.svelte';
  import { normalizeOllamaEndpoint } from '$lib/utils/ollama';
  import { parseStoredTimestamp } from '$lib/utils/time';
  import AgentCreateOverlay from './AgentCreateOverlay.svelte';
  import AgentPickerModal from './AgentPickerModal.svelte';
  import ProviderConnectionsPanel from './ProviderConnectionsPanel.svelte';
  import DefaultAgentPickerModal from './DefaultAgentPickerModal.svelte';
  import type { Agent } from '$lib/types';
  import { isAgentModelEditable } from '$lib/utils/agentDisplay';
  import { getAgents, setMainAgent as apiSetMainAgent, listRuntimeConnections, type RuntimeConnectionInfo } from '$lib/api';
  import { REASON_CREATED, ENTITY_CATEGORY, subscribeDataChanged } from '$lib/refreshEvent';

  // Tab state
  let activeTab = $state('general');

  // === TEMPLATES TAB (§32.4) ===
  // spec-covered-by: #2121 (§32, ADR-0058)
  let settingsTemplates: Template[] = $state([]);
  let templatesLoading = $state(false);
  let templatesLoaded = $state(false);
  // Rename state
  let renamingTemplateId: string | null = $state(null);
  let renameValue = $state('');
  // Delete confirm (tpl_ prefix to avoid collision with memory deleteConfirmId at line 280)
  let tplDeleteConfirmId: string | null = $state(null);
  let tplDeleteConfirmName = $state('');
  let tplDeleteConfirmError: string | null = $state(null);

  async function loadTemplates() {
    templatesLoading = true;
    try {
      settingsTemplates = await api.listTemplates();
    } catch (e) {
      console.error('[Settings/Templates] Failed to load:', e);
    } finally {
      templatesLoading = false;
      templatesLoaded = true;
    }
  }

  function startRename(t: Template) {
    renamingTemplateId = t.id;
    renameValue = t.name;
  }

  async function commitRename(id: string) {
    const name = renameValue.trim();
    if (!name) { cancelRename(); return; }
    try {
      await api.renameTemplate(id, name);
      await loadTemplates();
    } catch (e) { console.error('[Settings/Templates] Rename failed:', e); }
    renamingTemplateId = null;
  }

  function cancelRename() {
    renamingTemplateId = null;
    renameValue = '';
  }

  function startDelete(t: Template) {
    tplDeleteConfirmId = t.id;
    tplDeleteConfirmName = t.name;
    tplDeleteConfirmError = null;
  }

  // #2874 item G: surfaces a delete failure in the dialog itself
  // (ConfirmDialog's `error` prop) instead of only a console.error that
  // silently closed the modal — same pattern applied to Trash.svelte's
  // conversion in this PR.
  async function confirmDeleteTemplate() {
    if (!tplDeleteConfirmId) return;
    try {
      await api.deleteTemplate(tplDeleteConfirmId);
      await loadTemplates();
      tplDeleteConfirmId = null;
      tplDeleteConfirmName = '';
      tplDeleteConfirmError = null;
    } catch (e) {
      console.error('[Settings/Templates] Delete failed:', e);
      tplDeleteConfirmError = e instanceof Error ? e.message : 'Delete failed. Please try again.';
    }
  }

  function cancelDeleteTemplate() {
    tplDeleteConfirmId = null;
    tplDeleteConfirmName = '';
    tplDeleteConfirmError = null;
  }

  function formatTplDate(iso: string): string {
    try { return new Date(iso).toLocaleDateString('en-US', { month: 'short', day: 'numeric', year: 'numeric' }); }
    catch { return iso; }
  }

  // Load templates when Templates tab is opened
  $effect(() => {
    if (activeTab === 'templates' && !templatesLoaded && !templatesLoading) {
      loadTemplates();
    }
  });
  
  // === GENERAL TAB ===
  let vaultPath = $state('');
  let vaultPathSaved = $state(false);
  let vaultPathError = $state('');
  
  let definedCategories: CategoryWithColor[] = $state([]);
  let newCategoryName = $state('');
  let newCategoryColor = $state('#00E4E4');
  let editingCategoryIdx: number | null = $state(null);
  let editingCategoryName = $state('');
  let editingCategoryColor = $state('');
  const presetColors = ['#00E4E4', '#F87171', '#34D399', '#FBBF24', '#60A5FA', '#888888', '#FB923C', '#A855F7'];
  
  let isImporting = $state(false);
  let importResult = $state('');
  let folderScanResult: any = $state(null);
  let folderImportPath = $state('');
  let isFolderImporting = $state(false);
  let showFolderPreview = $state(false);
  let expandedFolders: Set<string> = $state(new Set());
  let importToast = $state("");
  let importToastTimeout: any = $state(null);
  
  let editorFontSize = $state('15.5');
  let appZoom = $state(1);
  
  // === AI TAB ===
  let provider = $state('none');
  let apiKey = $state('');
  let model = $state('');
  let endpoint = $state('http://localhost:11434');
  let loading = $state(false);
  let errorMessage = $state('');
  let successMessage = $state('');
  let ollamaModels = $state<{ id: string; display: string }[]>([]);
  let testingConnection = $state(false);
  let openclawUrl = $state('');
  let openclawToken = $state('');
  let showCustomModelInput = $state(false);
  let selectedModelOption = $state(''); // Track dropdown selection separately
  let savedProvider = $state(''); // The actually configured/saved provider
  let savedConnectionStatus = $state<'connected' | 'failed' | 'not-tested'>('not-tested');
  
  // Model dropdown state (for openclaw provider)
  let availableModels: { id: string; name: string; provider: string }[] = $state([]);
  let customModelName = $state('');

  let pairingFingerprint = $state('');

  let savedOpenclawUrl = $state('');
  let savedOpenclawToken = $state('');
  
  // Simple tier model dropdown state
  let simpleTierModel = $state('');
  let selectedSimpleTierModelOption = $state('');
  let customSimpleTierModelName = $state('');
  let extractionModel = $state('');
  
  // §29: Internal Provider (dual-provider routing)
  // C50 (#3171 PR 1): no key field — openrouter/anthropic/venice each read
  // their key from the matching Provider Connection (same shape as §39.2's
  // image-generation picker). The local key-input state this section used to
  // hold is retired along with the key field it fed.
  let internalProvider = $state('');
  let internalEndpoint = $state('http://127.0.0.1:11434');
  let testingInternalConnection = $state(false);
  let internalConnectionStatus = $state<'not-tested' | 'success' | 'error'>('not-tested');
  let internalConnectionMessage = $state('Not tested');
  let internalOllamaErrorClass = $state<string | null>(null);
  // #2641 D14 Commit 2: "configured this install" marker for the Smart Features status
  // dot (Commit 5), same pattern as {runtime}_configured_at for the provider cards —
  // written ONLY by saveInternalProvider()'s Save path, cleared on provider change.
  // Null/empty means never configured this install (grey), regardless of whether a
  // leftover internal_api_key exists in ai_settings from a previous install.
  let internalConfiguredAt = $state<string | null>(null);

  // #2641 D14 Commit 5: three-state status dot for the Smart Features section title,
  // mirroring ProviderConnectionCard.svelte's cardStatus derivation:
  //   grey  = !internalConfiguredAt (never configured this install)
  //   green = internalConfiguredAt present && internalConnectionStatus !== 'error'
  //   red   = internalConfiguredAt present && internalConnectionStatus === 'error'
  const internalStatusDotClass = $derived(
    !internalConfiguredAt
      ? 'grey'
      : internalConnectionStatus === 'error'
        ? 'red'
        : 'green'
  );

  // #2795 follow-up / C50 (#3171 PR 1): per-feature Smart Features config
  // (Extraction/Rerank/Smart Routing model pickers) must render whenever the
  // SELECTED provider is actually usable — not only after a fresh Save & Test
  // click sets internalConnectionStatus to 'success'. On mount,
  // internalConnectionStatus always starts 'not-tested' (§28.1 invariant
  // above), so gating the config section on connection status alone means a
  // returning user re-opening Settings sees no model pickers until they
  // re-test — same false-gating shape as the old Test-button bug (#2631).
  //   openrouter/anthropic/venice -> a provider is only EVER selectable here
  //   if `internalProviderOptions` already filtered it to "connected" (C50:
  //   no key field — Provider Connections already validated the key), so
  //   having picked one at all means it's usable.
  //   ollama -> no Provider Connection credential involved; still requires an
  //   explicit, successful probe (internalConnectionStatus === 'success').
  const internalIsConnected = $derived(
    !internalProvider
      ? false
      : internalProvider === 'ollama'
        ? internalConnectionStatus === 'success'
        : true
  );
  
  // Internal provider models (for simple_tier_model and memory_search_llm_rerank_model pickers)
  let internalProviderModels: { id: string; name: string; provider: string }[] = $state([]);
  
  // No hardcoded defaults — only populate from DB or leave empty
  const defaultModels: Record<string, string> = {
    anthropic: '',
    openrouter: '',
    ollama: '',
    openclaw: '',
  };
  
  // Per-provider model storage
  let providerModels: Record<string, string> = $state({
    anthropic: '',
    openrouter: '',
    ollama: '',
    openclaw: '',
  });
  
  // === AGENTS TAB ===
  let agentsList = $state<Agent[]>([]);
  let agentsLoaded = $state(false);
  let editingAgentIdx = $state<number | null>(null);
  let editingAgentName = $state('');
  // #3074: inline model-edit state — mirrors editingAgentIdx/editingAgentName above
  // but for the model field, gated by isAgentModelEditable() (agentDisplay.ts) at
  // the render call site, not re-derived here.
  let editingAgentModelIdx = $state<number | null>(null);
  let agentModelOptions = $state<
    Array<{
      id: string;
      name: string;
      provider: string;
      tier_label?: string;
      supports_tee_attestation?: boolean;
      supports_e2ee?: boolean;
    }>
  >([]);
  let loadingAgentModelOptions = $state(false);
  let agentModelLoadError = $state('');
  // #2030 PR B: two-group view, overlay, picker
  let showAgentCreate = $state(false);
  let agentCreateForced = $state(false);
  let agentCreatePendingProvider = $state('');
  let showAgentPicker = $state(false);
  // #2030 PR B Fix B: deferred provider save — captured before picker/forced-create, consumed on confirm
  let pendingProviderSave = $state<{ provider: string; apiKey: string | null; model: string | null; endpoint: string | null; simpleTierModel: string | null } | null>(null);
  // #2522 Stage 6a, ADR-0063: the Agents tab groups by runtime_type instead of the retired
  // "Locally created" / "Imported from OpenClaw" split (README-group-b.md §7 "Agents page").
  // agentsConnectedRuntimes drives the one remaining unavailable state — provider-disconnected —
  // which dims a group's rows and states the cause, per the same ruling.
  let agentsConnectedRuntimes = $state<Set<string>>(new Set());
  const RUNTIME_TYPE_LABELS: Record<string, string> = {
    openclaw: 'OpenClaw',
    openrouter: 'OpenRouter',
    anthropic: 'Anthropic',
    ollama: 'Ollama',
    venice: 'Venice',
  };
  const RUNTIME_TYPE_ORDER = ['openclaw', 'openrouter', 'anthropic', 'ollama', 'venice'];
  // C3 follow-up (#2587): deactivated legacy agents (is_active = 0 — e.g. `migrate_v83`'s
  // one-time deactivation of runtime_type IS NULL rows) are already excluded from every
  // picker surface (filterConnectedAgents' is_active gate, B5/C4) but the Agents tab itself
  // read straight off agentsList with no is_active filter, so a deactivated row was still
  // the one place they kept rendering. activeAgentsList closes that last surface — every
  // Agents-tab read (the roster groups below AND the header count) goes through this,
  // not the raw agentsList, so the two can never drift out of sync with each other.
  const activeAgentsList = $derived(agentsList.filter(a => a.is_active));
  const agentGroups = $derived.by(() => {
    const groups = new Map<string, Agent[]>();
    for (const agent of activeAgentsList) {
      const key = agent.runtime_type ?? 'unknown';
      const bucket = groups.get(key);
      if (bucket) bucket.push(agent); else groups.set(key, [agent]);
    }
    const keys = [...groups.keys()].sort((a, b) => {
      const ai = RUNTIME_TYPE_ORDER.indexOf(a);
      const bi = RUNTIME_TYPE_ORDER.indexOf(b);
      if (ai !== -1 && bi !== -1) return ai - bi;
      if (ai !== -1) return -1;
      if (bi !== -1) return 1;
      return a.localeCompare(b);
    });
    return keys.map(runtimeType => ({
      runtimeType,
      label: RUNTIME_TYPE_LABELS[runtimeType] ?? runtimeType,
      connected: agentsConnectedRuntimes.has(runtimeType),
      agents: groups.get(runtimeType)!,
    }));
  });
  // Legacy provider-switch flow (AgentPickerModal, pre-#2522 Stage 6a): unrelated to the
  // Agents-tab regroup above — kept narrowly scoped to that one existing call site.
  const locallyCreatedAgentsForPicker = $derived(agentsList.filter(a => a.runtime_ref == null));
  // #2522 design polish batch, item 3: current default agent for the AI tab's Default
  // agent row — same is_default flag DefaultAgentPickerModal already writes/reads.
  const currentDefaultAgent = $derived(agentsList.find(a => a.is_default) ?? null);
  // #2522 Stage 5d: cross-provider "Default agent" picker (DefaultAgentPickerModal) — the ONE
  // place Settings offers a default-agent comparison across every provider. See
  // docs/design/agent-sessions-sprint/README-group-b.md "Set-as-default lives in two places".
  let showDefaultAgentPicker = $state(false);
  let defaultPickerConnectedRuntimes = $state<Set<string>>(new Set());
  let defaultPickerLoading = $state(false);
  
  // === MEMORIES TAB ===
  let memoryEnabled = $state(true);
  let synthesisEnabled = $state(true);
  let dailyLogEnabled = $state(true);
  let showMemoryToasts = $state(true); // §5.4B: Controls memory extraction toast (#641)
  let memoryCount = $state(0);
  let lastSynthesisTime = $state('2 hours ago');
  
  // Memory search settings
  let searchFuseBothSources = $state(true);
  let searchKeywordWeight = $state(0.3);
  let searchTemporalHalfLife = $state(7);
  let searchLlmRerankEnabled = $state(false);
  let searchLlmRerankModel = $state(''); // No default - must be configured by user
  let searchAvailableModels = $state<string[]>([]);
  let searchFactBoost = $state(1.2);
  
  // === SEMANTIC SEARCH (ADR-0048 / §5.9 / #1657) ===
  let ssEnabled = $state(false);
  let ssProvider = $state('');   // 'openrouter' | 'ollama' | 'venice' | ''
  // C50 (#3171 PR 1): no key field — openrouter and venice each read their
  // key from the matching Provider Connection (`ssProviderOptions`, same
  // shape as §39.2's image-generation picker). The local key-input/probe
  // state this section used to hold is retired along with the field it fed;
  // "connected" status for both now comes from `ssProviderOptions` directly.
  let ssEndpoint = $state('http://localhost:11434');
  let ssModel = $state('');
  // Status payload from embeddings://status event or durable DB read
  let ssStatus = $state<{
    stage: string;
    notes?: number; facts?: number; chats?: number; failed?: number;
    notes_total?: number; facts_total?: number; chats_total?: number;
    reason?: string;
  } | null>(null);
  let ssSaving = $state(false);
  // #2776: context_length is threaded from OpenRouter's model list (ModelInfo.context_length,
  // already fetched here) through to switch_embedding_engine's max_tokens param so the
  // chunker can truncate to the model's real token limit instead of a hardcoded value.
  // Ollama models never carry a context_length here (no live API source) — the Rust side
  // falls back to its static seed table / conservative floor for those.
  let ssModels = $state<{ id: string; name: string; provider: string; context_length?: number | null }[]>([]);

  // #3161 PR 1 (§39.2) — Settings -> AI -> Image generation. Shaped like
  // Semantic Search above but with no key field (C50: the AI tab already
  // asks for the OpenRouter key elsewhere). `igProvider`/`igModel` are the
  // two new `ai_settings` keys (`image_provider`/`image_model`), both
  // null-default until the user picks (no default chosen on their behalf).
  let igProvider = $state('');
  let igModel = $state('');
  let igModels = $state<{ id: string; name: string; price_usd?: number | null }[]>([]);
  let igLoadingModels = $state(false);
  let igModelsError = $state('');
  // #3171 PR 2 (C51/C57): the image-model picker now reuses the SAME
  // searchable component the AI-tab model pickers already use
  // (SearchableModelSelect, backed by the generic SearchableSelect) —
  // Matt's ruling, "it has to be the same component the model pickers
  // use." This closes the exact anti-pattern #3168 introduced elsewhere
  // (a search box floating above a native `<select>`), which this
  // picker itself had fallen into (`igModelSearch`/`igFilteredModels`,
  // now removed — SearchableModelSelect does its own filtering).
  // `igModelLabel` maps the provider's bare {id, name, price_usd} shape
  // into SearchableModelSelect's required `{id, name, provider}` Model
  // shape, folding the price (previously a second `${...}/image`
  // interpolation inside the removed `<option>` text) into `name`
  // itself so it's still visible in the row — SearchableModelSelect's
  // Model interface has no dedicated price field to pass instead.
  function igProviderLabel(): string {
    return igProvider === 'openrouter' ? 'OpenRouter' : igProvider === 'venice' ? 'Venice' : igProvider;
  }
  let igSearchableModels = $derived(
    igModels.map((m) => ({
      id: m.id,
      name: m.price_usd != null ? `${m.name} ($${m.price_usd.toFixed(2)}/image)` : m.name,
      provider: igProviderLabel(),
    }))
  );
  // Connected-providers roster (`ai_list_runtime_connections`, the same
  // command ProviderConnectionsPanel.svelte already uses) — fetched once on
  // mount, shared by every per-feature provider picker in this tab (image
  // generation below, plus Smart Features + Semantic Search — C50 / #3171 PR
  // 1 extends §39.2's own "per-feature provider, no key field" shape to
  // those two sections instead of each re-fetching its own copy).
  let settingsConnections = $state<RuntimeConnectionInfo[]>([]);
  // Compile-time known universe (§39.1: OpenRouter + Venice, PR 2 adds the
  // latter); mirrors the backend's `IMAGE_CAPABLE_PROVIDER_TYPES`.
  const IMAGE_CAPABLE_PROVIDER_TYPES = ['openrouter', 'venice'];
  let igProviderOptions = $derived(
    settingsConnections.filter(c => IMAGE_CAPABLE_PROVIDER_TYPES.includes(c.runtime_type) && c.connected)
  );
  // C50 (#3171 PR 1): Smart Features' key-based provider universe — Ollama is
  // deliberately NOT in this list (it has no key, isn't a Provider Connection
  // credential, and keeps its own independent endpoint field below; always
  // offered regardless of connection state).
  const INTERNAL_FEATURES_KEY_PROVIDER_TYPES = ['openrouter', 'anthropic', 'venice'];
  let internalProviderOptions = $derived(
    settingsConnections.filter(c => INTERNAL_FEATURES_KEY_PROVIDER_TYPES.includes(c.runtime_type) && c.connected)
  );
  // C50 (#3171 PR 1): Semantic Search's key-based provider universe — mirrors
  // `ssProvider`'s existing engine set (openrouter/ollama/venice); Ollama is
  // excluded here too (endpoint, not a key — always offered, independent of
  // connection state).
  const SEMANTIC_SEARCH_KEY_PROVIDER_TYPES = ['openrouter', 'venice'];
  let ssProviderOptions = $derived(
    settingsConnections.filter(c => SEMANTIC_SEARCH_KEY_PROVIDER_TYPES.includes(c.runtime_type) && c.connected)
  );
  // §39.4 / #3161 PR 2 — the per-turn cap, default 4. Stored as a third
  // `ai_settings` key (`max_images_per_turn`), same null-means-default
  // shape as `image_provider`/`image_model` (no migration).
  let igMaxImagesPerTurn = $state(4);
  let igMaxImagesPerTurnSaving = $state(false);
  let igTesting = $state(false);
  let igTestResult = $state<{ ok: boolean; attachmentId?: string; dataUrl?: string; mimeType?: string; error?: string } | null>(null);
  let ssLoadingModels = $state(false);
  // Provider-switch confirm modal state
  let ssShowSwitchModal = $state(false);
  let ssPendingProvider = $state('');
  let ssShowWarn = $state(false); // inline warning strip while switch pending/indexing

  // === LINK EMBEDS (ADR-0050 / #401) ===
  let linkPreviewsEnabled = $state(true); // default true (null in DB → true); key: link_previews_enabled (#1780)

  // === WEB SEARCH (§4.6.8, Part 2, #2591) ===
  let wsBraveKeySaved = $state(false);       // ai_brave_key present in keychain
  let wsBraveKeyMasked = $state('');         // mask_secret(key), only meaningful when wsBraveKeySaved
  let wsBraveKeyInput = $state('');          // the field's live value while typing/saving
  let wsShowBraveKey = $state(false);
  let wsSearxngEndpoint = $state('');        // saved value from ai_settings.searxng_endpoint
  let wsSearxngEndpointInput = $state('');   // the field's live value while typing/saving
  let wsAdvancedOpen = $state(false);
  let wsSaving = $state(false);
  let wsTesting = $state(false);
  // Test result, cleared on any Save/Remove/Clear so a stale pass/fail never lingers
  // across a credential change.
  let wsTestResult = $state<{ ok: boolean; backend?: string; error?: string } | null>(null);

  // === AI BEHAVIOR (§4.5.2.1 / #1589) ===
  // fix(runtime): checkpoint default 10 → 30 (#3050 PR 3 follow-up, §36.2.2
  // Ruling 4, Matt 2026-10-27) — this is the pre-load placeholder only;
  // overwritten by the real stored/fallback value in the settings-load
  // effect below.
  let abMaxIter = $state(30);
  let abMaxIterSaved = $state<number | null>(null);
  let abLoopTimeout = $state(120);
  let abLoopTimeoutSaved = $state<number | null>(null);
  let abMaxIterSavedVisible = $state(false);
  let abLoopTimeoutSavedVisible = $state(false);
  // §36.2.2 (#3050 PR 3, Ruling 4): the overall job ceiling, shown beside
  // Max tool iterations — same `handleAbBlur`/`get_ai_setting`/`set_ai_setting`
  // round-trip shape as the two rows above, extended with a third key.
  let abJobCeiling = $state(200);
  let abJobCeilingSaved = $state<number | null>(null);
  let abJobCeilingSavedVisible = $state(false);
  // §36.2.2 (#3050 PR 8): "Concurrent jobs" — UI-only, new in this PR.
  // The backend value (`ai_settings.job_concurrency`) already exists,
  // already read by `scheduler_tick` (`ai/scheduler.rs`,
  // `.unwrap_or(RUNTIME_DEFAULT_CONCURRENCY)`) — this row is the first
  // thing that writes it. Unlike the ceiling/checkpoint pair, nothing
  // clamps this against another field — a plain bounded numeric value
  // (min 1: a 0 would silently stall the entire scheduler, per Reed's
  // PR 8 scope audit §9's blast-radius table).
  let abJobConcurrency = $state(2);
  let abJobConcurrencySaved = $state<number | null>(null);
  let abJobConcurrencySavedVisible = $state(false);

  // Fact browser state
  let allMemories = $state<any[]>([]);
  let searchQuery = $state('');
  let categoryFilter = $state('all');
  let editingMemoryId = $state<number | null>(null);
  let editingMemoryText = $state('');
  let deleteConfirmId = $state<number | null>(null);
  let displayLimit = $state(50);
  
  // Derived: unique categories from loaded facts
  let uniqueCategories = $derived(
    Array.from(new Set(allMemories.map(m => m.category).filter(Boolean))).sort()
  );
  
  // Derived: filtered memories based on search/filter
  let filteredMemories = $derived(
    allMemories.filter(m => {
      // Text search
      if (searchQuery && !m.fact.toLowerCase().includes(searchQuery.toLowerCase())) {
        return false;
      }
      
      // Category filter
      if (categoryFilter !== 'all' && m.category !== categoryFilter) {
        return false;
      }
      
      return true;
    })
  );
  
  // Derived: displayed memories (with pagination)
  let displayedMemories = $derived(filteredMemories.slice(0, displayLimit));
  
  // === DAILY LOGS TAB ===
  let dailyLogs = $state<Array<{agent: string, date: string, file_path: string, size: number}>>([]);
  let logsLoaded = $state(false);
  let selectedLog = $state<{agent: string, date: string, file_path: string, size: number} | null>(null);
  let selectedLogContent = $state('');
  
  // === HEALTH TAB ===
  let budgetStats = $state<any>(null);
  let extractionStats = $state<any>(null);
  let compactionStats = $state<any>(null);
  let backupsList = $state<any[]>([]);
  let creatingBackup = $state(false);
  let restoringBackup = $state(false);
  let backupToRestore = $state<string | null>(null);
  // Context-window override UI state (#1404-E)
  let ctxWindowInput = $state('');
  let ctxWindowSaving = $state(false);
  let ctxWindowError = $state<string | null>(null);
  let ctxWindowCapped = $state<string | null>(null);
  // Fix #1422 Problem 1: track last committed value to deduplicate save fires.
  // onblur + onkeydown(Enter) both fire on Enter-commit; this guard ensures
  // exactly one set_ai_setting call per unique value committed.
  let ctxWindowLastSaved = $state('');
  
  // === ACCOUNT & SYNC TAB ===
  // ADR-0070 Decision 3 (#3195 PR B), red-first test T3: the Account
  // block must not render when `account_enabled` is off — reuse the one
  // reader `authStore.ts::isAccountEnabled()` already defines (PR A)
  // rather than re-deriving "what does off mean" here.
  let accountEnabled = $state(false);
  let email = $state('');
  let password = $state('');
  let deviceName = $state('');
  let syncInfo = $state<{ db_version: number; total_changes: number } | null>(null);
  let syncStatus = $state('Ready');
  let syncPeers = $state<string[]>([]);
  let syncing = $state(false);
  let manualPeer = $state('');
  let showManualPeer = $state(false);
  let lastSyncTimeVal = $state<number | null>(null);
  let syncError = $state<string | null>(null);
  let showErrorTimeout: ReturnType<typeof setTimeout> | null = $state(null);
  let showSyncHistory = $state(false);
  let syncHistory = $state<Array<{timestamp: number; peer: string; changesSent: number; changesReceived: number; conflicts: number; error?: string}>>([]);
  let isRetrying = $state(false);
  let resetPasswordSending = $state(false);
  let resetPasswordSuccess = $state('');
  let showChangePassword = $state(false);
  let newPassword = $state('');
  
  let lastSyncPush = $state<string | null>(null);
  let confirmPassword = $state('');
  let changingPassword = $state(false);
  let changePasswordError = $state('');
  let changePasswordSuccess = $state('');
  
  // === ABOUT TAB ===
  let appVersion = $state('Unknown');
  
  // Load initial data
  $effect(() => {
    (async () => {
      try {
        appVersion = await getVersion();
      } catch (error) {
        console.error('[Settings] Failed to read app version:', error);
      }

      // Load vault path
      try {
        vaultPath = await invoke('get_vault_path_setting') as string;
      } catch {}
      
      // Load editor settings
      if (typeof localStorage !== 'undefined') {
        editorFontSize = localStorage.getItem('pane_editor_font_size') || '15.5';
        appZoom = parseFloat(localStorage.getItem('pane_zoom_level') || '1');
      }
      
      // Load categories
      await loadCategories();
      
      // Load AI config
      await loadAiConfig();
      
      // Load memory settings
      loadMemorySettings();
      await loadMemoryCount();
      await loadMemorySearchSettings();
      
      // Load sync preferences
      try {
        const savedPeer = localStorage.getItem('pane-sync-peer');
        if (savedPeer) manualPeer = savedPeer;
      } catch {}
      
      await refreshPeers();
      syncHistory = getSyncHistory();
      
      // Subscribe to sync status
      const unsubscribe = subscribeSyncStatus((status) => {
        if (status.isRetrying !== undefined) isRetrying = status.isRetrying;
        if (status.status && status.status !== syncStatus) {
          syncStatus = status.status;
          if (status.lastSyncTime) {
            lastSyncTimeVal = status.lastSyncTime;
            syncHistory = getSyncHistory();
            const minutes = Math.floor((Date.now() - lastSyncTimeVal) / (1000 * 60));
            if (minutes < 60) {
              setTimeout(() => updateSyncStatus(), 60000);
            }
          }
        }
      });
      
      // Get device name
      try {
        const hostname = await invoke('get_hostname') as string;
        deviceName = hostname || 'Mac Studio';
      } catch {
        deviceName = 'Mac Studio';
      }
      
      // ADR-0070 Decision 3 (#3195 PR B): read account_enabled so the
      // Account block (below) doesn't render a sign-in form that would
      // always fail — every command it could call is guarded off.
      accountEnabled = await isAccountEnabled();

      // Load sync info
      if ($isAuthenticated) {
        await loadSyncInfo();
      }
      
      // Load last sync push timestamp
      try {
        const syncResult = await invoke('get_ai_setting', { key: 'last_sync_push' }) as string | null;
        if (syncResult) {
          try { lastSyncPush = JSON.parse(syncResult); } catch { lastSyncPush = syncResult; }
        }
      } catch {}
      
      // Periodic peer discovery
      const peerInterval = setInterval(() => refreshPeers(), 30000);

      // Listen for embeddings://status events (ADR-0048 / §5.9 / #1657)
      // Single unified channel; emitted by lib.rs startup pipeline and
      // switch_embedding_engine command.
      const unlistenEmbed = listen('embeddings://status', async (event: any) => {
        ssStatus = event.payload;
        // Clear the pending-switch warning once indexing completes
        if (event.payload?.stage === 'ready' || event.payload?.stage === 'partial' || event.payload?.stage === 'error') {
          ssShowWarn = false;
        }
        // On completion, fetch live index counts from the DB (authoritative source)
        if (event.payload?.stage === 'ready' || event.payload?.stage === 'partial') {
          try {
            const counts = await invoke('get_embeddings_counts') as { notes: number; facts: number; chats: number; failed: number };
            ssStatus = { stage: ssStatus?.stage ?? 'ready', ...ssStatus, notes: counts.notes, facts: counts.facts, chats: counts.chats, failed: counts.failed };
          } catch (e) {
            console.warn('Failed to refresh embedding counts after pipeline completion:', e);
          }
        }
      });
      
      // Cleanup
      return () => {
        unsubscribe?.();
        clearInterval(peerInterval);
        if (showErrorTimeout) clearTimeout(showErrorTimeout);
        unlistenEmbed.then((fn: () => void) => fn());
      };
    })();
  });
  
  // Load agents when switching to agents tab
  $effect(() => {
    if (activeTab === 'agents' && !agentsLoaded && !loadingAgents) {
      loadAgents();
    }
  });

  // #2522 design polish batch, item 3: the Default agent row on the AI tab names the
  // current default ("Brit Freed · OpenClaw") instead of only offering "Change
  // default" with no indication of what's currently selected. Reuses the same
  // agentsList/loadAgents() the Agents tab already populates — loaded here too since
  // the AI tab can be opened first, before the Agents tab has ever loaded anything.
  $effect(() => {
    if (activeTab === 'ai' && !agentsLoaded && !loadingAgents) {
      loadAgents();
    }
  });
  
  // Load logs when switching to logs tab
  $effect(() => {
    if (activeTab === 'logs' && !logsLoaded) {
      loadDailyLogs();
    }
  });

  // §28.1 Fix 6: Auto-probe Ollama when the AI tab opens so the user sees real status
  // instead of 'not tested' forever. Only fires when internal provider is 'ollama'.
  $effect(() => {
    if (activeTab === 'ai' && internalProvider === 'ollama' && internalConnectionStatus === 'not-tested') {
      probeInternalOllama();
    }
  });
  
  // Sync model selection when availableModels loads or model changes
  // OC is excluded: no model picker is rendered for OC; model comes from fetch_oc_agent_model
  // (§27.6.1c) and X-OC-Resolved-Model header (§27.6.1d), not from the catalog. (#882, #935)
  $effect(() => {
    if ((provider === 'anthropic' || provider === 'openrouter') && model && availableModels.length > 0) {
      const modelExists = availableModels.some(m => m.id === model);
      if (modelExists) {
        selectedModelOption = model;
        customModelName = '';
      } else {
        selectedModelOption = '__custom__';
        customModelName = model;
      }
    }
  });
  
  // Sync model from selection dropdown
  // OC is excluded: picker is not rendered for OC; writeback loop is dead work. (#882, #935)
  $effect(() => {
    if (provider === 'anthropic' || provider === 'openrouter') {
      if (selectedModelOption !== '__custom__') {
        model = selectedModelOption;
      } else {
        model = customModelName;
      }
    }
  });
  
  // Sync simple tier model from selection dropdown
  $effect(() => {
    if (selectedSimpleTierModelOption !== '__custom__') {
      simpleTierModel = selectedSimpleTierModelOption;
    } else {
      simpleTierModel = customSimpleTierModelName;
    }
  });
  
  // === GENERAL FUNCTIONS ===
  async function selectPaneRoot() {
    try {
      const folder = await open({ directory: true, title: 'Select Pane vault location' });
      if (!folder) return;
      vaultPath = folder as string;
    } catch (e: any) {
      vaultPathError = e.toString();
    }
  }
  
  async function saveVaultPath() {
    try {
      vaultPathError = '';
      vaultPathSaved = false;
      const result = await invoke('set_vault_path_setting', { path: vaultPath }) as string;
      vaultPathSaved = true;
      vaultPathError = '';
      importResult = result;
    } catch (e: any) {
      vaultPathError = e.toString();
      vaultPathSaved = false;
    }
  }
  
  // #2972 stage 4 (§34 inventory): Settings' CATEGORIES tab refreshes
  // itself after its OWN add/rename/delete calls (each already calls
  // loadCategories() below), but had no listener for a category changed
  // ELSEWHERE (e.g. an agent tool call) while this screen was open — not
  // one of §34.12's nine screens, never migrated by any prior #2972
  // stage. Same "tiny, no per-item read, deliberate" full-list reread
  // Sidebar/ProjectsView already use for their own category branch.
  $effect(() => {
    const unlistenDataChanged = subscribeDataChanged((payload) => {
      if (payload.entity_type === ENTITY_CATEGORY) {
        loadCategories();
      }
    });
    return () => {
      unlistenDataChanged.then((fn) => fn());
    };
  });

  async function loadCategories() {
    try { definedCategories = await api.listCategories(); } catch { definedCategories = []; }
  }
  
  async function addCategory() {
    if (!newCategoryName.trim()) return;
    try {
      // #3018 (§34.1): a brand-new category is `reason: "created"` — the
      // color-only-update branch in updateCategory() below keeps the
      // default "edited".
      await api.setCategoryColor(newCategoryName.trim(), newCategoryColor, REASON_CREATED);
      newCategoryName = '';
      newCategoryColor = '#00E4E4';
      await loadCategories();
    } catch (e) { console.error('Failed to add category:', e); }
  }
  
  async function updateCategory(oldName: string, newName: string, color: string) {
    try {
      if (oldName !== newName) {
        // A3 fix (#2141): rename must propagate atomically to all projects.category references.
        await api.renameCategory(oldName, newName, color);
      } else {
        // Color-only update — no rename needed.
        await api.setCategoryColor(newName, color);
      }
      editingCategoryIdx = null;
      await loadCategories();
    } catch (e) { console.error('Failed to update category:', e); }
  }
  
  async function removeCategoryDef(name: string) {
    try {
      await api.deleteCategory(name);
      await loadCategories();
    } catch (e) { console.error('Failed to delete category:', e); }
  }
  
  async function selectImportFolder() {
    try {
      const folder = await open({ directory: true, title: 'Select folder to import' });
      if (!folder) return;
      folderImportPath = folder as string;
      folderScanResult = await invoke('scan_import_folder', { path: folderImportPath });
      expandedFolders = new Set([folderScanResult.path]);
      showFolderPreview = true;
    } catch (e: any) {
      importResult = 'Error scanning folder: ' + e;
    }
  }
  
  async function confirmFolderImport() {
    isFolderImporting = true;
    showFolderPreview = false;
    try {
      const result: any = await invoke('import_folder', { path: folderImportPath });
      importResult = `Imported ${result.imported} notes into ${result.projects_created} new projects. ${result.skipped} skipped. ${result.folders_created} folders created.`;
      showImportToast(`Imported ${result.imported} notes into ${result.projects_created} projects`);
    } catch (e: any) {
      importResult = 'Error importing folder: ' + e;
    } finally {
      isFolderImporting = false;
      folderScanResult = null;
    }
  }
  
  function showImportToast(msg: string) {
    importToast = msg;
    if (importToastTimeout) clearTimeout(importToastTimeout);
    importToastTimeout = setTimeout(() => { importToast = ""; }, 4000);
  }
  
  async function exportData() {
    try {
      const folder = await open({ directory: true, title: 'Select export destination' });
      if (!folder) return;
      await invoke('export_data', { path: folder as string });
      importResult = 'Data exported successfully';
    } catch (e: any) {
      importResult = 'Export failed: ' + e;
    }
  }
  
  // === AI FUNCTIONS ===
  async function loadAiConfig() {
    try {
      const config = await api.aiLoadConfig();
      provider = config.provider || 'none';
      apiKey = config.api_key || '';
      endpoint = config.endpoint || 'http://localhost:11434';
      
      // Store model per provider
      if (config.model && provider !== 'none') {
        providerModels[provider] = config.model;
      }
      
      // Set current model from provider-specific storage or default
      model = providerModels[provider] || defaultModels[provider] || '';
      
      // Initialize saved provider and status (assume connected if config exists)
      if (provider !== 'none') {
        savedProvider = provider;
        savedConnectionStatus = 'connected';
      }
      
      // Interim state: set selectedModelOption to the saved model for all providers.
      // For anthropic/openrouter, this is overridden after loadAvailableModels() resolves
      // below — let the live catalog fully own model display state (no hardcoded lists).
      // showCustomModelInput stays false (default); set_ai_setting intercept is the only
      // write path and the live-catalog block is the only display-state owner.
      selectedModelOption = model;
      
      if (provider === 'openclaw') {
        openclawUrl = config.endpoint || 'http://127.0.0.1:18789';
        openclawToken = config.api_key || '';
        savedOpenclawUrl = openclawUrl;
        savedOpenclawToken = openclawToken;
        

        
        // Load stored TLS fingerprint for SplitMachine setups
        if (isRemoteUrl(openclawUrl)) {
          try {
            const storedFp = await invoke('get_ai_setting', { key: 'gateway_tls_fingerprint' }) as string | null;
            if (storedFp) {
              pairingFingerprint = storedFp;
            }
          } catch (e) {
            console.warn('Failed to load stored fingerprint:', e);
          }
        }
      }
      
      // Load simple_tier_model from config
      try {
        const stmResult = await invoke('get_ai_setting', { key: 'simple_tier_model' }) as string | null;
        if (stmResult) {
          simpleTierModel = stmResult;
        }
      } catch (e) {
        console.warn('Failed to load simple_tier_model:', e);
      }
      
      // Load extraction_model from config
      try {
        const emResult = await invoke('get_ai_setting', { key: 'extraction_model' }) as string | null;
        if (emResult !== null) extractionModel = emResult;
      } catch (e) {
        console.warn('Failed to load extraction_model:', e);
      }

      // Load rerank_model from config
      try {
        const rerankResult = await invoke('get_ai_setting', { key: 'memory_search_llm_rerank_model' }) as string | null;
        if (rerankResult) {
          searchLlmRerankModel = rerankResult;
        }
      } catch (e) {
        console.warn('Failed to load memory_search_llm_rerank_model:', e);
      }
      
      // Load available models for Anthropic and OpenRouter only.
      // OC is excluded: model comes from fetch_oc_agent_model (§27.6.1c) and
      // X-OC-Resolved-Model header (§27.6.1d), not the /v1/models catalog. (#882, #935)
      if (provider === 'anthropic' || provider === 'openrouter') {
        await loadAvailableModels();
        
        // Set model selection state
        if (model) {
          const modelExists = availableModels.some(m => m.id === model);
          if (modelExists) {
            selectedModelOption = model;
            customModelName = '';
          } else {
            selectedModelOption = '__custom__';
            customModelName = model;
          }
        } else {
          selectedModelOption = '';
          customModelName = '';
        }
        
        // Set simple tier model selection state
        if (simpleTierModel) {
          const modelExists = availableModels.some(m => m.id === simpleTierModel);
          if (modelExists) {
            selectedSimpleTierModelOption = simpleTierModel;
            customSimpleTierModelName = '';
          } else {
            selectedSimpleTierModelOption = '__custom__';
            customSimpleTierModelName = simpleTierModel;
          }
        } else {
          selectedSimpleTierModelOption = '';
          customSimpleTierModelName = '';
        }
      }
      if (provider === 'ollama') {
        await checkOllama();
      }
      
      // §29: Load internal provider settings — no fallback to primary provider per §27.6.2 rule 3
      // If internal_provider is null, dropdown stays empty (truthful UI state)
      try {
        let intProvResult = await invoke('get_ai_setting', { key: 'internal_provider' }) as string | null;
        console.log('[Settings] Loaded internal provider from DB:', intProvResult);
        if (intProvResult) {
          internalProvider = intProvResult;
        }

        // C50 (#3171 PR 1): no key field to read back — openrouter/anthropic/
        // venice each resolve their key from the matching Provider Connection
        // at call time on the backend (`internalProviderOptions`, derived from
        // `settingsConnections`, drives the dropdown's filter instead).

        let intEndpointResult = await invoke('get_ai_setting', { key: 'internal_endpoint' }) as string | null;
        if (intEndpointResult) {
          internalEndpoint = intEndpointResult;
        }

        // #2641 D14 Commit 2: read back the "configured this install" marker for the
        // Smart Features status dot (Commit 5). Same key written by saveInternalProvider().
        let intConfiguredAtResult = await invoke('get_ai_setting', { key: 'internal_configured_at' }) as string | null;
        internalConfiguredAt = intConfiguredAtResult ?? null;
        
        console.log('[Settings] Internal provider state after load:', internalProvider);
        
        // §28.1 INVARIANT: On-load, Ollama stays 'not-tested' — success requires a real probe.
        // #2641 D14 Commit 5: the previous auto-"success" for non-Ollama providers with a
        // saved key (credential presence ≠ configured this install — the same #2631 false-
        // green shape as the provider cards) is REMOVED. Fresh load stays 'not-tested'
        // (neutral) regardless of provider until an explicit Test/Save — the status DOT
        // (Commit 5, rendered in the template) is what shows grey/green/red now, driven by
        // internalConfiguredAt + internalConnectionStatus, not this on-load branch.
        internalConnectionStatus = 'not-tested';
        internalConnectionMessage = 'Not tested';
        // C50 (#3171 PR 1): openrouter/anthropic/venice no longer gate on a
        // locally-read key — Provider Connections already holds it. Ollama
        // still requires its own prior successful probe (internalConfiguredAt
        // is only ever written after a passing Save & Test, same invariant as
        // before).
        if (
          internalProvider === 'openrouter' || internalProvider === 'anthropic' || internalProvider === 'venice' ||
          (internalProvider === 'ollama' && internalConfiguredAt)
        ) {
          await loadInternalProviderModels(); // populate pickers from last-known-good saved state
        }
      } catch (e) {
        console.warn('Failed to load internal provider settings:', e);
      }

      // Load Semantic Search settings (ADR-0048 / §5.9 / #1657)
      // Durable state is read here for mount-time reconciliation (spec §5 persistence).
      // Live transitions come from the embeddings://status event listener above.
      try {
        const ssEnabledResult = await invoke('get_ai_setting', { key: 'embeddings_enabled' }) as string | null;
        ssEnabled = ssEnabledResult === 'true';

        const ssEngineResult = await invoke('get_ai_setting', { key: 'embeddings_engine' }) as string | null;
        if (ssEngineResult) ssProvider = ssEngineResult;

        const ssModelResult = await invoke('get_ai_setting', { key: 'embeddings_model' }) as string | null;
        if (ssModelResult) ssModel = ssModelResult;

        // Read durable terminal status for mount-time reconciliation
        const ssStatusResult = await invoke('get_ai_setting', { key: 'embeddings_status' }) as string | null;
        if (ssStatusResult) {
          ssStatus = { stage: ssStatusResult };
          // Populate live index counts from the embeddings table (never stale across sessions)
          if (ssStatusResult === 'ready' || ssStatusResult === 'partial') {
            try {
              const counts = await invoke('get_embeddings_counts') as { notes: number; facts: number; chats: number; failed: number };
              ssStatus = { ...ssStatus, stage: ssStatus?.stage ?? ssStatusResult, notes: counts.notes, facts: counts.facts, chats: counts.chats, failed: counts.failed };
            } catch (e) {
              console.warn('Failed to load embedding counts:', e);
            }
          }
        }

        // C50 (#3171 PR 1): no key field to read back — openrouter reads
        // Provider Connections' own keychain slot on the backend
        // (`ssProviderOptions`, derived from `settingsConnections`, drives the
        // dropdown's filter) and venice already reused the chat-provider key.
        // `embeddings_api_key` is retired entirely (was plaintext — the
        // credential-handling defect C50 fixed).
        const savedEmbedEndpoint = await invoke('get_ai_setting', { key: 'embeddings_endpoint' }) as string | null;
        if (savedEmbedEndpoint) {
          ssEndpoint = savedEmbedEndpoint;
        }
        // Endpoint fallback: if no card endpoint saved yet, fall back to primary provider endpoint.
        if (!savedEmbedEndpoint && ssEndpoint === 'http://localhost:11434' && endpoint && endpoint !== 'http://localhost:11434') {
          ssEndpoint = endpoint;
        }

        // Populate model list if a provider is already configured
        if (ssEnabled && ssProvider) {
          await loadEmbeddingModels();
        }
      } catch (e) {
        console.warn('Failed to load semantic search settings:', e);
      }

      // Load link previews gate (ADR-0049 Decision 2, #1780 — replaces link_embeds_enabled)
      try {
        const lpResult = await invoke('get_ai_setting', { key: 'link_previews_enabled' }) as string | null;
        linkPreviewsEnabled = lpResult !== 'false'; // null → true, 'true' → true, 'false' → false
      } catch (e) {
        console.warn('Failed to load link_previews_enabled:', e);
      }

      // Load Web Search settings (§4.6.8, Part 2, #2591). State derives from key/endpoint
      // presence, not a configured_at marker — "There is no configured_at marker for this
      // card; state derives from key and endpoint presence" (Matt, 2026-08-27).
      try {
        // ai_brave_key is keychain-only (§4.6.8 "Key storage": never written to ai_settings
        // or config.json), so presence + masked display come from a dedicated command
        // rather than get_ai_setting, which only reads the ai_settings table.
        const maskedResult = await invoke('brave_key_status') as string | null;
        wsBraveKeySaved = !!maskedResult;
        wsBraveKeyMasked = maskedResult || '';

        const searxngResult = await invoke('get_ai_setting', { key: 'searxng_endpoint' }) as string | null;
        wsSearxngEndpoint = searxngResult || '';
        wsSearxngEndpointInput = wsSearxngEndpoint;
        if (wsSearxngEndpoint) {
          wsAdvancedOpen = true;
        }
      } catch (e) {
        console.warn('Failed to load web search settings:', e);
      }

      // Load Image generation settings (§39.2, #3161 PR 1). Both keys are
      // null until the user picks (no default chosen on their behalf) —
      // reading absence as '' here is display-only, never written back as
      // a choice. The connections roster drives the provider dropdown's
      // filter; the model list is fetched lazily below, only when a
      // provider was already picked (mirrors `handleSsProviderChange` ->
      // `loadEmbeddingModels()` — no fetch on mount for the model list).
      try {
        const igProviderResult = await invoke('get_ai_setting', { key: 'image_provider' }) as string | null;
        if (igProviderResult) igProvider = igProviderResult;
        const igModelResult = await invoke('get_ai_setting', { key: 'image_model' }) as string | null;
        if (igModelResult) igModel = igModelResult;

        settingsConnections = await listRuntimeConnections();

        if (igProvider) {
          await loadImageModels();
        }

        // §39.4 / #3161 PR 2: nullable, default-4-when-absent — a STORED
        // value always wins; 4 is only the seed/fallback for a key that
        // was never saved (same shape as `abMaxIter` below).
        const igCapResult = await invoke('get_ai_setting', { key: 'max_images_per_turn' }) as string | null;
        igMaxImagesPerTurn = igCapResult ? parseInt(igCapResult, 10) : 4;
      } catch (e) {
        console.warn('Failed to load image generation settings:', e);
      }

      // Load AI Behavior settings (§4.5.2.1 / #1589)
      try {
        const maxIterResult = await invoke('get_ai_setting', { key: 'max_tool_iterations' }) as string | null;
        // fix(runtime): checkpoint default 10 → 30 (#3050 PR 3 follow-up) —
        // a STORED value (maxIterResult non-null) always wins; 30 is only
        // the seed/fallback for a key that was never saved.
        abMaxIter = maxIterResult ? parseInt(maxIterResult, 10) : 30;
        abMaxIterSaved = abMaxIter;

        const timeoutResult = await invoke('get_ai_setting', { key: 'loop_timeout_seconds' }) as string | null;
        abLoopTimeout = timeoutResult ? parseInt(timeoutResult, 10) : 120;
        abLoopTimeoutSaved = abLoopTimeout;

        // §36.2.2 (#3050 PR 3): same read-time-fallback shape, default 200
        // (RUNTIME_DEFAULT_JOB_CEILING, runtime_constants.rs).
        const jobCeilingResult = await invoke('get_ai_setting', { key: 'job_ceiling_tool_calls' }) as string | null;
        abJobCeiling = jobCeilingResult ? parseInt(jobCeilingResult, 10) : 200;
        abJobCeilingSaved = abJobCeiling;

        // §36.2.2 (#3050 PR 8): same read-time-fallback shape, default 2
        // (RUNTIME_DEFAULT_CONCURRENCY, runtime_constants.rs) — this key
        // was never read by the UI before this PR.
        const jobConcurrencyResult = await invoke('get_ai_setting', { key: 'job_concurrency' }) as string | null;
        abJobConcurrency = jobConcurrencyResult ? parseInt(jobConcurrencyResult, 10) : 2;
        abJobConcurrencySaved = abJobConcurrency;
      } catch (e) {
        console.warn('Failed to load AI behavior settings:', e);
      }
    } catch (error) {
      console.error('Failed to load AI config:', error);
    }
  }
  
  async function checkOllama() {
    try {
      const result = await invoke('ollama_probe_two_step', { endpoint }) as {
        models: string[];
        error_class: string | null;
      };
      if (result.error_class) {
        throw new Error(`Ollama probe failed: ${result.error_class}`);
      }

      ollamaModels = result.models
        .map((name) => ({ id: name, display: name.replace(/:latest$/, '') }))
        .sort((a, b) => a.display.localeCompare(b.display));

      // Set model to first available if not set or not in list
      if (!model || !ollamaModels.some((m: { id: string; display: string }) => m.id === model)) {
        model = ollamaModels[0]?.id || '';
      }
    } catch (e) {
      console.error('Ollama check failed:', e);
      ollamaModels = [];
    }
  }
  
  async function loadAvailableModels() {
    try {
      const models = await invoke<{ id: string; name: string; provider: string }[]>('fetch_available_models');
      if (models && models.length > 0) {
        availableModels = models;
        console.log('[Settings] Loaded available models:', models.length);
        

      } else {
        console.error('No models returned from fetch_available_models');
        availableModels = [];
      }
    } catch (e) {
      console.error('Failed to load models:', e);
      availableModels = [];
    }
  }
  
  // §28.1: Probe internal Ollama — calls ollama_probe_two_step and branches on error_class
  async function probeInternalOllama(): Promise<boolean> {
    const endpoint = normalizeOllamaEndpoint(internalEndpoint || 'http://localhost:11434');
    internalOllamaErrorClass = null;
    try {
      const result: any = await invoke('ollama_probe_two_step', { endpoint });
      if (!result.error_class) {
        internalConnectionStatus = 'success';
        internalConnectionMessage = 'Connected to Ollama';
        internalOllamaErrorClass = null;
        await loadInternalProviderModels();
        return true;
      } else if (result.error_class === 'unreachable') {
        internalConnectionStatus = 'error';
        internalConnectionMessage = `Couldn't reach Ollama at ${endpoint}. Make sure it's running.`;
        internalOllamaErrorClass = 'unreachable';
      } else if (result.error_class === 'no_models') {
        internalConnectionStatus = 'error';
        internalConnectionMessage = "Ollama is running but no models are installed. Pull one (e.g. ollama pull llama3.2:3b), then retry.";
        internalOllamaErrorClass = 'no_models';
      } else if (result.error_class === 'tags_failed') {
        internalConnectionStatus = 'error';
        internalConnectionMessage = "Connected to Ollama but couldn't list models. Retry or check Ollama logs.";
        internalOllamaErrorClass = 'tags_failed';
      } else {
        internalConnectionStatus = 'error';
        internalConnectionMessage = result.error_message || 'Ollama probe failed.';
        internalOllamaErrorClass = result.error_class;
      }
    } catch (e: any) {
      internalConnectionStatus = 'error';
      internalConnectionMessage = e?.message || 'Ollama probe failed.';
      internalOllamaErrorClass = 'unreachable';
    }
    return false;
  }

  // §29: Load models from internal provider (for simple_tier_model and memory_search_llm_rerank_model)
  async function loadInternalProviderModels() {
    try {
      const models = await invoke<{ id: string; name: string; provider: string }[]>('fetch_internal_provider_models');
      if (models && models.length > 0) {
        internalProviderModels = models;
        console.log('[Settings] Loaded internal provider models:', models.length);
      } else {
        internalProviderModels = [];
      }
    } catch (e) {
      console.warn('[Settings] Failed to load internal provider models:', e);
      internalProviderModels = [];
    }
  }
  
  // ─── Semantic Search (ADR-0048 / §5.9 / #1657) ─────────────────────────────

  /// Fetch available embedding models from the configured provider.
  /// Called when the provider dropdown changes or on initial load.
  async function loadEmbeddingModels() {
    if (!ssProvider) return;
    ssLoadingModels = true;
    try {
      const models = await invoke<{ id: string; name: string; provider: string; context_length?: number | null }[]>(
        'fetch_embedding_models',
        {
          engine: ssProvider,
          // C50 (#3171 PR 1): no apiKey param at all — openrouter reads
          // Provider Connections' own keychain slot on the backend and venice
          // already read the chat-provider key directly. Ollama is the only
          // arm that still needs anything here (its endpoint, not a key).
          endpoint: ssProvider === 'ollama' ? (ssEndpoint || null) : null
        }
      );
      ssModels = models || [];
    } catch (e) {
      console.warn('[SemanticSearch] Failed to load embedding models:', e);
      ssModels = [];
    } finally {
      ssLoadingModels = false;
    }
  }

  /// Toggle semantic search on/off and persist to DB.
  async function handleSsToggle() {
    ssEnabled = !ssEnabled;
    try {
      await invoke('set_ai_setting', { key: 'embeddings_enabled', value: ssEnabled ? 'true' : 'false' });
    } catch (e) {
      console.warn('[SemanticSearch] Failed to save embeddings_enabled:', e);
    }
    // Load models if turning on with a provider already set
    if (ssEnabled && ssProvider && ssModels.length === 0) {
      await loadEmbeddingModels();
    }
  }

  /// Toggle link previews on/off and persist to DB (ADR-0049 Decision 2, #1780).
  async function handleLinkPreviewsToggle() {
    linkPreviewsEnabled = !linkPreviewsEnabled;
    try {
      await invoke('set_ai_setting', { key: 'link_previews_enabled', value: linkPreviewsEnabled ? 'true' : 'false' });
    } catch (e) {
      console.warn('[LinkPreviews] Failed to save link_previews_enabled:', e);
    }
  }

  // ─── Web Search (§4.6.8, Part 2, #2591) ───────────────────────────────────

  /// Save the Brave Search API key. Clears any stale Test result — a passing/failing
  /// test against the OLD key must not still be shown next to a just-changed key.
  async function handleSaveBraveKey() {
    if (!wsBraveKeyInput.trim()) return;
    wsSaving = true;
    wsTestResult = null;
    try {
      await invoke('save_brave_key', { key: wsBraveKeyInput.trim() });
      const maskedResult = await invoke('brave_key_status') as string | null;
      wsBraveKeySaved = !!maskedResult;
      wsBraveKeyMasked = maskedResult || '';
      wsBraveKeyInput = '';
      wsShowBraveKey = false;
    } catch (e) {
      console.warn('[WebSearch] Failed to save Brave key:', e);
    } finally {
      wsSaving = false;
    }
  }

  /// Remove the saved Brave Search API key.
  async function handleRemoveBraveKey() {
    wsSaving = true;
    wsTestResult = null;
    try {
      await invoke('remove_brave_key');
      wsBraveKeySaved = false;
      wsBraveKeyMasked = '';
    } catch (e) {
      console.warn('[WebSearch] Failed to remove Brave key:', e);
    } finally {
      wsSaving = false;
    }
  }

  /// Save the SearXNG endpoint (under "Advanced").
  async function handleSaveSearxngEndpoint() {
    if (!wsSearxngEndpointInput.trim()) return;
    wsSaving = true;
    wsTestResult = null;
    try {
      await invoke('save_searxng_endpoint', { endpoint: wsSearxngEndpointInput.trim() });
      wsSearxngEndpoint = wsSearxngEndpointInput.trim();
    } catch (e) {
      console.warn('[WebSearch] Failed to save SearXNG endpoint:', e);
    } finally {
      wsSaving = false;
    }
  }

  /// Clear the SearXNG endpoint (§4.6.8: "Clear sets searxng_endpoint to ''").
  async function handleClearSearxngEndpoint() {
    wsSaving = true;
    wsTestResult = null;
    try {
      await invoke('clear_searxng_endpoint');
      wsSearxngEndpoint = '';
      wsSearxngEndpointInput = '';
    } catch (e) {
      console.warn('[WebSearch] Failed to clear SearXNG endpoint:', e);
    } finally {
      wsSaving = false;
    }
  }

  /// Run a 1-result test query against whichever backend is currently configured.
  async function handleTestWebSearch() {
    wsTesting = true;
    wsTestResult = null;
    try {
      const backend = await invoke('test_web_search') as string;
      wsTestResult = { ok: true, backend };
    } catch (e: any) {
      wsTestResult = { ok: false, error: typeof e === 'string' ? e : (e?.message || String(e)) };
    } finally {
      wsTesting = false;
    }
  }

  // ─── Image generation (§39.2, #3161 PR 1) ──────────────────────────────

  /// Fetches the chosen provider's image-model list. Only called after a
  /// provider is explicitly selected (mount-time load above, or
  /// `handleIgProviderChange` below) — never speculatively on render,
  /// mirroring `loadEmbeddingModels`'s own fetch-on-pick discipline.
  async function loadImageModels() {
    if (!igProvider) return;
    igLoadingModels = true;
    igModelsError = '';
    try {
      if (igProvider === 'openrouter') {
        igModels = await invoke('fetch_openrouter_image_models_cmd') as { id: string; name: string; price_usd?: number | null }[];
      } else if (igProvider === 'venice') {
        igModels = await invoke('fetch_venice_image_models_cmd') as { id: string; name: string; price_usd?: number | null }[];
      } else {
        igModels = [];
      }
    } catch (e: any) {
      igModels = [];
      igModelsError = typeof e === 'string' ? e : (e?.message || String(e));
    } finally {
      igLoadingModels = false;
    }
  }

  async function handleIgProviderChange(newProvider: string) {
    igProvider = newProvider;
    igModel = '';
    igModels = [];
    igModelsError = '';
    await invoke('set_ai_setting', { key: 'image_provider', value: newProvider });
    if (newProvider) {
      await loadImageModels();
    }
  }

  async function handleIgModelSelect(modelId: string) {
    igModel = modelId;
    await invoke('set_ai_setting', { key: 'image_model', value: modelId });
  }

  /// §39.4 / #3161 PR 2: the per-turn cap control. Clamped to ≥1 client-
  /// side (a 0 cap would silently refuse every job-turn call with no
  /// indication why) — the backend's own parse-and-default
  /// (`max_images_per_turn_setting`) is the authoritative floor; this is
  /// just an honest input, not a second source of truth.
  async function handleIgMaxImagesPerTurnChange(value: string) {
    const parsed = parseInt(value, 10);
    const clamped = Number.isFinite(parsed) && parsed >= 1 ? parsed : 1;
    igMaxImagesPerTurn = clamped;
    igMaxImagesPerTurnSaving = true;
    try {
      await invoke('set_ai_setting', { key: 'max_images_per_turn', value: String(clamped) });
    } finally {
      igMaxImagesPerTurnSaving = false;
    }
  }

  /// §39.2: "Test — generates one small image from a fixed prompt into
  /// Home/Assets and shows it inline, same pattern as Web search's Test."
  /// Reuses the real `generate_image` path end to end (`test_generate_
  /// image` on the backend calls the same `execute_generate_image`
  /// function the tool itself uses) — not a second, parallel code path.
  async function handleTestImageGen() {
    igTesting = true;
    igTestResult = null;
    try {
      const result = await invoke('test_generate_image') as { attachment_id: string };
      const meta = await api.getAttachment(result.attachment_id);
      const dataResult = await api.getAttachmentData(result.attachment_id);
      igTestResult = {
        ok: true,
        attachmentId: result.attachment_id,
        mimeType: meta.mime_type,
        dataUrl: `data:${meta.mime_type};base64,${dataResult.data}`,
      };
    } catch (e: any) {
      igTestResult = { ok: false, error: typeof e === 'string' ? e : (e?.message || String(e)) };
    } finally {
      igTesting = false;
    }
  }

  let igStatusLine = $derived.by(() => {
    if (igTestResult) {
      return igTestResult.ok ? 'Test succeeded — image saved to Home/Assets.' : `Test failed: ${igTestResult.error}`;
    }
    if (igProvider && igModel) {
      return `Using ${igProvider === 'openrouter' ? 'OpenRouter' : igProvider} — ${igModel}.`;
    }
    return 'No image model chosen — Settings → AI → Image generation.';
  });
  let igStatusOk = $derived(igTestResult ? igTestResult.ok : true);

  /// §4.6.8 Settings states table, in priority order: a just-run Test result wins over
  /// the steady-state line (it's the freshest signal); otherwise SearXNG overrides Brave
  /// (same precedence as the backend-selection wrapper itself); otherwise Brave-configured;
  /// otherwise not-configured (A5 ruling: design wording "Reading pages by URL still
  /// works", not spec's chat-side "web_fetch still works with direct URLs" — that text is
  /// unchanged in the tool's own §4.6.7 error surface).
  let wsStatusLine = $derived.by(() => {
    if (wsTestResult) {
      return wsTestResult.ok
        ? `Test succeeded (${wsTestResult.backend}).`
        : `Test failed: ${wsTestResult.error}`;
    }
    if (wsSearxngEndpoint) {
      return `Searching via SearXNG at ${wsSearxngEndpoint}.`;
    }
    if (wsBraveKeySaved) {
      return `Brave Search API key saved (${wsBraveKeyMasked}).`;
    }
    return 'Web search is off. Reading pages by URL still works.';
  });
  let wsStatusOk = $derived(wsTestResult ? wsTestResult.ok : true);
  let wsAnyConfigured = $derived(wsBraveKeySaved || !!wsSearxngEndpoint);

  /// #2988 PR1: human-readable label for a Semantic Search provider value —
  /// shared by the switch-confirm modal (previously hardcoded to only
  /// OpenRouter/Ollama, which would have rendered the literal string 'venice').
  function ssProviderLabel(provider: string): string {
    if (provider === 'openrouter') return 'OpenRouter';
    if (provider === 'venice') return 'Venice';
    return 'Ollama';
  }

  /// Handle provider dropdown change — show confirm modal if an index already exists.
  async function handleSsProviderChange(newProvider: string) {
    if (ssProvider && ssProvider !== newProvider && ssStatus?.stage === 'ready') {
      // An index exists under the current provider — confirm before clearing it
      ssPendingProvider = newProvider;
      ssShowSwitchModal = true;
    } else {
      ssProvider = newProvider;
      ssModel = '';
      ssModels = [];
      if (newProvider) await loadEmbeddingModels();
    }
  }

  /// Confirm the provider switch — clear index and switch.
  async function confirmSsProviderSwitch() {
    ssProvider = ssPendingProvider;
    ssPendingProvider = '';
    ssShowSwitchModal = false;
    ssShowWarn = true;
    ssModel = '';
    ssModels = [];
    if (ssProvider) await loadEmbeddingModels();
  }

  /// Cancel the provider switch — restore original provider.
  function cancelSsProviderSwitch() {
    ssPendingProvider = '';
    ssShowSwitchModal = false;
  }

  /// Save & Test — calls switch_embedding_engine which persists settings and
  /// runs the pipeline. Status events arrive via the embeddings://status listener.
  async function handleSsSaveTest() {
    if (!ssProvider) return;
    if (!ssModel) return;
    ssSaving = true;
    ssStatus = { stage: 'indexing' };
    try {
      // #2776: pass through the selected model's context_length (already fetched by
      // loadEmbeddingModels for OpenRouter; null/undefined for Ollama or custom model
      // names not in ssModels) so the Rust side can persist the real token limit
      // instead of falling back to its static seed table / conservative floor.
      const selectedModelInfo = ssModels.find(m => m.id === ssModel);
      const result = await invoke('switch_embedding_engine', {
        engine: ssProvider,
        model: ssModel,
        // C50 (#3171 PR 1): no apiKey param at all (see loadEmbeddingModels'
        // matching comment). Endpoint is still Ollama-only — it's not a secret.
        endpoint: ssProvider === 'ollama' ? (ssEndpoint || null) : null,
        maxTokens: selectedModelInfo?.context_length ?? null,
      });
      console.log('[SemanticSearch] switch_embedding_engine result:', result);
      // Terminal status is also written to DB and emitted as an event;
      // the listener above will update ssStatus automatically.
    } catch (e: any) {
      console.error('[SemanticSearch] switch_embedding_engine failed:', e);
      ssStatus = { stage: 'error', reason: e?.message || String(e) };
    } finally {
      ssSaving = false;
      ssShowWarn = false;
    }
  }

  // ─── AI Behavior (§4.5.2.1 / #1589) ──────────────────────────────────────────

  /// Handle blur (or stepper change) for an AI Behavior number input.
  /// Clamps to [min, max], reverts on empty/NaN, saves once on commit, shows "Saved".
  ///
  /// §36.2.2 (#3050 PR 3, Ruling 4): "Setting them equal reproduces today's
  /// stop-at-30 behaviour and is the documented escape hatch" — so
  /// `job_ceiling_tool_calls` is CLAMPED up to `max_tool_iterations` here
  /// (never rejected/blocked), same clamp-not-validate choice the backend
  /// makes at its own two read sites (`ai_chat_stream`,
  /// `prepare_job_dispatch`) via `.max(max_tool_iterations)`. Saving
  /// `max_tool_iterations` HIGHER than the current ceiling also raises the
  /// ceiling to match, for the same reason — a user raising the checkpoint
  /// without separately raising the ceiling must still get a working turn.
  async function handleAbBlur(
    key: 'max_tool_iterations' | 'loop_timeout_seconds' | 'job_ceiling_tool_calls' | 'job_concurrency',
    rawValue: number | string,
    min: number,
    max: number,
    defaultVal: number
  ) {
    let num = typeof rawValue === 'string' ? parseInt(rawValue, 10) : rawValue;
    if (isNaN(num) || rawValue === '') {
      // Revert to last-saved or spec default
      num = key === 'max_tool_iterations'
        ? (abMaxIterSaved ?? defaultVal)
        : key === 'loop_timeout_seconds'
        ? (abLoopTimeoutSaved ?? defaultVal)
        : key === 'job_concurrency'
        ? (abJobConcurrencySaved ?? defaultVal)
        : (abJobCeilingSaved ?? defaultVal);
    }
    // Clamp to spec range
    num = Math.max(min, Math.min(max, num));

    // §36.2.2 Ruling 4 — ceiling >= checkpoint, clamped both directions.
    // `job_concurrency` (#3050 PR 8) has NO cross-field clamp — a plain
    // bounded numeric value, unlike the ceiling/checkpoint pair (Reed's
    // PR 8 scope audit §7: "nothing in the spec or code describes a
    // cross-field clamp for job_concurrency").
    let ceilingToSave: number | null = null;
    if (key === 'job_ceiling_tool_calls' && num < abMaxIter) {
      num = abMaxIter;
    } else if (key === 'max_tool_iterations' && num > abJobCeiling) {
      ceilingToSave = num;
    }

    // Update reactive state
    if (key === 'max_tool_iterations') {
      abMaxIter = num;
    } else if (key === 'loop_timeout_seconds') {
      abLoopTimeout = num;
    } else if (key === 'job_concurrency') {
      abJobConcurrency = num;
    } else {
      abJobCeiling = num;
    }
    if (ceilingToSave !== null) {
      abJobCeiling = ceilingToSave;
    }
    // Save to DB
    try {
      await invoke('set_ai_setting', { key, value: String(num) });
      if (ceilingToSave !== null) {
        await invoke('set_ai_setting', { key: 'job_ceiling_tool_calls', value: String(ceilingToSave) });
      }
      // Update saved ref for future reverts
      if (key === 'max_tool_iterations') {
        abMaxIterSaved = num;
        abMaxIterSavedVisible = true;
        setTimeout(() => { abMaxIterSavedVisible = false; }, 2000);
        if (ceilingToSave !== null) {
          abJobCeilingSaved = ceilingToSave;
        }
      } else if (key === 'loop_timeout_seconds') {
        abLoopTimeoutSaved = num;
        abLoopTimeoutSavedVisible = true;
        setTimeout(() => { abLoopTimeoutSavedVisible = false; }, 2000);
      } else if (key === 'job_concurrency') {
        abJobConcurrencySaved = num;
        abJobConcurrencySavedVisible = true;
        setTimeout(() => { abJobConcurrencySavedVisible = false; }, 2000);
      } else {
        abJobCeilingSaved = num;
        abJobCeilingSavedVisible = true;
        setTimeout(() => { abJobCeilingSavedVisible = false; }, 2000);
      }
    } catch (e) {
      console.warn('[AIBehavior] Failed to save', key, ':', e);
    }
  }

  // ─────────────────────────────────────────────────────────────────────────────

  // §22C.11A: Helper to check if URL is remote
  function isRemoteUrl(url: string): boolean {
    try {
      const parsed = new URL(url);
      const hostname = parsed.hostname.toLowerCase();
      return hostname !== 'localhost' && hostname !== '127.0.0.1' && hostname !== '::1';
    } catch {
      return false;
    }
  }
  
  // §29: Internal Provider save and test functions.
  // C50 (#3171 PR 1): openrouter/anthropic/venice have no key field —
  // `internalProviderOptions` already filtered the dropdown to providers
  // CONNECTED in Provider Connections, so picking one here is itself the
  // "configure" step (the key was already validated over there). Only
  // Ollama still needs an explicit endpoint + probe, since it has no
  // Provider Connection credential at all.
  async function handleInternalProviderChange() {
    console.log('[handleInternalProviderChange] provider changed to:', internalProvider);
    internalConnectionStatus = 'not-tested';
    internalConnectionMessage = 'Not tested';
    internalProviderModels = [];
    // C1: Clear model selections when provider changes
    simpleTierModel = '';
    extractionModel = '';
    searchLlmRerankModel = '';
    internalConfiguredAt = null;
    try {
      await invoke('set_ai_setting', { key: 'internal_provider', value: internalProvider });
      await invoke('set_ai_setting', { key: 'internal_configured_at', value: '' });
      await invoke('set_ai_setting', { key: 'simple_tier_model', value: '' });
      await invoke('set_ai_setting', { key: 'extraction_model', value: '' });
      await invoke('set_ai_setting', { key: 'memory_search_llm_rerank_model', value: '' });
    } catch (e) {
      console.warn('[handleInternalProviderChange] Failed to persist provider change:', e);
    }

    if (internalProvider === 'openrouter' || internalProvider === 'anthropic' || internalProvider === 'venice') {
      // #2641 D14 Commit 2: mark "configured this install" immediately — the
      // connection's key was already validated in Provider Connections, so
      // there is no separate test-then-save step left to gate this on.
      const nowIso = new Date().toISOString();
      try {
        await invoke('set_ai_setting', { key: 'internal_configured_at', value: nowIso });
        internalConfiguredAt = nowIso;
      } catch (e) {
        console.warn('[handleInternalProviderChange] Failed to set internal_configured_at:', e);
      }
      internalConnectionStatus = 'success';
      const providerLabel = internalProvider === 'openrouter' ? 'OpenRouter' : internalProvider === 'anthropic' ? 'Anthropic' : 'Venice';
      internalConnectionMessage = 'Connected to ' + providerLabel;
      await loadInternalProviderModels();
    }
    // Ollama: no auto-configure — user must set the endpoint and click Test.
  }

  /// Ollama-only now (C50 / #3171 PR 1) — openrouter/anthropic/venice are
  /// configured the instant they're picked (`handleInternalProviderChange`),
  /// since Provider Connections already validated their key. Kept under its
  /// original name: existing tests and the markup's "Save & Test Connection"
  /// button both reference `saveInternalProvider`.
  async function saveInternalProvider() {
    console.log('[saveInternalProvider] called with provider:', internalProvider);
    if (internalProvider !== 'ollama') {
      return;
    }
    loading = true;
    errorMessage = '';
    successMessage = '';

    try {
      testingInternalConnection = true;
      internalConnectionMessage = 'Testing...';
      const probeOk = await probeInternalOllama();
      testingInternalConnection = false;
      if (!probeOk) {
        // probeInternalOllama already set internalConnectionStatus + internalConnectionMessage.
        errorMessage = 'Connection test failed: ' + internalConnectionMessage;
        loading = false;
        return;
      }

      // SAVE ONLY IF TEST PASSED
      await invoke('set_ai_setting', { key: 'internal_provider', value: 'ollama' });
      await invoke('set_ai_setting', { key: 'internal_endpoint', value: internalEndpoint || 'http://localhost:11434' });
      // #2641 D14 Commit 2: mark "configured this install" ONLY here — the Save path,
      // after the connection test above has already passed. Same pattern as
      // {runtime}_configured_at for the provider cards (Commit 1).
      const nowIso = new Date().toISOString();
      await invoke('set_ai_setting', { key: 'internal_configured_at', value: nowIso });
      internalConfiguredAt = nowIso;

      // Load models after successful save
      await loadInternalProviderModels();
      successMessage = 'Ollama connected successfully';
      setTimeout(() => { successMessage = ''; }, 3000);
    } catch (error: any) {
      console.log('[saveInternalProvider] error:', error.toString());
      errorMessage = error.toString();
    } finally {
      loading = false;
    }
  }
  
  
  // === AGENTS FUNCTIONS ===
  let loadingAgents = $state(false);
  
  async function loadAgents() {
    loadingAgents = true;
    try {
      const [agents, connections] = await Promise.all([getAgents(), listRuntimeConnections()]);
      agentsList = agents;
      // #2522 Stage 6a: drives the provider-disconnected dim state on the regrouped Agents tab.
      agentsConnectedRuntimes = new Set(connections.filter(c => c.connected).map(c => c.runtime_type));
      console.log('Loaded agents from ai_agents table:', agents);
    } catch (err) {
      console.error('Failed to load agents:', err);
      agentsList = [];
    } finally {
      loadingAgents = false;
      agentsLoaded = true;
    }
  }

  // #2522 Stage 5d: open the cross-provider Default agent picker (DefaultAgentPickerModal).
  // Replaces the old per-agent "Set as Main" buttons (§24.7A) — those offered the same
  // choice a second time in the roster, which README-group-b.md's ruling forbids.
  async function openDefaultAgentPicker() {
    defaultPickerLoading = true;
    try {
      const [agents, connections] = await Promise.all([getAgents(), listRuntimeConnections()]);
      agentsList = agents;
      defaultPickerConnectedRuntimes = new Set(connections.filter(c => c.connected).map(c => c.runtime_type));
      showDefaultAgentPicker = true;
    } catch (err) {
      console.error('Failed to load data for Default agent picker:', err);
      alert(`Failed to open Default agent picker: ${err}`);
    } finally {
      defaultPickerLoading = false;
    }
  }

  // #2030 PR B: Open create overlay from "+ New agent" button
  function openCreateFlow() {
    showAgentCreate = true;
    agentCreateForced = false;
    agentCreatePendingProvider = '';
  }

  // #2030 PR B Fix B: complete deferred ai_configure_and_save after picker/create confirms
  async function completeDeferredProviderSave() {
    if (!pendingProviderSave) return;
    const p = pendingProviderSave;
    pendingProviderSave = null;
    loading = true;
    errorMessage = '';
    try {
      await invoke('ai_configure_and_save', {
        provider: p.provider,
        apiKey: p.apiKey,
        model: p.model,
        endpoint: p.endpoint,
        simpleTierModel: p.simpleTierModel,
      });
      savedProvider = p.provider;
      savedConnectionStatus = 'connected';
      // Bug 6: refresh model list after the new provider is committed so the AI tab
      // dropdown populates immediately without requiring a Settings close/reopen.
      if (p.provider === 'anthropic' || p.provider === 'openrouter') {
        await loadAvailableModels();
      }
    } catch (err: any) {
      errorMessage = err.toString();
    } finally {
      loading = false;
    }
  }

  // #2030 PR B: Called after overlay successfully creates an agent
  async function handleAgentCreated(_agent: Agent) {
    await loadAgents();
    if (agentCreateForced && pendingProviderSave) {
      await completeDeferredProviderSave();
      agentCreateForced = false;
    }
  }
  
  function startEditAgentName(idx: number, currentName: string) {
    editingAgentIdx = idx;
    editingAgentName = currentName;
  }
  
  async function saveAgentName(agent: any) {
    if (!editingAgentName.trim() || editingAgentName === agent.name) {
      editingAgentIdx = null;
      return;
    }
    
    try {
      await invoke('update_agent', {
        request: {
          agent_id: agent.id,
          name: editingAgentName.trim()
        }
      });
      
      // Reload agents to get updated data
      await loadAgents();
      editingAgentIdx = null;
      
      // Notify chat panel to update name display
      window.dispatchEvent(new CustomEvent('pane-agent-renamed'));
    } catch (err) {
      console.error('Failed to rename agent:', err);
      alert(`Failed to rename agent: ${err}`);
      editingAgentIdx = null;
    }
  }

  // #3074: second inline pencil next to the model sublabel text, gated at the
  // render site by isAgentModelEditable(agent) (agentDisplay.ts) — only called
  // for agents where the picker is actually allowed to show.
  //
  // Loads models via the same fetch_available_models + provider-filter idiom
  // AgentCreateOverlay.svelte's loadModelsForRuntime() already uses. That
  // function is NOT exported (no `export`, plain `<script>` block, private to
  // its own component) so it cannot be imported here without extracting it
  // into a shared module — which would be scope creep beyond this PR's
  // ruling. Copying the exact idiom (same invoke call, same filter) rather
  // than re-deriving a different one.
  async function startEditAgentModel(idx: number, agent: Agent) {
    editingAgentModelIdx = idx;
    agentModelLoadError = '';
    agentModelOptions = [];
    if (!agent.runtime_type) return;
    loadingAgentModelOptions = true;
    try {
      const models = await invoke<
        Array<{
          id: string;
          name: string;
          provider: string;
          tier_label?: string;
          supports_tee_attestation?: boolean;
          supports_e2ee?: boolean;
        }>
      >('fetch_available_models');
      agentModelOptions = models.filter((m) => m.provider === agent.runtime_type);
      if (agentModelOptions.length === 0) {
        agentModelLoadError = `No models available for ${RUNTIME_TYPE_LABELS[agent.runtime_type] ?? agent.runtime_type}.`;
      }
    } catch (err: any) {
      console.error('[Settings] Failed to load models for agent', agent.id, err);
      agentModelLoadError = err?.message || String(err);
      agentModelOptions = [];
    } finally {
      loadingAgentModelOptions = false;
    }
  }

  function cancelEditAgentModel() {
    editingAgentModelIdx = null;
    agentModelOptions = [];
    agentModelLoadError = '';
  }

  // #3074: a dropdown can't "blur-save" the way the rename text input does —
  // there's no meaningful intermediate "typing" state to blur out of. The
  // equivalent single user action that completes the edit is picking an
  // option, so SearchableModelSelect's onSelect saves immediately (matches
  // this component's existing SearchableModelSelect call sites, e.g. Smart
  // Routing/Extraction model pickers above, which also save-on-select rather
  // than staging a separate commit step).
  //
  // Payload is hand-built with exactly { agent_id, model } — never a spread
  // of `agent` — because UpdateAgentRequest.runtime_type means "legacy-pin
  // this NULL row" (agents.rs:80-86, set_legacy_runtime_type_impl), not "the
  // current runtime". Including it here would misroute a normal model-edit
  // call into that one-time legacy-pin path. saveAgentName above gets this
  // same discipline right by hand-building its payload; this mirrors it.
  async function saveAgentModel(agent: Agent, modelId: string) {
    if (!modelId || modelId === agent.model) {
      editingAgentModelIdx = null;
      return;
    }
    try {
      await invoke('update_agent', {
        request: {
          agent_id: agent.id,
          model: modelId
        }
      });
      await loadAgents();
    } catch (err) {
      console.error('[Settings] Failed to update agent model:', err);
      alert(`Failed to update model: ${err}`);
    } finally {
      editingAgentModelIdx = null;
      agentModelOptions = [];
      agentModelLoadError = '';
    }
  }
  
  // === MEMORY FUNCTIONS ===
  function loadMemorySettings() {
    if (typeof localStorage === 'undefined') return;
    memoryEnabled = localStorage.getItem('pane_memory_enabled') !== 'false';
    synthesisEnabled = localStorage.getItem('pane_synthesis_enabled') !== 'false';
    dailyLogEnabled = localStorage.getItem('pane_daily_log_enabled') !== 'false';
    showMemoryToasts = localStorage.getItem('pane_memory_toasts') !== 'false'; // §5.4B (#641)
  }
  
  function saveMemorySettings() {
    if (typeof localStorage === 'undefined') return;
    localStorage.setItem('pane_memory_enabled', String(memoryEnabled));
    localStorage.setItem('pane_synthesis_enabled', String(synthesisEnabled));
    localStorage.setItem('pane_daily_log_enabled', String(dailyLogEnabled));
    localStorage.setItem('pane_memory_toasts', String(showMemoryToasts)); // §5.4B (#641)
  }
  
  async function loadMemoryCount() {
    try {
      const memories: any = await invoke('get_all_memories');
      allMemories = memories;
      memoryCount = memories.length;
    } catch (e) {
      allMemories = [];
      memoryCount = 0;
    }
  }
  
  async function loadMemorySearchSettings() {
    try {
      const fuseBoth = await invoke('get_ai_setting', { key: 'memory_search_fuse_both_sources' }) as string | null;
      if (fuseBoth !== null) searchFuseBothSources = fuseBoth === 'true';
      
      const keywordWeight = await invoke('get_ai_setting', { key: 'memory_search_keyword_weight' }) as string | null;
      if (keywordWeight !== null) searchKeywordWeight = parseFloat(keywordWeight);
      
      const temporalHalfLife = await invoke('get_ai_setting', { key: 'memory_search_temporal_half_life_days' }) as string | null;
      if (temporalHalfLife !== null) searchTemporalHalfLife = parseFloat(temporalHalfLife);
      
      const llmRerankEnabled = await invoke('get_ai_setting', { key: 'memory_search_llm_rerank_enabled' }) as string | null;
      if (llmRerankEnabled !== null) searchLlmRerankEnabled = llmRerankEnabled === 'true';
      
      const llmRerankModel = await invoke('get_ai_setting', { key: 'memory_search_llm_rerank_model' }) as string | null;
      if (llmRerankModel !== null) searchLlmRerankModel = llmRerankModel;

      try {
        searchAvailableModels = await invoke('list_available_models') as string[];
      } catch (e) {
        console.warn('Failed to fetch available models:', e);
        searchAvailableModels = [];
      }
      
      const factBoost = await invoke('get_ai_setting', { key: 'memory_search_fact_boost' }) as string | null;
      if (factBoost !== null) searchFactBoost = parseFloat(factBoost);
    } catch (e) {
      console.error('Failed to load memory search settings:', e);
    }
  }
  
  async function saveMemorySearchSetting(key: string, value: string) {
    try {
      await invoke('set_ai_setting', { key, value });
    } catch (e) {
      console.error(`Failed to save ${key}:`, e);
    }
  }
  
  /** #3151 PR3/C44: `timestamp` (memory `updated_at`/`created_at`) is
   *  SQLite's naive-UTC `datetime('now')` shape ("YYYY-MM-DD HH:MM:SS",
   *  no `T`/`Z`/offset) — handing it straight to `new Date()` parses it
   *  as LOCAL wall-clock time, not UTC, making this diff negative in
   *  any timezone behind UTC and pinning the result at "just now"
   *  forever. Same fix pattern already used elsewhere (`ChatPanel.svelte`,
   *  `scheduledPanel.ts`, `jobStatusChip.ts`, `utils/time.ts`): append
   *  `Z` (after swapping the space for `T`) so `Date` treats it as UTC.
   *  Kept as its own local function — its output (lowercase "just
   *  now"/"Xm ago", falling back to a bare "Mon D" for anything a week+
   *  old) differs from both `utils/time.ts`'s and `NotificationPanel
   *  .svelte`'s own vocabularies; unifying them would be a visible
   *  format change on another surface, out of scope for a parsing fix. */
  function formatRelativeTime(timestamp: string): string {
    // C44 (#3155): the parse itself (normalize + `new Date(...)`) is now
    // shared via `parseStoredTimestamp` (`$lib/utils/time.ts`) — this was
    // the last inline reimplementation of it. Output formatting below
    // (lowercase 'just now'/'Xm ago'/bare date fallback) is UNCHANGED
    // — this surface's vocabulary genuinely differs from both
    // `utils/time.ts`'s own sibling and `NotificationPanel.svelte`'s, so
    // only the parse is unified, per the original C44 ruling.
    const date = parseStoredTimestamp(timestamp);
    const now = new Date();
    const diffMs = now.getTime() - date.getTime();
    const diffMins = Math.floor(diffMs / (1000 * 60));
    const diffHours = Math.floor(diffMins / 60);
    const diffDays = Math.floor(diffHours / 24);
    
    if (diffMins < 1) return 'just now';
    if (diffMins < 60) return `${diffMins}m ago`;
    if (diffHours < 24) return `${diffHours}h ago`;
    if (diffDays < 7) return `${diffDays}d ago`;
    
    // Return formatted date for older items
    const month = date.toLocaleDateString('en-US', { month: 'short' });
    const day = date.getDate();
    return `${month} ${day}`;
  }
  
  function formatSyncPushTime(timestamp: string | null): string {
    if (!timestamp) return 'Never';
    const date = new Date(timestamp);
    const now = new Date();
    const diffMs = now.getTime() - date.getTime();
    const diffMin = Math.floor(diffMs / 60000);
    if (diffMin < 1) return 'Just now';
    if (diffMin < 60) return `${diffMin} minute${diffMin > 1 ? 's' : ''} ago`;
    const diffHr = Math.floor(diffMin / 60);
    if (diffHr < 24) return `${diffHr} hour${diffHr > 1 ? 's' : ''} ago`;
    return date.toLocaleDateString();
  }
  
  function startEditMemory(id: number, currentText: string) {
    editingMemoryId = id;
    editingMemoryText = currentText;
    deleteConfirmId = null;
  }
  
  function cancelEditMemory() {
    editingMemoryId = null;
    editingMemoryText = '';
  }
  
  async function saveEditMemory(id: number) {
    try {
      await invoke('update_memory', { memoryId: id, fact: editingMemoryText });
      // Update local list
      allMemories = allMemories.map(m => 
        m.id === id ? { ...m, fact: editingMemoryText } : m
      );
      editingMemoryId = null;
      editingMemoryText = '';
    } catch (e) {
      console.error('Failed to update memory:', e);
    }
  }
  
  function showDeleteConfirm(id: number) {
    deleteConfirmId = id;
    editingMemoryId = null;
  }
  
  function cancelDelete() {
    deleteConfirmId = null;
  }
  
  async function confirmDelete(id: number) {
    try {
      await invoke('delete_memory', { memoryId: id });
      // Remove from local list
      allMemories = allMemories.filter(m => m.id !== id);
      memoryCount = allMemories.length;
      deleteConfirmId = null;
    } catch (e) {
      console.error('Failed to delete memory:', e);
    }
  }
  
  function loadMoreMemories() {
    displayLimit += 50;
  }
  
  // === DAILY LOGS FUNCTIONS ===
  async function loadDailyLogs() {
    try {
      dailyLogs = await invoke('list_daily_logs') as any[];
    } catch (e) {
      console.error('Failed to load daily logs:', e);
      dailyLogs = [];
    } finally {
      logsLoaded = true;
    }
  }

  async function selectLog(log: typeof dailyLogs[0]) {
    selectedLog = log;
    try {
      selectedLogContent = await invoke('read_daily_log', { filePath: log.file_path });
    } catch (e) {
      console.error('Failed to read log:', e);
      selectedLogContent = 'Failed to load log content.';
    }
  }

  function formatLogDate(dateStr: string): string {
    // "2026-03-22" → "Mar 22, 2026"
    const d = new Date(dateStr + 'T12:00:00');
    return d.toLocaleDateString('en-US', { month: 'short', day: 'numeric', year: 'numeric' });
  }

  function formatSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    return `${(bytes / 1024).toFixed(1)} KB`;
  }
  
  // === HEALTH TAB FUNCTIONS ===

  /**
   * Fix #1422 Problem 3 — Split loadHealthStats into two call paths:
   *   rebindInput=true  → initial mount + Clear: refetch AND rebind ctxWindowInput to
   *                        the resolver's returned value (preserves #1404-E Q1=A pre-fill).
   *   rebindInput=false → post-save refresh: refetch stats so the budget row updates
   *                        (Used/Available, % used, tier bars) WITHOUT touching ctxWindowInput
   *                        while the user may still be typing.
   */
  async function refreshHealthStats(rebindInput: boolean) {
    try {
      budgetStats = await invoke('get_context_budget_stats');
      extractionStats = await invoke('get_extraction_stats');
      compactionStats = await invoke('get_compaction_stats');
      if (rebindInput && budgetStats) {
        // Pre-fill with the resolver's output (locked Q1=A design, #1404-E §27.11).
        ctxWindowInput = String(budgetStats.context_window);
        // WARN-1 (Reed audit): sync lastSaved with pre-fill so unedited blur doesn't fire spurious save (Settings.svelte:1466-1471, audit 2026-06-10-1427).
        ctxWindowLastSaved = ctxWindowInput;
        ctxWindowError = null;
        ctxWindowCapped = null;
      }
    } catch (e) {
      console.error('Failed to load health stats:', e);
    }
  }

  /** Initial-mount entry point — always rebinds input (pre-fill). */
  async function loadHealthStats() {
    await refreshHealthStats(true);
  }

  // Context-window override save handler (#1404-E, Q1=A: pre-fill with resolved value).
  async function saveContextWindowOverride() {
    // Stage 3 fix (#1422): sync re-entry guard MUST be the first line, before any other
    // statement. ctxWindowSaving is set synchronously below (before the first await), so
    // a second concurrent caller (e.g., onblur firing after onkeydown Enter on WebKit)
    // sees ctxWindowSaving=true and returns immediately without reaching the invoke.
    // Stage 2 dedupe (ctxWindowLastSaved) failed under concurrent re-entry because
    // lastSaved is only updated AFTER the first await resolves — both concurrent callers
    // see the OLD lastSaved and both proceed to invoke.
    if (ctxWindowSaving) return;

    ctxWindowError = null;
    ctxWindowCapped = null;

    // Fix #1422 Problem 2 / #1434: empty-string guard.
    // #1434: bind:value on <input type="number"> coerces ctxWindowInput to Number,
    // not String. Calling .trim() on a Number throws TypeError before the try block —
    // silent async rejection, no invoke fires, no log, override never persists.
    // Coerce to string at handler entry to defend against this. Only
    // clearContextWindowOverride() is the authorised path for empty writes.
    const inputStr = String(ctxWindowInput ?? '').trim();
    if (inputStr === '') return;

    const parsed = parseInt(inputStr, 10);
    if (isNaN(parsed) || parsed < 50_000) {
      ctxWindowError = 'Minimum 50,000 tokens';
      return;
    }

    // Fix #1422 Problem 1: deduplicate save fires (kept for separate-event-loop re-commit).
    // Not sufficient for concurrent re-entry (Stage 2 gap) but still useful: if the user
    // blurs twice seconds apart without editing, this avoids a redundant save.
    const valueToSave = String(parsed);
    if (valueToSave === ctxWindowLastSaved) return;

    ctxWindowSaving = true;
    try {
      await invoke('set_ai_setting', { key: 'context_window_override', value: valueToSave });
      ctxWindowLastSaved = valueToSave;
      // Fix #1422 Problem 3: post-save refresh updates the budget row WITHOUT rebinding
      // ctxWindowInput, so mid-typing snap-back cannot occur.
      await refreshHealthStats(false);
      // If the resolved value is smaller than what was typed, the backend clamped it.
      if (budgetStats && budgetStats.context_window < parsed) {
        ctxWindowCapped = `Capped at model max: ${budgetStats.context_window.toLocaleString()}`;
      }
    } catch (e) {
      console.error('Failed to save context window override:', e);
      ctxWindowError = 'Save failed';
    } finally {
      ctxWindowSaving = false;
    }
  }

  // Context-window override clear handler (#1404-E, Q3: explicit Clear button).
  // This is the ONLY path that writes an empty string to the DB.
  async function clearContextWindowOverride() {
    // Stage 3 fix (#1422): sync re-entry guard MUST be the first line.
    // Closes the SAVE '' × N burst: Svelte 5 batches DOM updates as microtasks, so the
    // disabled={ctxWindowSaving} attribute on the Clear button may lag behind the JS state.
    // Rapid clicks on Clear before DOM re-renders all pass the DOM-level check but are
    // correctly blocked here by the JS-state guard. Root cause of live SAVE '' × 4 burst.
    if (ctxWindowSaving) return;

    ctxWindowError = null;
    ctxWindowCapped = null;
    ctxWindowSaving = true;
    try {
      await invoke('set_ai_setting', { key: 'context_window_override', value: '' });
      // Reset dedup guard so the next typed value can be saved normally.
      ctxWindowLastSaved = '';
      // Rebind input to the resolver's default (pre-fill, Q1=A design preserved).
      await refreshHealthStats(true);
    } catch (e) {
      console.error('Failed to clear context window override:', e);
    } finally {
      ctxWindowSaving = false;
    }
  }

  async function loadBackups() {
    try {
      backupsList = await invoke('list_backups') as any[];
    } catch (e) {
      console.error('Failed to load backups:', e);
      backupsList = [];
    }
  }

  async function createBackupNow() {
    creatingBackup = true;
    try {
      const path = await invoke('create_backup') as string;
      console.log('Backup created:', path);
      await loadBackups();
    } catch (e) {
      console.error('Failed to create backup:', e);
      alert('Failed to create backup: ' + e);
    } finally {
      creatingBackup = false;
    }
  }

  async function confirmRestore(backupPath: string) {
    backupToRestore = backupPath;
  }

  async function doRestore() {
    if (!backupToRestore) return;
    restoringBackup = true;
    try {
      await invoke('restore_backup', { backupPath: backupToRestore });
      alert('Backup restored successfully. Please restart Pane.');
      backupToRestore = null;
    } catch (e) {
      console.error('Failed to restore backup:', e);
      alert('Failed to restore backup: ' + e);
    } finally {
      restoringBackup = false;
    }
  }

  function cancelRestore() {
    backupToRestore = null;
  }

  function formatTimestamp(ts: string): string {
    try {
      const d = new Date(ts);
      return d.toLocaleString('en-US', { 
        month: 'short', 
        day: 'numeric', 
        year: 'numeric',
        hour: 'numeric',
        minute: '2-digit'
      });
    } catch {
      return ts;
    }
  }

  function formatBytes(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  }

  function getStatusColor(status: string): string {
    switch (status) {
      case 'healthy': return '#34D399';
      case 'behind': return '#FBBF24';
      case 'degraded': return '#F87171';
      default: return '#888888';
    }
  }

  // === ACCOUNT & SYNC FUNCTIONS ===
  async function loadSyncInfo() {
    try {
      const info = await invoke('get_sync_info') as { db_version: number; total_changes: number };
      syncInfo = info;
    } catch (error) {
      console.error('Failed to load sync info:', error);
    }
  }
  
  async function handleSignIn() {
    if (!email || !password) {
      errorMessage = 'Email and password are required';
      return;
    }
    
    loading = true;
    errorMessage = '';
    successMessage = '';
    
    try {
      await signIn(email, password);
      password = '';
      successMessage = 'Signed in successfully';
      setTimeout(() => { successMessage = ''; }, 3000);
      await loadSyncInfo();
    } catch (error: any) {
      errorMessage = error.message || 'Failed to sign in. Check your credentials.';
    } finally {
      loading = false;
    }
  }
  
  async function handleSignUp() {
    if (!email || !password) {
      errorMessage = 'Email and password are required';
      return;
    }
    
    if (password.length < 8) {
      errorMessage = 'Password must be at least 8 characters';
      return;
    }
    
    loading = true;
    errorMessage = '';
    successMessage = '';
    
    try {
      await signUp(email, password);
      password = '';
      successMessage = 'Account created successfully';
      setTimeout(() => { successMessage = ''; }, 3000);
      await loadSyncInfo();
    } catch (error: any) {
      const msg = typeof error === 'string' ? error : error?.message || '';
      if (msg === 'CONFIRM_EMAIL' || msg.includes('CONFIRM_EMAIL')) {
        successMessage = 'Check your email to confirm your account, then sign in.';
        errorMessage = '';
      } else {
        errorMessage = msg || 'Failed to create account.';
      }
    } finally {
      loading = false;
    }
  }
  
  async function handleSignOut() {
    loading = true;
    errorMessage = '';
    
    try {
      await signOut();
      email = '';
      password = '';
      syncInfo = null;
      lastSyncTime.set(null);
    } catch (error: any) {
      errorMessage = error.message || 'Failed to sign out';
    } finally {
      loading = false;
    }
  }
  
  async function handleResetPassword() {
    if (!$userEmail) {
      errorMessage = 'No email found';
      return;
    }
    
    resetPasswordSending = true;
    errorMessage = '';
    resetPasswordSuccess = '';
    
    try {
      await invoke('auth_request_password_reset', { email: $userEmail });
      resetPasswordSuccess = `Reset email sent to ${$userEmail}`;
      setTimeout(() => { resetPasswordSuccess = ''; }, 5000);
    } catch (error: any) {
      errorMessage = error.toString();
    } finally {
      resetPasswordSending = false;
    }
  }
  
  async function handleChangePassword() {
    if (!newPassword || newPassword.length < 6) {
      changePasswordError = 'Password must be at least 6 characters';
      return;
    }
    
    if (newPassword !== confirmPassword) {
      changePasswordError = 'Passwords do not match';
      return;
    }
    
    changingPassword = true;
    changePasswordError = '';
    changePasswordSuccess = '';
    
    try {
      await invoke('auth_change_password', { newPassword });
      changePasswordSuccess = 'Password changed successfully';
      showChangePassword = false;
      newPassword = '';
      confirmPassword = '';
      setTimeout(() => { changePasswordSuccess = ''; }, 3000);
    } catch (error: any) {
      changePasswordError = error.toString();
    } finally {
      changingPassword = false;
    }
  }
  
  async function refreshPeers() {
    try {
      syncPeers = await invoke('discover_peers');
      syncStatus = syncPeers.length > 0 ? `${syncPeers.length} peer${syncPeers.length > 1 ? 's' : ''}` : 'Ready';
    } catch (e) {
      syncStatus = 'Ready';
    }
  }
  
  async function syncNow() {
    syncing = true;
    syncStatus = 'Connecting…';
    try {
      const peers = [...syncPeers];
      if (manualPeer.trim()) {
        const mp = manualPeer.trim();
        peers.push(mp.includes(':') ? mp : `${mp}:21059`);
      }
      if (peers.length === 0) {
        syncStatus = 'No peers — enter IP';
        syncing = false;
        showManualPeer = true;
        return;
      }
      
      let results: string[] = [];
      for (const peer of peers) {
        syncStatus = `Syncing ${peer}…`;
        const result = await syncWithPeer(peer);
        results.push(result);
      }
      
      lastSyncTimeVal = Date.now();
      syncStatus = 'Last synced: just now';
      syncError = null;
      
      if (manualPeer.trim() && results.length > 0) {
        const peerToSave = manualPeer.trim();
        const normalizedPeer = peerToSave.includes(':') ? peerToSave : `${peerToSave}:21059`;
        localStorage.setItem('pane-sync-peer', normalizedPeer);
      }
      
      syncHistory = getSyncHistory();
    } catch (e: any) {
      if (!isRetrying) {
        const errorMessage = `Error: ${String(e).slice(0, 60)}`;
        syncError = errorMessage;
        syncStatus = errorMessage;
        console.error('Sync error:', e);
        
        if (showErrorTimeout) clearTimeout(showErrorTimeout);
        showErrorTimeout = setTimeout(() => {
          syncError = null;
          updateSyncStatus();
          showErrorTimeout = null;
        }, 5000);
      }
    } finally {
      syncing = false;
    }
  }
  
  function toggleSyncHistory() {
    showSyncHistory = !showSyncHistory;
    if (showSyncHistory) {
      syncHistory = getSyncHistory();
    }
  }
  
  function formatSyncTime(timestamp: number): string {
    return new Date(timestamp).toLocaleTimeString();
  }
  
  function updateSyncStatus() {
    if (lastSyncTimeVal) {
      const now = Date.now();
      const diffMs = now - lastSyncTimeVal;
      const minutes = Math.floor(diffMs / (1000 * 60));
      
      if (minutes < 1) {
        syncStatus = 'Last synced: just now';
      } else if (minutes < 60) {
        syncStatus = `Last synced: ${minutes}m ago`;
      } else {
        const hours = Math.floor(minutes / 60);
        if (hours < 24) {
          syncStatus = `Last synced: ${hours}h ago`;
        } else {
          const days = Math.floor(hours / 24);
          syncStatus = `Last synced: ${days}d ago`;
        }
      }
    }
  }
  
  // === ABOUT FUNCTIONS ===
  let checkingForUpdate = $state(false);
  let installingUpdate = $state(false);
  let availableUpdate: Update | null = $state(null);
  let updateStatus = $state('');
  let updateError = $state(false);

  async function checkForUpdates() {
    checkingForUpdate = true;
    availableUpdate = null;
    updateStatus = '';
    updateError = false;

    try {
      const update = await check();
      if (update) {
        availableUpdate = update;
        updateStatus = `Pane ${update.version} is available.`;
      } else {
        updateStatus = 'Pane is up to date.';
      }
    } catch (error) {
      console.error('[Settings] Update check failed:', error);
      updateError = true;
      updateStatus = `Unable to check for updates: ${String(error)}`;
    } finally {
      checkingForUpdate = false;
    }
  }

  async function installAvailableUpdate() {
    if (!availableUpdate) return;

    installingUpdate = true;
    updateError = false;
    updateStatus = `Installing Pane ${availableUpdate.version}…`;

    try {
      await availableUpdate.downloadAndInstall();
      await relaunch();
    } catch (error) {
      console.error('[Settings] Update installation failed:', error);
      updateError = true;
      updateStatus = `Unable to install update: ${String(error)}`;
      installingUpdate = false;
    }
  }
</script>

<div class="settings-container">
  <!-- Segmented Control -->
  <div class="segmented-wrapper">
    <div class="segmented-control">
      <button class="seg-btn" class:active={activeTab === 'general'} onclick={() => activeTab = 'general'}>General</button>
      <button class="seg-btn" class:active={activeTab === 'ai'} onclick={() => activeTab = 'ai'}>AI</button>
      <button class="seg-btn" class:active={activeTab === 'agents'} onclick={() => activeTab = 'agents'}>Agents</button>
      <button class="seg-btn" class:active={activeTab === 'templates'} onclick={() => activeTab = 'templates'}>Templates</button>
      <button class="seg-btn" class:active={activeTab === 'memories'} onclick={() => activeTab = 'memories'}>Memories</button>
      <button class="seg-btn" class:active={activeTab === 'logs'} onclick={() => activeTab = 'logs'}>Daily Logs</button>
      <button class="seg-btn" class:active={activeTab === 'health'} onclick={() => { activeTab = 'health'; loadHealthStats(); loadBackups(); }}>AI Health</button>
      {#if accountEnabled}
      <button class="seg-btn" class:active={activeTab === 'account'} onclick={() => activeTab = 'account'}>Account & Sync</button>
      {/if}
      <button class="seg-btn" class:active={activeTab === 'about'} onclick={() => activeTab = 'about'}>About</button>
    </div>
  </div>
  
  <!-- Tab Content -->
  <div class="tab-scroll">
    <div class="tab-content">
      
      <!-- ==================== GENERAL ==================== -->
      {#if activeTab === 'general'}
        <div class="section">
          <div class="section-title">DATA FOLDER</div>
          <div class="section-card">
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Vault Location</div>
              </div>
              <div class="form-row-right">
                <span class="path-display">{vaultPath || '~/Pane'}</span>
                <button class="btn btn-sm" onclick={selectPaneRoot}>Browse</button>
              </div>
            </div>
          </div>
        </div>
        
        <div class="section">
          <div class="section-title">CATEGORIES</div>
          <div class="section-card">
            {#each definedCategories as cat, i}
              {#if editingCategoryIdx === i}
                <div class="cat-row">
                  <input class="cat-name-input" bind:value={editingCategoryName} placeholder="Name" />
                  <div class="cat-color-row">
                    {#each presetColors as pc}
                      <button class="cat-color-swatch" class:selected={editingCategoryColor === pc} style:background={pc} onclick={() => { editingCategoryColor = pc; }}></button>
                    {/each}
                  </div>
                  <button class="btn-icon save" onclick={() => updateCategory(cat.category, editingCategoryName, editingCategoryColor)}>Save</button>
                  <button class="btn-icon cancel" onclick={() => { editingCategoryIdx = null; }}>✕</button>
                </div>
              {:else}
                <div class="cat-row">
                  <span class="cat-dot" style:background={cat.color}></span>
                  <span class="cat-name">{cat.category}</span>
                  <button class="btn-icon edit" onclick={() => { editingCategoryIdx = i; editingCategoryName = cat.category; editingCategoryColor = cat.color; }}>&#9998;</button>
                  <button class="btn-icon delete" onclick={() => removeCategoryDef(cat.category)}>&times;</button>
                </div>
              {/if}
            {/each}
            
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Add Category</div>
              </div>
              <div class="form-row-right">
                <div class="color-swatch" style:background={newCategoryColor} title="Pick color">
                  <input type="color" bind:value={newCategoryColor} />
                </div>
                <input type="text" class="input" placeholder="Category name" bind:value={newCategoryName} style="width:160px" />
                <button class="btn btn-sm btn-primary" onclick={addCategory}>Add</button>
              </div>
            </div>
          </div>
        </div>
        
        <div class="section">
          <div class="section-title">IMPORT & EXPORT</div>
          <div class="section-card">
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Import Files</div>
                <div class="form-row-sublabel">Select a folder of markdown files to import</div>
              </div>
              <div class="form-row-right">
                <button class="btn" onclick={selectImportFolder} disabled={isFolderImporting}>
                  {isFolderImporting ? 'Importing...' : 'Import Files'}
                </button>
              </div>
            </div>
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Export Data</div>
                <div class="form-row-sublabel">Export all notes and data as a zip archive</div>
              </div>
              <div class="form-row-right">
                <button class="btn" onclick={exportData}>Export Data</button>
              </div>
            </div>
          </div>
        </div>
        
        <div class="section">
          <div class="section-title">APPEARANCE</div>
          <div class="section-card">
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Theme</div>
              </div>
              <div class="form-row-right">
                <select class="select-input">
                  <option selected>Dark</option>
                  <option disabled>Light (coming soon)</option>
                </select>
              </div>
            </div>
          </div>
        </div>
      {/if}
      
      <!-- ==================== AI ==================== -->
      {#if activeTab === 'ai'}
        <div class="section">
          <!-- pairingFingerprint (state var declared above, orphaned since Stage 5's
               ceremony deletion) is passed through so the paired OC card's truncated
               TLS Fingerprint row and this component's own load-on-mount logic share
               one value instead of double-fetching gateway_tls_fingerprint. See
               ProviderConnectionCard.svelte module comment. -->
          <!-- D24 (#2649): onAgentsChanged resets agentsLoaded so the Agents-tab
               $effect above ("Load agents when switching to agents tab") re-fetches
               instead of relying on the user navigating away and back. -->
          <ProviderConnectionsPanel bind:pairingFingerprint onAgentsChanged={() => { agentsLoaded = false; }} />
        </div>

        <!-- #2522 Stage 5d, ADR-0063 D8: "Default agent" trigger — opens DefaultAgentPickerModal,
             the ONE place Settings offers a default-agent comparison across every provider.
             Roster rows on the provider cards above and the Agents tab below stay read-only
             (README-group-b.md: "the same choice is not offered three ways"). -->
        <div class="section">
          <div class="section-card">
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Default agent</div>
                {#if currentDefaultAgent}
                  <div class="form-row-sublabel default-agent-current">
                    <span class="default-agent-name">{currentDefaultAgent.name}</span>
                    <span class="default-agent-sep">·</span>
                    <span class="default-agent-runtime">{RUNTIME_TYPE_LABELS[currentDefaultAgent.runtime_type ?? ''] ?? 'Unassigned'}</span>
                  </div>
                {:else}
                  <div class="form-row-sublabel">Preselected for new sessions, and used for sessions Pane creates on its own</div>
                {/if}
              </div>
              <div class="form-row-right">
                <button class="btn btn-sm" onclick={openDefaultAgentPicker} disabled={defaultPickerLoading}>
                  {defaultPickerLoading ? 'Loading…' : 'Change default'}
                </button>
              </div>
            </div>
          </div>
        </div>
        
        <!-- §29: SMART FEATURES (multi-provider) — ADR-0061 D6, #2522 Stage 5c -->
        <!-- Standalone connection config block below the rule, beneath the provider    -->
        <!-- connection cards. Own single provider dropdown (internalProvider),          -->
        <!-- unchanged from pre-Stage-5 behavior. A Pane subsystem, not a conversation — -->
        <!-- deliberately NOT folded into any ProviderConnectionCard. See                -->
        <!-- docs/design/agent-sessions-sprint/README-group-b.md "Other rulings".        -->
        <div style="border-top:1px solid rgba(255,255,255,0.06);margin:16px 0"></div>
        <div class="section">
          <div class="section-title" style="display:flex;align-items:center;gap:8px">
            SMART FEATURES
            <!-- #2641 D14 Commit 5: three-state status dot (not a full pill+border,
                 per Matt's ruling "keep the existing form layout") — grey/never
                 configured this install, green/configured+no error, red/configured+
                 error. Reuses the .status-dot classes already defined for the Sync
                 section above (grey variant added this commit). -->
            <span class="status-dot {internalStatusDotClass}" data-testid="smart-features-status-dot" title={internalStatusDotClass === 'grey' ? 'Not configured' : internalStatusDotClass === 'red' ? 'Error' : 'Connected'}></span>
          </div>
          <div class="section-description" style="margin-bottom:16px;font-size:13px;color:#A0A0A0">Unlock smart routing, memory reranking, and background AI processing. Choose a provider below.</div>
          
          <div class="section-card">
            <!-- Provider dropdown — C50 (#3171 PR 1): openrouter/anthropic/venice
                 only listed if CONNECTED in Provider Connections above (same
                 shape as §39.2's image-generation picker, `igProviderOptions`).
                 Ollama is always listed — it has no key, only its own endpoint
                 field below, independent of Provider Connections. -->
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Provider</div>
                <div class="form-row-sublabel">Connect OpenRouter, Anthropic, or Venice above to use them here — no second key entry. Ollama needs no connection.</div>
              </div>
              <div class="form-row-right" style="flex:1;max-width:300px">
                <select class="select-input" bind:value={internalProvider} onchange={handleInternalProviderChange}>
                  <option value="">Select a provider...</option>
                  {#each internalProviderOptions as opt}
                    <option value={opt.runtime_type}>{opt.runtime_type === 'openrouter' ? 'OpenRouter' : opt.runtime_type === 'anthropic' ? 'Anthropic' : 'Venice'}</option>
                  {/each}
                  <option value="ollama">Ollama</option>
                </select>
              </div>
            </div>

            {#if !internalProvider || internalProvider === ''}
              <!-- No provider selected -->
              <div style="padding:12px 16px;margin-top:8px;font-size:13px;color:#A0A0A0;line-height:1.6">
                Select a provider above to configure Smart Features
              </div>
            {:else if internalProvider === 'ollama'}
              <!-- Ollama fields — unaffected by C50 (endpoint, not a key) -->
              <div class="form-row">
                <div class="form-row-left">
                  <div class="form-row-label">Ollama Endpoint</div>
                  <div class="form-row-sublabel">Local Ollama server URL</div>
                </div>
                <div class="form-row-right" style="flex:1;max-width:300px">
                  <input type="text" class="input input-wide" bind:value={internalEndpoint} placeholder="http://localhost:11434" />
                </div>
              </div>
            {/if}
            <!-- #3175: openrouter/anthropic/venice branch's "Connected via
                 Provider Connections" status row removed — the Provider
                 row's own sublabel above already states the key is
                 reused. No {:else} needed; ollama is the only branch with
                 its own fields. -->

            {#if internalProvider === 'ollama'}
              <div class="form-row">
                <div class="form-row-left"></div>
                <div class="form-row-right">
                  <button class="btn btn-sm btn-primary" onclick={saveInternalProvider} disabled={loading}>
                    {loading ? 'Saving...' : 'Save & Test Connection'}
                  </button>
                </div>
              </div>

              {#if internalConnectionStatus === 'error'}
                <div style="margin:8px 0;padding:0 16px;font-size:13px;color:#ef4444">
                  ✕ {internalConnectionMessage}
                </div>
              {/if}
            {/if}
            
            {#if internalConnectionStatus === 'success'}
              <div style="margin:16px 0;padding:0 16px;font-size:13px;color:#10b981">
                ● {internalConnectionMessage}
              </div>
            {/if}

            <!-- #3175: orphaned 1px divider removed here — it drew a second
                 rule immediately beneath the now-removed connection-status
                 row; the Provider row's own .form-row border-bottom (CSS)
                 already separates sections. -->
            {#if internalIsConnected}
              <div class="form-row">
                <div class="form-row-left">
                  <div class="form-row-label">Smart Routing Model</div>
                  <div class="form-row-sublabel">Used for message classification and simple queries. Pick a capable model — weak classifiers can degrade your answers.</div>
                </div>
                <div class="form-row-right" style="flex:1">
                  {#if internalProviderModels.length === 0}
                    <div style="flex:1;font-size:13px;color:rgba(255,255,255,0.3);padding:8px 12px;background:rgba(255,255,255,0.03);border:1px solid rgba(255,255,255,0.05);border-radius:6px">
                      Loading models...
                    </div>
                  {:else}
                    <SearchableModelSelect
                      models={internalProviderModels}
                      provider={internalProvider}
                      selectedModel={simpleTierModel}
                      placeholder="Search models... (e.g. Claude, GPT, Gemini)"
                      onSelect={async (modelId) => {
                        simpleTierModel = modelId;
                        await invoke('set_ai_setting', { key: 'simple_tier_model', value: modelId });
                      }}
                    />
                  {/if}
                </div>
              </div>
              
              <div class="form-row">
                <div class="form-row-left">
                  <div class="form-row-label">Extraction Model</div>
                  <div class="form-row-sublabel">Model for memory extraction from conversations</div>
                </div>
                <div class="form-row-right" style="flex:1">
                  {#if internalProviderModels.length === 0}
                    <div style="flex:1;font-size:13px;color:rgba(255,255,255,0.3);padding:8px 12px;background:rgba(255,255,255,0.03);border:1px solid rgba(255,255,255,0.05);border-radius:6px">
                      Loading models...
                    </div>
                  {:else}
                    <SearchableModelSelect
                      models={internalProviderModels}
                      provider={internalProvider}
                      selectedModel={extractionModel}
                      placeholder="Search models..."
                      onSelect={async (modelId) => {
                        extractionModel = modelId;
                        await invoke('set_ai_setting', { key: 'extraction_model', value: modelId });
                      }}
                    />
                  {/if}
                </div>
              </div>

              <div class="form-row">
                <div class="form-row-left">
                  <div class="form-row-label">Rerank Model</div>
                  <div class="form-row-sublabel">Improves memory search accuracy</div>
                </div>
                <div class="form-row-right" style="flex:1">
                  {#if internalProviderModels.length === 0}
                    <div style="flex:1;font-size:13px;color:rgba(255,255,255,0.3);padding:8px 12px;background:rgba(255,255,255,0.03);border:1px solid rgba(255,255,255,0.05);border-radius:6px">
                      Loading models...
                    </div>
                  {:else}
                    <SearchableModelSelect
                      models={internalProviderModels}
                      provider={internalProvider}
                      selectedModel={searchLlmRerankModel}
                      placeholder="Search models..."
                      onSelect={async (modelId) => {
                        searchLlmRerankModel = modelId;
                        await invoke('set_ai_setting', { key: 'memory_search_llm_rerank_model', value: modelId });
                      }}
                    />
                  {/if}
                </div>
              </div>

              {#if internalProvider === 'ollama'}
                <div style="background: rgba(234, 179, 8, 0.1); border: 1px solid rgba(234, 179, 8, 0.3); border-radius: 8px; padding: 12px; margin: 8px 0 16px;">
                  <p style="font-size: 12px; color: #EAB308; margin: 0; font-weight: 600;">⚠️ Model Size Recommendation</p>
                  <p style="font-size: 12px; color: #A1A1AA; margin: 4px 0 0;">Models under 14B parameters may produce unreliable results for extraction and classification. We recommend <code style="background: rgba(255,255,255,0.1); padding: 2px 4px; border-radius: 3px;">qwen2.5:14b</code> or larger.</p>
                </div>
              {/if}
            {/if}
          </div>
        </div>

        {#if errorMessage}
          <div class="error-message">{errorMessage}</div>
          {#if errorMessage.includes('openrouter.ai/settings/credits')}
            <div style="margin-top:4px">
              <a href="https://openrouter.ai/settings/credits" target="_blank" rel="noopener noreferrer" class="text-link">Add credits →</a>
            </div>
          {/if}
        {/if}
        {#if successMessage}
          <div class="success-message">{successMessage}</div>
        {/if}

        <!-- §30: SEMANTIC SEARCH (ADR-0048 / §5.9 / #1657) ———————————————————— -->
        <!-- Slotted below Smart Features, follows the same card pattern:             -->
        <!-- ALL-CAPS eyebrow + lede outside the card, bordered card with rows.       -->
        <div style="border-top:1px solid rgba(255,255,255,0.06);margin:16px 0"></div>
        <div class="section">
          <div class="section-title">SEMANTIC SEARCH</div>
          <div class="section-description" style="margin-bottom:16px;font-size:13px;color:#A0A0A0">Smarter search that understands meaning, not just keywords.</div>

          <div class="section-card">
            <!-- Master toggle row — always visible -->
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Enable semantic search</div>
                <div class="form-row-sublabel">Builds an embedding index of your notes via your chosen provider so search matches by meaning. Runs in the background; off by default.</div>
              </div>
              <div class="form-row-right">
                <label class="toggle-switch">
                  <input type="checkbox" checked={ssEnabled} onchange={handleSsToggle} />
                  <span class="toggle-slider"></span>
                </label>
              </div>
            </div>

            {#if ssEnabled}
              <!-- Inline warning strip — shown while a provider switch is pending/indexing -->
              {#if ssShowWarn}
                <div style="display:flex;align-items:flex-start;gap:8px;padding:10px 16px;background:rgba(234,179,8,0.1);border-top:1px solid rgba(255,255,255,0.06);">
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="#EAB308" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" style="flex-shrink:0;margin-top:1px">
                    <path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z"/><path d="M12 9v4"/><path d="M12 17h.01"/>
                  </svg>
                  <span style="font-size:13px;color:#EAB308"><strong>Switching the provider clears the current index.</strong> Pane will delete the existing embeddings and re-index every note with the new provider. This can take a few minutes.</span>
                </div>
              {/if}

              <!-- Provider dropdown — C50 (#3171 PR 1): openrouter/venice only
                   listed if CONNECTED in Provider Connections above (same shape
                   as §39.2's image-generation picker, `igProviderOptions`).
                   Ollama is always listed — it has no key, only its own endpoint
                   field below, independent of Provider Connections. -->
              <div class="form-row">
                <div class="form-row-left">
                  <div class="form-row-label">Provider</div>
                  <div class="form-row-sublabel">Where embeddings are generated. Ollama runs on your own machine; OpenRouter and Venice reuse the key from Provider Connections — connect one there first. Only providers with embedding support are listed.</div>
                </div>
                <div class="form-row-right" style="flex:1;max-width:300px">
                  <select class="select-input" value={ssProvider}
                    onchange={(e) => handleSsProviderChange((e.target as HTMLSelectElement).value)}>
                    <option value="">Select a provider...</option>
                    {#each ssProviderOptions as opt}
                      <option value={opt.runtime_type}>{opt.runtime_type === 'openrouter' ? 'OpenRouter' : 'Venice'}</option>
                    {/each}
                    <option value="ollama">Ollama</option>
                  </select>
                </div>
              </div>

              {#if ssProvider === 'openrouter' || ssProvider === 'venice'}
                <!-- C50 (#3171 PR 1): NO key field for either arm — both reuse the
                     key from Provider Connections (the dropdown above only ever
                     offers them when connected). -->
                <!-- Embedding model picker -->
                <div class="form-row">
                  <div class="form-row-left">
                    <div class="form-row-label">Embedding model</div>
                    <div class="form-row-sublabel">The model used to turn your notes into vectors for search.</div>
                  </div>
                  <div class="form-row-right" style="flex:1">
                    {#if ssLoadingModels}
                      <div style="flex:1;font-size:13px;color:rgba(255,255,255,0.3);padding:8px 12px;background:rgba(255,255,255,0.03);border:1px solid rgba(255,255,255,0.05);border-radius:6px">
                        Loading models...
                      </div>
                    {:else if ssModels.length === 0}
                      <div style="flex:1;font-size:13px;color:rgba(255,255,255,0.3);padding:8px 12px;background:rgba(255,255,255,0.03);border:1px solid rgba(255,255,255,0.05);border-radius:6px">
                        No models found. Reload Settings to try again.
                      </div>
                    {:else}
                      <SearchableModelSelect
                        models={ssModels}
                        provider={ssProvider}
                        selectedModel={ssModel}
                        placeholder={ssProvider === 'venice' ? "Search models… (e.g. text-embedding-bge-m3)" : "Search models… (e.g. text-embedding-3-small)"}
                        onSelect={async (modelId: string) => { ssModel = modelId; }}
                      />
                    {/if}
                  </div>
                </div>
              {/if}

              {#if ssProvider === 'ollama'}
                <!-- Ollama endpoint -->
                <div class="form-row">
                  <div class="form-row-left">
                    <div class="form-row-label">Ollama Endpoint</div>
                    <div class="form-row-sublabel">Your Ollama server URL</div>
                  </div>
                  <div class="form-row-right" style="flex:1;max-width:300px">
                    <input type="text" class="input input-wide" bind:value={ssEndpoint} placeholder="http://localhost:11434" />
                  </div>
                </div>
                <!-- Embedding model picker -->
                <div class="form-row">
                  <div class="form-row-left">
                    <div class="form-row-label">Embedding model</div>
                    <div class="form-row-sublabel">The model used to turn your notes into vectors for search.</div>
                  </div>
                  <div class="form-row-right" style="flex:1">
                    {#if ssLoadingModels}
                      <div style="flex:1;font-size:13px;color:rgba(255,255,255,0.3);padding:8px 12px;background:rgba(255,255,255,0.03);border:1px solid rgba(255,255,255,0.05);border-radius:6px">
                        Loading models...
                      </div>
                    {:else if ssModels.length > 0}
                      <SearchableModelSelect
                        models={ssModels}
                        provider={ssProvider}
                        selectedModel={ssModel}
                        placeholder="Search models… (e.g. nomic-embed-text)"
                        onSelect={async (modelId: string) => { ssModel = modelId; }}
                      />
                    {:else}
                      <div style="flex:1;font-size:13px;color:rgba(255,255,255,0.3);padding:8px 12px;background:rgba(255,255,255,0.03);border:1px solid rgba(255,255,255,0.05);border-radius:6px">
                        No models found. Make sure Ollama is running, then re-open Settings.
                      </div>
                    {/if}
                  </div>
                </div>
              {/if}

              <!-- Footer: index status + Save & Test -->
              {#if ssProvider}
                <div style="display:flex;align-items:center;justify-content:space-between;padding:12px 16px;border-top:1px solid rgba(255,255,255,0.06);flex-wrap:wrap;gap:8px">
                  <!-- Index status (Gateway-Status pattern) -->
                  <div style="display:flex;align-items:center;gap:8px">
                    {#if !ssStatus || ssStatus.stage === 'disabled' || ssStatus.stage === 'not_configured'}
                      <span style="width:8px;height:8px;border-radius:50%;background:#555;display:inline-block;flex-shrink:0"></span>
                      <span style="font-size:13px;color:rgba(255,255,255,0.4)">Off — not indexed</span>
                    {:else if ssStatus.stage === 'indexing'}
                      <!-- Spinner -->
                      <svg style="animation:spin 1s linear infinite;flex-shrink:0" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="rgba(255,255,255,0.6)" stroke-width="2">
                        <path d="M21 12a9 9 0 1 1-6.219-8.56"/>
                      </svg>
                      <div>
                        <div style="font-size:13px;color:rgba(255,255,255,0.8)">
                          Indexing…
                          {#if ssStatus.notes_total !== undefined}
                            {(ssStatus.notes ?? 0) + (ssStatus.facts ?? 0) + (ssStatus.chats ?? 0)}&nbsp;/&nbsp;{(ssStatus.notes_total ?? 0) + (ssStatus.facts_total ?? 0) + (ssStatus.chats_total ?? 0)}
                          {/if}
                        </div>
                      </div>
                    {:else if ssStatus.stage === 'ready'}
                      <span style="width:8px;height:8px;border-radius:50%;background:#34C759;display:inline-block;flex-shrink:0"></span>
                      <div>
                        <div style="font-size:13px;color:#34C759">Ready</div>
                        {#if ssStatus.notes !== undefined}
                          <div style="font-size:11px;color:rgba(255,255,255,0.4)">{ssStatus.notes} notes · {ssStatus.facts} facts · {ssStatus.chats} chats indexed</div>
                        {/if}
                      </div>
                    {:else if ssStatus.stage === 'partial'}
                      <span style="width:8px;height:8px;border-radius:50%;background:#EAB308;display:inline-block;flex-shrink:0"></span>
                      <div>
                        <div style="font-size:13px;color:#EAB308">Ready (with warnings)</div>
                        {#if ssStatus.notes !== undefined}
                          <div style="font-size:11px;color:rgba(255,255,255,0.4)">{ssStatus.notes} notes · {ssStatus.facts} facts · {ssStatus.chats} chats indexed · {ssStatus.failed ?? 0} failed</div>
                        {/if}
                      </div>
                    {:else if ssStatus.stage === 'error'}
                      <span style="width:8px;height:8px;border-radius:50%;background:#EAB308;display:inline-block;flex-shrink:0"></span>
                      <div>
                        <div style="font-size:13px;color:#EAB308">Unavailable</div>
                        {#if ssStatus.reason}
                          <div style="font-size:11px;color:rgba(255,255,255,0.4)" title={ssStatus.reason}>Provider unreachable</div>
                        {/if}
                      </div>
                    {:else}
                      <span style="width:8px;height:8px;border-radius:50%;background:#555;display:inline-block;flex-shrink:0"></span>
                      <span style="font-size:13px;color:rgba(255,255,255,0.4)">{ssStatus.stage}</span>
                    {/if}
                  </div>

                  <!-- Save & Test button -->
                  <button class="btn btn-sm btn-primary"
                    onclick={handleSsSaveTest}
                    disabled={ssSaving || !ssModel || ssStatus?.stage === 'indexing'}>
                    {ssSaving ? 'Indexing...' : 'Save \u0026 Test'}
                  </button>
                </div>
              {/if}
            {/if}
          </div>
        </div>

        <!-- §31B: WEB SEARCH (§4.6.8, Part 2, #2591) — sits with Semantic Search above
             it, both search features, per the design README's "in context" layout note. -->
        <div style="border-top:1px solid rgba(255,255,255,0.06);margin:16px 0"></div>
        <div class="section">
          <div class="section-title">WEB SEARCH</div>
          <div class="section-description" style="margin-bottom:16px;font-size:13px;color:#A0A0A0">Lets your AI search the web. Reading pages by URL works without this.</div>

          <div class="section-card">
            <!-- Brave Search API key -->
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Brave Search API key</div>
                <div class="form-row-sublabel">You bring your own Brave Search API key. Get one at <a href="https://api-dashboard.search.brave.com/" target="_blank" rel="noopener noreferrer" style="color:#00E4E4">Brave Search API</a>. Searches are sent to Brave with your key. Reading pages exposes your IP address to the sites fetched.</div>
              </div>
              <div class="form-row-right" style="flex:1;max-width:300px">
                {#if wsBraveKeySaved}
                  <div class="password-wrapper">
                    <input type="text" class="input input-wide" value={wsBraveKeyMasked} readonly disabled />
                  </div>
                  <button class="btn btn-sm" onclick={handleRemoveBraveKey} disabled={wsSaving}>Remove</button>
                {:else}
                  <div class="password-wrapper">
                    <input type={wsShowBraveKey ? 'text' : 'password'} class="input input-wide" bind:value={wsBraveKeyInput} placeholder="BSA..." />
                    <button class="password-toggle" onclick={() => wsShowBraveKey = !wsShowBraveKey}>{wsShowBraveKey ? 'Hide' : 'Show'}</button>
                  </div>
                  <button class="btn btn-sm btn-primary" onclick={handleSaveBraveKey} disabled={wsSaving || !wsBraveKeyInput.trim()}>Save</button>
                {/if}
              </div>
            </div>

            <!-- Test -->
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Test</div>
                <div class="form-row-sublabel">Runs a one-result search and reports the outcome below.</div>
              </div>
              <div class="form-row-right">
                <button class="btn btn-sm" onclick={handleTestWebSearch} disabled={wsTesting || !wsAnyConfigured}>
                  {wsTesting ? 'Testing...' : 'Test'}
                </button>
              </div>
            </div>

            <!-- Advanced disclosure: SearXNG endpoint -->
            <div style="border-top:1px solid rgba(255,255,255,0.06)">
              <button
                onclick={() => wsAdvancedOpen = !wsAdvancedOpen}
                style="display:flex;align-items:center;gap:6px;width:100%;padding:12px 16px;background:none;border:none;color:rgba(255,255,255,0.6);font-size:13px;cursor:pointer;text-align:left"
              >
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" style={`transform:rotate(${wsAdvancedOpen ? 90 : 0}deg);transition:transform 0.15s`}>
                  <path d="m9 6 6 6-6 6"/>
                </svg>
                Advanced
              </button>
              {#if wsAdvancedOpen}
                <div class="form-row">
                  <div class="form-row-left">
                    <div class="form-row-label">SearXNG endpoint</div>
                    <div class="form-row-sublabel">Self-hosted search. When set, this replaces Brave.</div>
                  </div>
                  <div class="form-row-right" style="flex:1;max-width:300px">
                    <input type="text" class="input input-wide" bind:value={wsSearxngEndpointInput} placeholder="https://" />
                    {#if wsSearxngEndpoint}
                      <button class="btn btn-sm" onclick={handleClearSearxngEndpoint} disabled={wsSaving}>Clear</button>
                    {/if}
                    <button class="btn btn-sm btn-primary" onclick={handleSaveSearxngEndpoint} disabled={wsSaving || !wsSearxngEndpointInput.trim() || wsSearxngEndpointInput.trim() === wsSearxngEndpoint}>Save</button>
                  </div>
                </div>
              {/if}
            </div>

            <!-- Status line (§4.6.8 states table) -->
            <div style="display:flex;align-items:center;gap:8px;padding:12px 16px;border-top:1px solid rgba(255,255,255,0.06)">
              <span style={`width:8px;height:8px;border-radius:50%;display:inline-block;flex-shrink:0;background:${wsStatusOk ? '#34C759' : '#EAB308'}`}></span>
              <span style={`font-size:13px;color:${wsStatusOk ? 'rgba(255,255,255,0.8)' : '#EAB308'}`}>{wsStatusLine}</span>
            </div>
          </div>
        </div>

        <!-- §39.2: IMAGE GENERATION (#3161 PR 1) — shaped like Semantic Search
             above, but with NO key field (C50: the AI tab already asks for the
             OpenRouter key elsewhere; this adds zero new key inputs). -->
        <div style="border-top:1px solid rgba(255,255,255,0.06);margin:16px 0"></div>
        <div class="section">
          <div class="section-title">IMAGE GENERATION</div>
          <div class="section-description" style="margin-bottom:16px;font-size:13px;color:#A0A0A0">Lets your AI generate images through a connected provider and place them in notes.</div>

          <div class="section-card">
            <!-- Provider dropdown -->
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Provider</div>
                <div class="form-row-sublabel">Only connected providers that offer image models are listed. Connect OpenRouter or Venice to generate images.</div>
              </div>
              <div class="form-row-right" style="flex:1;max-width:300px">
                {#if igProviderOptions.length === 0}
                  <div style="flex:1;font-size:13px;color:rgba(255,255,255,0.3);padding:8px 12px;background:rgba(255,255,255,0.03);border:1px solid rgba(255,255,255,0.05);border-radius:6px">
                    Connect OpenRouter or Venice to generate images.
                  </div>
                {:else}
                  <select class="select-input" value={igProvider}
                    onchange={(e) => handleIgProviderChange((e.target as HTMLSelectElement).value)}>
                    <option value="">Select a provider...</option>
                    {#each igProviderOptions as opt}
                      <option value={opt.runtime_type}>{opt.runtime_type === 'openrouter' ? 'OpenRouter' : opt.runtime_type === 'venice' ? 'Venice' : opt.runtime_type}</option>
                    {/each}
                  </select>
                {/if}
              </div>
            </div>

            {#if igProvider}
              <!-- Image model picker -->
              <div class="form-row">
                <div class="form-row-left">
                  <div class="form-row-label">Image model</div>
                  <div class="form-row-sublabel">Price shown per image where the provider reports it.</div>
                </div>
                <div class="form-row-right" style="flex:1">
                  {#if igLoadingModels}
                    <div style="flex:1;font-size:13px;color:rgba(255,255,255,0.3);padding:8px 12px;background:rgba(255,255,255,0.03);border:1px solid rgba(255,255,255,0.05);border-radius:6px">
                      Loading models...
                    </div>
                  {:else if igModelsError}
                    <div style="flex:1;font-size:13px;color:#EAB308;padding:8px 12px;background:rgba(255,255,255,0.03);border:1px solid rgba(255,255,255,0.05);border-radius:6px">
                      {igModelsError}
                    </div>
                  {:else if igModels.length === 0}
                    <div style="flex:1;font-size:13px;color:rgba(255,255,255,0.3);padding:8px 12px;background:rgba(255,255,255,0.03);border:1px solid rgba(255,255,255,0.05);border-radius:6px">
                      No image models reported by this provider.
                    </div>
                  {:else}
                    <!-- #3171 PR 2 (C51/C57): the SAME searchable component
                         the AI-tab model pickers already use — see
                         `igSearchableModels`'s own doc comment above for
                         the {id, name, provider} shape mapping. -->
                    <SearchableModelSelect
                      models={igSearchableModels}
                      provider={igProvider}
                      selectedModel={igModel}
                      placeholder="Search models..."
                      onSelect={handleIgModelSelect}
                    />
                  {/if}
                </div>
              </div>
            {/if}

            <!-- Test -->
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Test</div>
                <div class="form-row-sublabel">Generates one small image from a fixed prompt into Home/Assets.</div>
              </div>
              <div class="form-row-right">
                <button class="btn btn-sm" onclick={handleTestImageGen} disabled={igTesting || !igProvider || !igModel}>
                  {igTesting ? 'Testing...' : 'Test'}
                </button>
              </div>
            </div>

            {#if igTestResult?.ok && igTestResult.dataUrl}
              <div style="padding:0 16px 12px 16px">
                <img src={igTestResult.dataUrl} alt="Generated by the Image generation Test" style="max-width:200px;max-height:200px;border-radius:6px;border:1px solid rgba(255,255,255,0.08)" />
              </div>
            {/if}

            <!-- §39.4 / #3161 PR 2: per-turn image cap, job turns only. -->
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Max images per job turn</div>
                <div class="form-row-sublabel">A background job can generate at most this many images in one turn. Interactive chat is never capped.</div>
              </div>
              <div class="form-row-right">
                <input type="number" min="1" class="select-input" style="max-width:80px"
                  value={igMaxImagesPerTurn}
                  disabled={igMaxImagesPerTurnSaving}
                  onchange={(e) => handleIgMaxImagesPerTurnChange((e.target as HTMLInputElement).value)} />
              </div>
            </div>

            <!-- Status line -->
            <div style="display:flex;align-items:center;gap:8px;padding:12px 16px;border-top:1px solid rgba(255,255,255,0.06)">
              <span style={`width:8px;height:8px;border-radius:50%;display:inline-block;flex-shrink:0;background:${igStatusOk ? '#34C759' : '#EAB308'}`}></span>
              <span style={`font-size:13px;color:${igStatusOk ? 'rgba(255,255,255,0.8)' : '#EAB308'}`}>{igStatusLine}</span>
            </div>
          </div>
        </div>

        <!-- §32: LINK EMBEDS (ADR-0050 / #401) ————————————————————————————————— -->
        <div style="border-top:1px solid rgba(255,255,255,0.06);margin:16px 0"></div>
        <div class="section">
          <div class="section-title">LINK EMBEDS</div>
          <div class="section-description" style="margin-bottom:16px;font-size:13px;color:#A0A0A0">Allow YouTube and Vimeo links to render as inline video players.</div>

          <div class="section-card">
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Enable link previews (og: metadata enrichment)</div>
                <div class="form-row-sublabel">Fetches og:title, og:description, og:image from link URLs. Pane makes a server-side request to the linked site when you paste a link. Disable to suppress all outbound link fetch.</div>
              </div>
              <div class="form-row-right">
                <label class="toggle-switch">
                  <input type="checkbox" checked={linkPreviewsEnabled} onchange={handleLinkPreviewsToggle} />
                  <span class="toggle-slider"></span>
                </label>
              </div>
            </div>
          </div>
        </div>

        <!-- §31: AI BEHAVIOR (§4.5.2.1 / #1589) ——————————————————————————————— -->
        <div style="border-top:1px solid rgba(255,255,255,0.06);margin:16px 0"></div>
        <div class="section">
          <div class="section-title">AI BEHAVIOR</div>
          <div class="section-description" style="margin-bottom:16px;font-size:13px;color:#A0A0A0">Controls for the AI’s tool-use loop — how many iterations it runs and how long it can take.</div>

          <div class="section-card">
            <!-- §36.2.2 Ruling 4 (#3050 PR 3): relabeled from "Max tool
                 iterations" — same ai_settings key (max_tool_iterations),
                 now a checkpoint interval rather than a hard brake: the
                 agent auto-continues past it (on API-runtime turns) instead
                 of stopping, as long as the job ceiling below has budget
                 left. -->
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Checkpoint every N tool calls</div>
                <div class="form-row-sublabel">The agent pauses to note progress and re-check context, then continues. Each round can include several tool calls.</div>
              </div>
              <div class="form-row-right" style="display:flex;align-items:center;gap:8px;padding-right:8px">
                {#if abMaxIterSavedVisible}
                  <span style="font-size:12px;color:#22C55E">Saved</span>
                {/if}
                <input
                  type="number"
                  class="input"
                  style="width:80px;text-align:right"
                  min="1"
                  max="30"
                  step="1"
                  value={abMaxIter}
                  oninput={(e) => { abMaxIter = parseInt((e.target as HTMLInputElement).value, 10) || abMaxIter; }}
                  onblur={(e) => handleAbBlur('max_tool_iterations', (e.target as HTMLInputElement).value, 1, 30, 30)}
                  onchange={(e) => {
                    const v = parseInt((e.target as HTMLInputElement).value, 10);
                    if (!isNaN(v)) handleAbBlur('max_tool_iterations', v, 1, 30, 30);
                  }}
                />
              </div>
            </div>

            <!-- Loop timeout (#3053 D+E: relabeled, same ai_settings key —
                 renaming the persisted key is a bigger blast radius than
                 relabeling it; see commands.rs's idle_timeout_seconds comment) -->
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">No-progress timeout (seconds)</div>
                <div class="form-row-sublabel">How long the AI’s tool-use loop can go without making progress — no tool call finishing, no new text — before it summarizes and stops. A job that keeps working is never cut off, however long it runs.</div>
              </div>
              <div class="form-row-right" style="display:flex;align-items:center;gap:8px;padding-right:8px">
                {#if abLoopTimeoutSavedVisible}
                  <span style="font-size:12px;color:#22C55E">Saved</span>
                {/if}
                <input
                  type="number"
                  class="input"
                  style="width:80px;text-align:right"
                  min="30"
                  max="300"
                  step="10"
                  value={abLoopTimeout}
                  oninput={(e) => { abLoopTimeout = parseInt((e.target as HTMLInputElement).value, 10) || abLoopTimeout; }}
                  onblur={(e) => handleAbBlur('loop_timeout_seconds', (e.target as HTMLInputElement).value, 30, 300, 120)}
                  onchange={(e) => {
                    const v = parseInt((e.target as HTMLInputElement).value, 10);
                    if (!isNaN(v)) handleAbBlur('loop_timeout_seconds', v, 30, 300, 120);
                  }}
                />
              </div>
            </div>

            <!-- §36.2.2 (#3050 PR 3): the overall job ceiling, new in this
                 PR, shown beside "Checkpoint every N tool calls" per the
                 scope's own instruction. Min clamped to abMaxIter in
                 handleAbBlur (Ruling 4: ceiling >= checkpoint, clamp not
                 validate — setting them equal reproduces the pre-PR-3
                 stop-at-N behaviour, the documented escape hatch). -->
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Overall job ceiling</div>
                <div class="form-row-sublabel">Hard stop per job — total tool calls across every checkpoint this turn auto-continues through. Raise it here, or ask the agent to continue once it stops. Can never be set lower than the checkpoint above.</div>
              </div>
              <div class="form-row-right" style="display:flex;align-items:center;gap:8px;padding-right:8px">
                {#if abJobCeilingSavedVisible}
                  <span style="font-size:12px;color:#22C55E">Saved</span>
                {/if}
                <input
                  type="number"
                  class="input"
                  style="width:80px;text-align:right"
                  min={abMaxIter}
                  max="5000"
                  step="10"
                  value={abJobCeiling}
                  oninput={(e) => { abJobCeiling = parseInt((e.target as HTMLInputElement).value, 10) || abJobCeiling; }}
                  onblur={(e) => handleAbBlur('job_ceiling_tool_calls', (e.target as HTMLInputElement).value, abMaxIter, 5000, 200)}
                  onchange={(e) => {
                    const v = parseInt((e.target as HTMLInputElement).value, 10);
                    if (!isNaN(v)) handleAbBlur('job_ceiling_tool_calls', v, abMaxIter, 5000, 200);
                  }}
                />
              </div>
            </div>

            <!-- §36.2.2 (#3050 PR 8): "Concurrent jobs" — new in this
                 PR, shown beside the overall job ceiling above.
                 `ai_settings.job_concurrency` already exists and is
                 already read by `scheduler_tick`
                 (`ai/scheduler.rs`, `.unwrap_or(RUNTIME_DEFAULT_CONCURRENCY)`)
                 — only this UI row is new. Min 1 (a 0 would silently
                 stall the entire scheduler — no job would ever be
                 claimed); no cross-field clamp, unlike the
                 ceiling/checkpoint pair above. -->
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Concurrent jobs</div>
                <div class="form-row-sublabel">How many scheduled jobs the scheduler runs at once. Interactive turns are never counted against this — they always run.</div>
              </div>
              <div class="form-row-right" style="display:flex;align-items:center;gap:8px;padding-right:8px">
                {#if abJobConcurrencySavedVisible}
                  <span style="font-size:12px;color:#22C55E">Saved</span>
                {/if}
                <input
                  type="number"
                  class="input"
                  style="width:80px;text-align:right"
                  min="1"
                  max="20"
                  step="1"
                  value={abJobConcurrency}
                  oninput={(e) => { abJobConcurrency = parseInt((e.target as HTMLInputElement).value, 10) || abJobConcurrency; }}
                  onblur={(e) => handleAbBlur('job_concurrency', (e.target as HTMLInputElement).value, 1, 20, 2)}
                  onchange={(e) => {
                    const v = parseInt((e.target as HTMLInputElement).value, 10);
                    if (!isNaN(v)) handleAbBlur('job_concurrency', v, 1, 20, 2);
                  }}
                />
              </div>
            </div>
          </div>
        </div>

        <!-- Provider-switch confirm modal -->
        {#if ssShowSwitchModal}
          <div class="modal-overlay" onclick={cancelSsProviderSwitch}>
            <div class="modal-content" onclick={(e) => e.stopPropagation()}>
              <div style="display:flex;align-items:center;gap:8px;margin-bottom:12px">
                <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="#EAB308" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z"/><path d="M12 9v4"/><path d="M12 17h.01"/>
                </svg>
                <div style="font-size:16px;font-weight:600">Switch to {ssProviderLabel(ssPendingProvider)}?</div>
              </div>
              <div style="font-size:13px;color:rgba(255,255,255,0.7);margin-bottom:20px">
                Changing the provider <strong>clears the current index</strong>. Pane will delete the existing embeddings and re-index every note with {ssProviderLabel(ssPendingProvider)}. Search will be unavailable until indexing finishes — usually a few minutes.
              </div>
              <div style="display:flex;gap:8px;justify-content:flex-end">
                <button class="btn" onclick={cancelSsProviderSwitch}>Cancel</button>
                <button class="btn btn-danger" onclick={confirmSsProviderSwitch}>
                  Clear index &amp; switch
                </button>
              </div>
            </div>
          </div>
        {/if}

      {/if}
      
      <!-- ==================== AGENTS ==================== -->
      <!-- #2522 Stage 6a, ADR-0063: grouped by runtime_type. The locally-created vs.
           imported-from-OpenClaw split is gone, and so is every agent disabled because
           the wrong provider was active — the only unavailable state left is
           provider-disconnected, which dims the group and states the cause.
           See docs/design/agent-sessions-sprint/README-group-b.md §7 "Agents page". -->
      {#if activeTab === 'agents'}
        {#if loadingAgents}
          <div class="section">
            <div class="section-card">
              <div class="empty-state"><p>Loading agents…</p></div>
            </div>
          </div>
        {:else}
          <div class="section">
            <!-- Section header: title + "+ New agent" button -->
            <!-- #2522 Stage 6a: the #2392 provider-gate on this button is retired — creation is
                 always enabled now. The picker for which runtime a new agent lands on is 6b's scope. -->
            <div class="section-header-row">
              <div class="section-title">AGENTS ({activeAgentsList.length})</div>
              <button
                class="btn btn-sm btn-primary"
                onclick={openCreateFlow}
              >
                + New agent
              </button>
            </div>

            <!-- §36.17.4 (#3050 PR 10), item 5: helper line, top of the
                 Agents tab content, once total — no other Settings change. -->
            <p class="agents-captain-crew-helper" data-testid="agents-captain-crew-helper">
              Any agent can be a captain (runs a session) or crew (takes jobs from a captain)
            </p>

            {#if agentGroups.length === 0}
              <div class="section-card">
                <div class="empty-state">
                  <p>No agents yet. Create one above.</p>
                </div>
              </div>
            {/if}
            {#each agentGroups as group, groupIdx (group.runtimeType)}
              <div class="agents-group-label" class:agents-group-label--mt={groupIdx > 0}>{group.label}</div>
              <div class="section-card">
                {#each group.agents as agent, idx}
                  {@const globalIdx = agentsList.indexOf(agent)}
                  <div class="form-row" class:agents-row--disabled={!group.connected}>
                    <div class="form-row-left">
                      <div class="form-row-label">
                        {#if editingAgentIdx === globalIdx}
                          <input
                            type="text"
                            class="agent-name-edit"
                            bind:value={editingAgentName}
                            onblur={() => saveAgentName(agent)}
                            onkeydown={(e) => { if (e.key === 'Enter') saveAgentName(agent); }}
                            autofocus
                          />
                        {:else}
                          <div class="agent-name-display">
                            <span>{agent.name}</span>
                            <!-- Subagent label (#2271): role=sub OC agents cannot be set as main.
                                 Scoped to OC-imported agents only (runtime_ref != null) — locally
                                 created agents are also role='sub' but were never labelled this way
                                 pre-#2522 Stage 6a, and the regroup-by-runtime_type change doesn't
                                 alter that distinction.
                                 #2567 B6: locally-created agents now birth with role='local', not
                                 'sub' (see migrate_v81 for the one-time data repair of pre-existing
                                 rows). The `runtime_ref != null` conjunct here was the original
                                 workaround for the old birth mislabel and is no longer strictly
                                 load-bearing post-migration — role='local' agents never match
                                 `agent.role === 'sub'` in the first place — but it's left in place
                                 as defense-in-depth per the same reasoning as the B5 guard in
                                 agents.rs `set_default_agent_impl`. -->
                            {#if agent.role === 'sub' && !agent.is_default && agent.runtime_ref != null}
                              <span class="subagent-label">subagent</span>
                            {/if}
                            <!-- OC main agent: locked rename (checks the agent's own runtime_type) -->
                            {#if agent.is_default && agent.runtime_type === 'openclaw'}
                              <span class="openclaw-locked-label">Linked to OpenClaw</span>
                            {:else if !(agent.runtime_type === 'openclaw' && agent.role === 'sub')}
                              <button
                                class="agent-edit-icon"
                                onclick={() => startEditAgentName(globalIdx, agent.name)}
                                title="Rename agent"
                              >
                                <svg xmlns="http://www.w3.org/2000/svg" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"/><path d="m15 5 4 4"/></svg>
                              </button>
                            {/if}
                          </div>
                        {/if}
                        {#if agent.is_default}
                          <span class="badge badge-teal agents-main-badge">{DEFAULT_CAPTAIN_LABEL}</span>
                        {/if}
                      </div>
                      <div class="form-row-sublabel">
                        <!-- D9 (#2587): subagents (role='sub' with a real OC runtime_ref) never get a
                             model write-back — write_back_agent_model_if_changed only fires for agents
                             Pane itself dispatches to, and subagent dispatch is gateway-side. Showing
                             their model column would mean showing the migrate_v83 'openclaw-pending'
                             sentinel forever (see ProviderConnectionCard.svelte's formatAgentModel for
                             the same sentinel handled on that surface). "Brit Freed" (the main OC agent,
                             runtime_ref set but role='main') and any Pane-dispatched non-sub agent keep
                             showing their real model — only role='sub' + runtime_ref != null is hidden.
                             #3074: second inline pencil next to the model text, same if/editingIdx-else
                             shape as the rename affordance above, but gated by isAgentModelEditable()
                             (agentDisplay.ts, part 1 of #3074) — true only for anthropic/openrouter/
                             venice/ollama. False for 'openclaw' (write_back_agent_model_if_changed
                             would silently revert the edit next turn, ADR-0063 D9) and for null
                             (legacy-unpinned rows with no runtime to fetch models from yet). Those two
                             cases keep today's plain read-only text, unchanged. -->
                        {#if editingAgentModelIdx === globalIdx}
                          {agent.slug} · {group.label} ·
                          <span class="agent-model-edit-wrap">
                            {#if loadingAgentModelOptions}
                              <span class="agent-model-edit-loading">Loading models…</span>
                            {:else if agentModelOptions.length === 0}
                              <span class="agent-model-edit-loading">{agentModelLoadError || 'No models available.'}</span>
                            {:else}
                              <SearchableModelSelect
                                models={agentModelOptions}
                                provider={agent.runtime_type ?? undefined}
                                selectedModel={agent.model ?? ''}
                                placeholder="Search models..."
                                onSelect={(modelId) => saveAgentModel(agent, modelId)}
                              />
                            {/if}
                            <button class="agent-model-edit-cancel" onclick={cancelEditAgentModel} title="Cancel">✕</button>
                          </span>
                        {:else}
                          {agent.slug} · {group.label}{(agent.role === 'sub' && agent.runtime_ref != null) ? '' : (agent.model ? ` · ${agent.model}` : '')}
                          {#if isAgentModelEditable(agent) && !(agent.role === 'sub' && agent.runtime_ref != null)}
                            <button
                              class="agent-model-edit-icon"
                              onclick={() => startEditAgentModel(globalIdx, agent)}
                              title="Change model"
                            >
                              <svg xmlns="http://www.w3.org/2000/svg" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"/><path d="m15 5 4 4"/></svg>
                            </button>
                          {/if}
                        {/if}
                      </div>
                      <!-- §24.7C disabled ≠ hidden: reason chip when the row's own runtime is disconnected -->
                      {#if !group.connected}
                        <div class="agents-reason-chip">
                          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17.5 19H9a7 7 0 1 1 6.71-9h1.79a4.5 4.5 0 1 1 0 9Z"/></svg>
                          {group.label} is disconnected
                        </div>
                      {/if}
                    </div>
                  </div>
                {/each}
              </div>
            {/each}
          </div>
        {/if}

      {/if}

      <!-- Full-window overlays — render regardless of active tab (Bug 1 fix) -->
      {#if showAgentCreate}
        <AgentCreateOverlay
          forcedCreate={agentCreateForced}
          pendingProvider={agentCreatePendingProvider}
          pendingApiKey={pendingProviderSave?.apiKey ?? undefined}
          pendingEndpoint={pendingProviderSave?.endpoint ?? undefined}
          pendingModel={pendingProviderSave?.model ?? undefined}
          onClose={() => { showAgentCreate = false; agentCreateForced = false; agentCreatePendingProvider = ''; pendingProviderSave = null; }}
          onAgentCreated={handleAgentCreated}
        />
      {/if}

      {#if showAgentPicker}
        <AgentPickerModal
          agents={[...locallyCreatedAgentsForPicker].sort((a, b) => b.created_at.localeCompare(a.created_at))}
          onConfirm={async (_agent) => { showAgentPicker = false; if (pendingProviderSave) { await completeDeferredProviderSave(); } }}
          onCreateNew={() => { showAgentPicker = false; showAgentCreate = true; agentCreateForced = true; }}
          onCancel={() => { showAgentPicker = false; pendingProviderSave = null; }}
        />
      {/if}

      {#if showDefaultAgentPicker}
        <DefaultAgentPickerModal
          agents={agentsList}
          connectedRuntimes={defaultPickerConnectedRuntimes}
          onConfirm={async (_agent) => { showDefaultAgentPicker = false; await loadAgents(); }}
          onCancel={() => { showDefaultAgentPicker = false; }}
        />
      {/if}
      
      <!-- ==================== TEMPLATES ==================== -->
      {#if activeTab === 'templates'}
        <!-- spec-covered-by: #2121 (§32.4, ADR-0058) -->
        <div class="section">
          <div class="tpl-set-header">
            <div class="tpl-set-header__txt">
              <div class="tpl-set-eyebrow">Templates</div>
              <p class="tpl-set-lede">Templates you've saved from notes. Revise one by applying it, editing the note, and saving over the same name — there's no editor here.</p>
            </div>
            <span class="tpl-set-count">{settingsTemplates.length} template{settingsTemplates.length !== 1 ? 's' : ''}</span>
          </div>

          {#if templatesLoading}
            <div class="section-card">
              <div class="empty-state"><p>Loading templates…</p></div>
            </div>
          {:else if settingsTemplates.length === 0}
            <div class="section-card">
              <div class="empty-state">
                <p>No templates yet.<br />Save any note as a template from the note menu (⋯ button in the editor toolbar).</p>
              </div>
            </div>
          {:else}
            <div class="tpl-set-list">
              {#each settingsTemplates as t}
                <div class="tpl-srow">
                  <span class="tpl-srow__ico">
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                      <rect x="3" y="3" width="18" height="18" rx="2"/><path d="M3 9h18M9 21V9"/>
                    </svg>
                  </span>
                  {#if renamingTemplateId === t.id}
                    <!-- svelte-ignore a11y_autofocus -->
                    <input
                      class="tpl-srow__rename"
                      bind:value={renameValue}
                      autofocus
                      onkeydown={(e) => {
                        if (e.key === 'Enter') { e.preventDefault(); commitRename(t.id); }
                        if (e.key === 'Escape') { e.preventDefault(); cancelRename(); }
                      }}
                    />
                    <button class="btn btn-sm" onclick={() => commitRename(t.id)} style="flex-shrink:0;background:#00E4E4;color:#0A0A0F;border-color:#00E4E4">Save</button>
                    <button class="btn btn-sm" onclick={cancelRename} style="flex-shrink:0">Cancel</button>
                  {:else}
                    <span class="tpl-srow__body">
                      <span class="tpl-srow__name">{t.name}</span>
                      <span class="tpl-srow__meta">Updated {formatTplDate(t.updated_at)}</span>
                    </span>
                    <span class="tpl-srow__actions">
                      <button class="tpl-iconbtn" title="Rename" onclick={() => startRename(t)}>
                        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                          <path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z"/>
                        </svg>
                      </button>
                      <button class="tpl-iconbtn tpl-iconbtn--danger" title="Delete" onclick={() => startDelete(t)}>
                        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                          <path d="M3 6h18M8 6V4a1 1 0 0 1 1-1h6a1 1 0 0 1 1 1v2M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/>
                        </svg>
                      </button>
                    </span>
                  {/if}
                </div>
              {/each}
            </div>
          {/if}
        </div>

        <!-- Delete confirm modal (§32.4) — #2874 item G: converted from a
             hand-rolled modal to the shared ConfirmDialog.svelte. -->
        {#if tplDeleteConfirmId}
          <ConfirmDialog
            title="Delete template?"
            message={`"${tplDeleteConfirmName}" will be permanently deleted. Notes created from it are unaffected.`}
            error={tplDeleteConfirmError}
            onConfirm={confirmDeleteTemplate}
            onCancel={cancelDeleteTemplate}
          />
        {/if}
      {/if}
      
      <!-- ==================== MEMORIES ==================== -->
      {#if activeTab === 'memories'}
        <div class="section">
          <div class="section-title">MEMORY EXTRACTION</div>
          <div class="section-card">
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Extract facts from conversations</div>
                <div class="form-row-sublabel">After each chat, the AI pulls notable facts and saves them to your memory.</div>
              </div>
              <div class="form-row-right">
                <div class="toggle" class:on={memoryEnabled} onclick={() => { memoryEnabled = !memoryEnabled; saveMemorySettings(); }}></div>
              </div>
            </div>
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Synthesis</div>
                <div class="form-row-sublabel">Periodically synthesize and consolidate memories</div>
              </div>
              <div class="form-row-right">
                <div class="toggle" class:on={synthesisEnabled} onclick={() => { synthesisEnabled = !synthesisEnabled; saveMemorySettings(); }}></div>
              </div>
            </div>
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Daily Log</div>
                <div class="form-row-sublabel">Maintain a running daily activity log</div>
              </div>
              <div class="form-row-right">
                <div class="toggle" class:on={dailyLogEnabled} onclick={() => { dailyLogEnabled = !dailyLogEnabled; saveMemorySettings(); }}></div>
              </div>
            </div>
            <!-- §5.4B: Memory extraction toast toggle (#641) -->
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Memory Toasts</div>
                <div class="form-row-sublabel">Show a toast when new memories are extracted</div>
              </div>
              <div class="form-row-right">
                <div class="toggle" class:on={showMemoryToasts} onclick={() => { showMemoryToasts = !showMemoryToasts; saveMemorySettings(); }}></div>
              </div>
            </div>
          </div>
        </div>
        
        <div class="section">
          <div class="section-title">MEMORY HEALTH</div>
          <div class="section-card">
            <div class="stat-row">
              <span class="stat-label">Extraction</span>
              <span class="stat-value">Active <span class="stat-check">&#10003;</span></span>
            </div>
            <div class="stat-row">
              <span class="stat-label">Last synthesis</span>
              <span class="stat-value">{lastSynthesisTime}</span>
            </div>
            <div class="stat-row">
              <span class="stat-label">Facts stored</span>
              <span class="stat-value">{memoryCount}</span>
            </div>
          </div>
        </div>
        
        <div class="section">
          <div class="section-title">MEMORY SEARCH</div>
          <div class="section-card">
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Include raw conversations in search</div>
                <div class="form-row-sublabel">Search both extracted facts and original chat messages.</div>
              </div>
              <div class="form-row-right">
                <div class="toggle" class:on={searchFuseBothSources} onclick={async () => {
                  searchFuseBothSources = !searchFuseBothSources;
                  await saveMemorySearchSetting('memory_search_fuse_both_sources', String(searchFuseBothSources));
                }}></div>
              </div>
            </div>
            
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Keyword match importance</div>
                <div class="form-row-sublabel">How much to weight exact keyword matches (0% = semantic only, 50% = heavy keyword bias)</div>
              </div>
              <div class="form-row-right">
                <input 
                  type="range" 
                  min="0" 
                  max="0.5" 
                  step="0.05" 
                  bind:value={searchKeywordWeight}
                  onchange={async () => await saveMemorySearchSetting('memory_search_keyword_weight', String(searchKeywordWeight))}
                  class="slider"
                />
                <span class="slider-value">{Math.round(searchKeywordWeight * 100)}%</span>
              </div>
            </div>
            
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Recency boost strength</div>
                <div class="form-row-sublabel">How quickly older memories fade in relevance (7 days = moderate, 14 days = slow fade)</div>
              </div>
              <div class="form-row-right">
                <input 
                  type="range" 
                  min="0" 
                  max="14" 
                  step="1" 
                  bind:value={searchTemporalHalfLife}
                  onchange={async () => await saveMemorySearchSetting('memory_search_temporal_half_life_days', String(searchTemporalHalfLife))}
                  class="slider"
                />
                <span class="slider-value">{searchTemporalHalfLife} days</span>
              </div>
            </div>
            
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">AI-powered reranking</div>
                <div class="form-row-sublabel">
                  Re-orders results by relevance using an LLM.
                </div>
              </div>
              <div class="form-row-right">
                <div 
                  class="toggle" 
                  class:on={searchLlmRerankEnabled} 
                  onclick={async () => {
                    searchLlmRerankEnabled = !searchLlmRerankEnabled;
                    await saveMemorySearchSetting('memory_search_llm_rerank_enabled', String(searchLlmRerankEnabled));
                  }}
                ></div>
              </div>
            </div>
            
            {#if searchLlmRerankEnabled}
              <div class="form-row">
                <div class="form-row-left">
                  <div class="form-row-label">Rerank model</div>
                  <div class="form-row-sublabel">Model configured in Settings → AI</div>
                </div>
                <div class="form-row-right" style="flex:1">
                  <div style="font-size:13px;color:rgba(255,255,255,0.4);padding:8px 12px;background:rgba(255,255,255,0.03);border:1px solid rgba(255,255,255,0.05);border-radius:6px">
                    {#if searchLlmRerankModel}
                      Rerank model: {searchLlmRerankModel} · <button style="background:none;border:none;color:rgba(255,255,255,0.5);cursor:pointer;text-decoration:underline;padding:0;font-size:13px" onclick={() => { activeTab = 'ai'; }}>Set in Settings → AI</button>
                    {:else}
                      No rerank model configured · <button style="background:none;border:none;color:rgba(255,255,255,0.5);cursor:pointer;text-decoration:underline;padding:0;font-size:13px" onclick={() => { activeTab = 'ai'; }}>Configure in Settings → AI</button>
                    {/if}
                  </div>
                </div>
              </div>
            {/if}
            
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Fact priority boost</div>
                <div class="form-row-sublabel">How much to prioritize extracted facts over raw conversations</div>
              </div>
              <div class="form-row-right">
                <input 
                  type="range" 
                  min="1" 
                  max="2" 
                  step="0.1" 
                  bind:value={searchFactBoost}
                  onchange={async () => await saveMemorySearchSetting('memory_search_fact_boost', String(searchFactBoost))}
                  class="slider"
                />
                <span class="slider-value">{searchFactBoost.toFixed(1)}x</span>
              </div>
            </div>
          </div>
        </div>
        
        <div class="section">
          <div class="section-title">FACT MANAGEMENT</div>
          <div class="memory-info-note" style="margin-bottom: 12px;">
            ℹ️ These facts are the source of truth for your agent's memory files. Editing or deleting facts here updates the memory files automatically.
          </div>
          <div class="section-card">
            <!-- Search and filter bar -->
            <div class="fact-filters">
              <input 
                type="text" 
                class="input" 
                placeholder="Search facts..." 
                bind:value={searchQuery}
                style="flex: 1;"
              />
              <select class="select-input" bind:value={categoryFilter}>
                <option value="all">All categories</option>
                {#each uniqueCategories as cat}
                  <option value={cat}>{cat}</option>
                {/each}
              </select>
            </div>
            
            <!-- Fact list -->
            {#if displayedMemories.length === 0}
              <div class="fact-empty">
                {searchQuery || categoryFilter !== 'all'
                  ? 'No facts match your filters'
                  : 'No facts extracted yet. Start chatting with Pane to build memory.'}
              </div>
            {:else}
              <div class="fact-list">
                {#each displayedMemories as memory}
                  <div class="fact-row">
                    {#if editingMemoryId === memory.id}
                      <!-- Edit mode -->
                      <div class="fact-edit-container">
                        <textarea 
                          class="fact-edit-textarea" 
                          bind:value={editingMemoryText}
                          rows="3"
                        ></textarea>
                        <div class="fact-edit-actions">
                          <button class="btn btn-sm btn-primary" onclick={() => saveEditMemory(memory.id)}>Save</button>
                          <button class="btn btn-sm" onclick={cancelEditMemory}>Cancel</button>
                        </div>
                      </div>
                    {:else if deleteConfirmId === memory.id}
                      <!-- Delete confirmation -->
                      <div class="fact-delete-container">
                        <span class="fact-delete-text">Delete this fact?</span>
                        <div class="fact-delete-actions">
                          <button class="btn btn-sm btn-danger" onclick={() => confirmDelete(memory.id)}>Yes</button>
                          <button class="btn btn-sm" onclick={cancelDelete}>No</button>
                        </div>
                      </div>
                    {:else}
                      <!-- Normal view -->
                      <div class="fact-content">
                        <span class="fact-text" title={memory.fact}>{memory.fact}</span>
                      </div>
                      <span class="fact-category-badge">{memory.category || '—'}</span>
                      <span class="fact-time">{formatRelativeTime(memory.updated_at || memory.created_at)}</span>
                      <div class="fact-meta-actions">
                        <button class="btn-icon" onclick={() => startEditMemory(memory.id, memory.fact)} title="Edit">✎</button>
                        <button class="btn-icon delete" onclick={() => showDeleteConfirm(memory.id)} title="Delete">×</button>
                      </div>
                    {/if}
                  </div>
                {/each}
              </div>
              
              <!-- Load more button -->
              {#if filteredMemories.length > displayLimit}
                <div class="fact-load-more">
                  <button class="btn btn-sm" onclick={loadMoreMemories}>
                    Load more ({filteredMemories.length - displayLimit} remaining)
                  </button>
                </div>
              {/if}
            {/if}
          </div>
        </div>
      {/if}
      
      <!-- ==================== DAILY LOGS ==================== -->
      {#if activeTab === 'logs'}
        <div class="logs-container">
          <!-- Left: Log list -->
          <div class="logs-list">
            {#each dailyLogs as log}
              <button 
                class="log-card" 
                class:active={selectedLog?.file_path === log.file_path}
                onclick={() => selectLog(log)}
              >
                <div class="log-date">{formatLogDate(log.date)}</div>
                <div class="log-meta">
                  <span class="log-agent">{log.agent}</span>
                  <span class="log-size">{formatSize(log.size)}</span>
                </div>
              </button>
            {/each}
            {#if dailyLogs.length === 0}
              <div class="logs-empty">No daily logs yet</div>
            {/if}
          </div>
          
          <!-- Right: Log viewer -->
          <div class="log-viewer">
            {#if selectedLogContent}
              <div class="log-viewer-header">
                <span class="log-viewer-title">{selectedLog?.date} — {selectedLog?.agent}</span>
              </div>
              <div class="log-viewer-content">
                {@html renderMarkdown(selectedLogContent)}
              </div>
            {:else}
              <div class="log-viewer-empty">Select a log to view</div>
            {/if}
          </div>
        </div>
      {/if}
      
      <!-- ==================== AI HEALTH ==================== -->
      {#if activeTab === 'health'}
        <!-- Context Budget Panel -->
        <div class="section">
          <div class="section-title">CONTEXT BUDGET</div>
          <div class="section-card">
            {#if budgetStats}
              <div class="health-panel">
                <div class="health-row">
                  <span class="health-label">Model</span>
                  <span class="health-value">{budgetStats.model}</span>
                </div>
                {#if provider === 'openclaw'}
                  <!-- OC: read-only with lock label per .openclaw-locked-label pattern (#1404-E) -->
                  <div class="health-row">
                    <span class="health-label">Context Window</span>
                    <span class="health-value">
                      {budgetStats.context_window.toLocaleString()} tokens
                      {#if budgetStats.is_estimated}<span class="ctx-estimated">(estimated)</span>{/if}
                    </span>
                    <span class="openclaw-locked-label">Set in OpenClaw</span>
                  </div>
                {:else}
                  <!-- Non-OC: editable numeric input + Clear button (#1404-E §27.11) -->
                  <div class="health-row ctx-window-row">
                    <span class="health-label">Context Window</span>
                    <div class="ctx-window-controls">
                      <div class="ctx-window-input-row">
                        <input
                          type="number"
                          min="50000"
                          class="ctx-window-input"
                          bind:value={ctxWindowInput}
                          disabled={ctxWindowSaving}
                          onblur={saveContextWindowOverride}
                          onkeydown={(e) => { if (e.key === 'Enter') saveContextWindowOverride(); }}
                        />
                        <span class="ctx-window-unit">tokens</span>
                        <button
                          class="ctx-clear-btn"
                          disabled={ctxWindowSaving}
                          onclick={clearContextWindowOverride}
                        >Clear override</button>
                      </div>
                      {#if ctxWindowError}
                        <span class="ctx-error">{ctxWindowError}</span>
                      {:else if ctxWindowCapped}
                        <span class="ctx-note">{ctxWindowCapped}</span>
                      {:else if budgetStats.is_estimated}
                        <span class="ctx-note">(estimated — no live API source for this model)</span>
                      {/if}
                    </div>
                  </div>
                {/if}
                <div class="health-row">
                  <span class="health-label">Used / Available</span>
                  <span class="health-value">{budgetStats.total_used.toLocaleString()} / {budgetStats.available_budget.toLocaleString()} 
                    ({((budgetStats.total_used / budgetStats.available_budget) * 100).toFixed(1)}%)</span>
                </div>
                
                <div class="budget-tiers" style="margin-top:16px">
                  {#each budgetStats.tiers as tier}
                    <div class="tier-row">
                      <div class="tier-label">T{tier.tier} — {tier.name}</div>
                      <div class="tier-bar-container">
                        <div class="tier-bar" style="width:{(tier.tokens / budgetStats.available_budget) * 100}%"></div>
                      </div>
                      <div class="tier-tokens">
                        {tier.tokens.toLocaleString()} 
                        ({((tier.tokens / budgetStats.total_used) * 100).toFixed(1)}%)
                      </div>
                    </div>
                  {/each}
                </div>
                
                <div class="health-row" style="margin-top:16px; border-top:1px solid rgba(255,255,255,0.06); padding-top:12px">
                  <span class="health-label">Messages</span>
                  <span class="health-value">{budgetStats.messages_included} / {budgetStats.messages_total} included</span>
                </div>
                <div class="health-row">
                  <span class="health-label">Images</span>
                  <span class="health-value">{budgetStats.images_included} inline, {budgetStats.images_replaced} replaced</span>
                </div>
                <div class="health-row">
                  <span class="health-label">Cache Prefix</span>
                  <span class="health-value">
                    {budgetStats.cache_prefix_tokens.toLocaleString()} tokens 
                    {#if budgetStats.cache_prefix_stable}
                      <span style="color:#34D399">● stable</span>
                    {:else}
                      <span style="color:#FBBF24">● {budgetStats.cache_prefix_change_reason}</span>
                    {/if}
                  </span>
                </div>
              </div>
            {:else}
              <div style="padding:16px;color:rgba(255,255,255,0.5)">
                No budget data available yet. Context budget is calculated on each AI request.
              </div>
            {/if}
          </div>
        </div>

        <!-- Extraction Health Panel -->
        <div class="section">
          <div class="section-title">EXTRACTION QUEUE</div>
          <div class="section-card">
            {#if extractionStats}
              <div class="health-panel">
                <div class="health-row">
                  <span class="health-label">Status</span>
                  <span class="health-value">
                    <span style="color:{getStatusColor(extractionStats.status)}">●</span>
                    {extractionStats.status}
                  </span>
                </div>
                <div class="health-row">
                  <span class="health-label">Queue Depth</span>
                  <span class="health-value">{extractionStats.queue_depth} pending</span>
                </div>
                <div class="health-row">
                  <span class="health-label">Last Processed</span>
                  <span class="health-value">{extractionStats.last_processed ? formatTimestamp(extractionStats.last_processed) : 'Never'}</span>
                </div>
                <div class="health-row">
                  <span class="health-label">Facts Extracted Today</span>
                  <span class="health-value">{extractionStats.facts_today}</span>
                </div>
                <div class="health-row">
                  <span class="health-label">Failed Extractions</span>
                  <span class="health-value">{extractionStats.failed_count}</span>
                </div>
                <div class="health-row" style="border-top:1px solid rgba(255,255,255,0.06); padding-top:12px; margin-top:12px">
                  <span class="health-label">Total Processed</span>
                  <span class="health-value">{extractionStats.total_processed}</span>
                </div>
                <div class="health-row">
                  <span class="health-label">Total Facts</span>
                  <span class="health-value">{extractionStats.total_facts}</span>
                </div>
                <div class="health-row">
                  <span class="health-label">Failure Rate</span>
                  <span class="health-value">{extractionStats.failure_rate.toFixed(2)}%</span>
                </div>
              </div>
            {:else}
              <div style="padding:16px;color:rgba(255,255,255,0.5)">
                Loading extraction stats...
              </div>
            {/if}
          </div>
        </div>

        <!-- Compaction Status Panel -->
        <div class="section">
          <div class="section-title">COMPACTION</div>
          <div class="section-card">
            {#if compactionStats}
              <div class="health-panel">
                <div class="health-row">
                  <span class="health-label">Active Thread</span>
                  <span class="health-value">{compactionStats.thread_name}</span>
                </div>
                <div class="health-row">
                  <span class="health-label">Messages in Conversation</span>
                  <span class="health-value">{compactionStats.messages_in_conversation}</span>
                </div>
                <div class="health-row">
                  <span class="health-label">Messages in Context</span>
                  <span class="health-value">{compactionStats.messages_in_context}</span>
                </div>
                <div class="health-row">
                  <span class="health-label">Compaction Summaries</span>
                  <span class="health-value">{compactionStats.compaction_summary_count}</span>
                </div>
                <div class="health-row">
                  <span class="health-label">Last Compaction</span>
                  <span class="health-value">{compactionStats.last_compaction ? formatTimestamp(compactionStats.last_compaction) : 'Never'}</span>
                </div>
              </div>
            {:else}
              <div style="padding:16px;color:rgba(255,255,255,0.5)">
                Loading compaction stats...
              </div>
            {/if}
          </div>
        </div>

        <!-- Backup Panel -->
        <div class="section">
          <div class="section-title">BACKUPS</div>
          <div class="section-card">
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Manual Backup</div>
                <div class="form-row-sublabel">Create a backup of your database now</div>
              </div>
              <div class="form-row-right">
                <button class="btn btn-sm btn-primary" onclick={createBackupNow} disabled={creatingBackup}>
                  {creatingBackup ? 'Creating...' : 'Backup Now'}
                </button>
              </div>
            </div>
            
            <div style="border-top:1px solid rgba(255,255,255,0.06); margin-top:16px; padding-top:16px">
              <div style="font-size:12px; color:rgba(255,255,255,0.7); margin-bottom:12px; font-weight:500">
                Available Backups
              </div>
              {#if backupsList.length > 0}
                <div class="backups-list">
                  {#each backupsList as backup}
                    <div class="backup-card">
                      <div class="backup-info">
                        <div class="backup-filename">{backup.filename}</div>
                        <div class="backup-meta">
                          {formatTimestamp(backup.timestamp)} · {formatBytes(backup.size_bytes)} · {backup.backup_type}
                        </div>
                      </div>
                      <button 
                        class="btn btn-sm" 
                        onclick={() => confirmRestore(backup.path)}
                        disabled={restoringBackup}
                      >
                        Restore
                      </button>
                    </div>
                  {/each}
                </div>
              {:else}
                <div style="padding:12px;color:rgba(255,255,255,0.4);font-size:12px">
                  No backups found
                </div>
              {/if}
            </div>
          </div>
        </div>

        <!-- Restore Confirmation Dialog -->
        {#if backupToRestore}
          <div class="modal-overlay" onclick={cancelRestore}>
            <div class="modal-content" onclick={(e) => e.stopPropagation()}>
              <div style="font-size:16px;font-weight:600;margin-bottom:12px">Confirm Restore</div>
              <div style="font-size:13px;color:rgba(255,255,255,0.7);margin-bottom:20px">
                Are you sure you want to restore this backup? This will replace your current database. 
                You should create a backup of your current state first.
              </div>
              <div style="display:flex;gap:8px;justify-content:flex-end">
                <button class="btn" onclick={cancelRestore}>Cancel</button>
                <button class="btn btn-danger" onclick={doRestore} disabled={restoringBackup}>
                  {restoringBackup ? 'Restoring...' : 'Restore Backup'}
                </button>
              </div>
            </div>
          </div>
        {/if}
      {/if}
      
      <!-- ==================== ACCOUNT & SYNC ==================== -->
      {#if activeTab === 'account' && accountEnabled}
        <!-- ADR-0070 Decision 3 (#3195 PR B): no Account block when the
             flag is off — every command the sign-in form below would call
             is guarded off, so showing it would only produce a misleading
             "Failed to sign in" error. T3 (red-first). -->
        {#if accountEnabled}
        <div class="section">
          <div class="section-title">ACCOUNT</div>
          <div class="section-card">
            {#if $isAuthenticated}
              <div class="form-row">
                <div class="form-row-left">
                  <div class="form-row-label">Email</div>
                  <div class="form-row-sublabel">{$userEmail}</div>
                </div>
                <div class="form-row-right">
                  <button class="btn btn-sm" onclick={() => showChangePassword = !showChangePassword}>Change Password</button>
                  <button class="btn btn-sm" onclick={handleResetPassword} disabled={resetPasswordSending}>
                    {resetPasswordSending ? 'Sending...' : 'Reset Password'}
                  </button>
                  <button class="btn btn-sm btn-danger" onclick={handleSignOut}>Sign Out</button>
                </div>
              </div>
              {#if showChangePassword}
                <div style="padding: 12px 16px; border-top: 1px solid rgba(255,255,255,0.06);">
                  <input type="password" class="input input-wide" placeholder="New password" bind:value={newPassword} style="margin-bottom:8px" />
                  <input type="password" class="input input-wide" placeholder="Confirm new password" bind:value={confirmPassword} style="margin-bottom:12px" />
                  <div style="display:flex;gap:8px;align-items:center">
                    <button class="btn btn-sm btn-primary" onclick={handleChangePassword} disabled={changingPassword || !newPassword || newPassword !== confirmPassword}>
                      {changingPassword ? 'Saving...' : 'Save Password'}
                    </button>
                    <button class="btn btn-sm" onclick={() => { showChangePassword = false; newPassword = ''; confirmPassword = ''; changePasswordError = ''; }}>Cancel</button>
                    {#if newPassword && confirmPassword && newPassword !== confirmPassword}
                      <span style="font-size:12px;color:#ff4444">Passwords don't match</span>
                    {/if}
                    {#if changePasswordError}
                      <span style="font-size:12px;color:#ff4444">{changePasswordError}</span>
                    {/if}
                  </div>
                </div>
              {/if}
              {#if resetPasswordSuccess}
                <div class="success-message" style="margin:12px 16px">{resetPasswordSuccess}</div>
              {/if}
              {#if changePasswordSuccess}
                <div class="success-message" style="margin:12px 16px">{changePasswordSuccess}</div>
              {/if}
            {:else}
              <div style="padding:16px">
                <p style="margin:0 0 16px 0;font-size:13px;color:rgba(255,255,255,0.5)">Sign in to sync across devices</p>
                <input type="email" class="input input-wide" placeholder="Email" bind:value={email} style="margin-bottom:8px" />
                <input type="password" class="input input-wide" placeholder="Password" bind:value={password} style="margin-bottom:12px" />
                <div style="display:flex;gap:8px">
                  <button class="btn btn-primary" onclick={handleSignIn} disabled={loading}>
                    {loading ? 'Signing in...' : 'Sign In'}
                  </button>
                  <button class="btn" onclick={handleSignUp} disabled={loading}>Create Account</button>
                </div>
                {#if errorMessage}
                  <div class="error-message" style="margin-top:12px">{errorMessage}</div>
                {/if}
                {#if successMessage}
                  <div class="success-message" style="margin-top:12px">{successMessage}</div>
                {/if}
              </div>
            {/if}
          </div>
        </div>
        {/if}
        
        <div class="section">
          <div class="section-title">SYNC</div>
          <div class="section-card">
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Auto-sync</div>
                <div class="form-row-sublabel">Automatically sync changes across devices</div>
              </div>
              <div class="form-row-right">
                <div class="toggle" class:on={$autoSyncEnabled} onclick={() => $autoSyncEnabled ? stopAutoSync() : startAutoSync()}></div>
              </div>
            </div>
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Sync Status</div>
                <div class="form-row-sublabel">Last synced: {lastSyncTimeVal ? formatSyncTime(lastSyncTimeVal) : 'Never'}</div>
              </div>
              <div class="form-row-right">
                <span class="status-dot green"></span>
                <button class="btn btn-sm btn-primary" onclick={syncNow} disabled={syncing}>
                  {syncing ? 'Syncing...' : 'Sync Now'}
                </button>
              </div>
            </div>
            <div class="form-row">
              <div class="form-row-left">
                <div class="form-row-label">Device Name</div>
              </div>
              <div class="form-row-right">
                <input type="text" class="input" bind:value={deviceName} style="width:180px" />
              </div>
            </div>
            <div class="sync-history-toggle" onclick={toggleSyncHistory}>
              <span>Sync History</span>
              <span>{showSyncHistory ? '▴' : '▾'}</span>
            </div>
            {#if showSyncHistory}
              <div class="sync-history-content">
                {#if syncHistory.length === 0}
                  <p style="font-size:12px;color:rgba(255,255,255,0.3);margin:8px 0">No sync history yet</p>
                {:else}
                  {#each syncHistory as entry}
                    <div class="sync-entry">
                      <span class="sync-time">{formatSyncTime(entry.timestamp)}</span>
                      <span class="sync-detail">
                        {#if entry.error}
                          <span style="color:#ff4444">{entry.error}</span>
                        {:else}
                          Sent: {entry.changesSent}, Received: {entry.changesReceived}
                        {/if}
                      </span>
                    </div>
                  {/each}
                {/if}
              </div>
            {/if}
          </div>
        </div>
      
      {/if}
      
      <!-- ==================== ABOUT ==================== -->
      {#if activeTab === 'about'}
        <div class="about-center">
          <div class="about-logo">Pane</div>
          <div class="about-version">Version {appVersion}</div>
          <button class="btn btn-primary" onclick={checkForUpdates} disabled={checkingForUpdate || installingUpdate}>
            {checkingForUpdate ? 'Checking…' : 'Check for Updates'}
          </button>
          {#if updateStatus}
            <div class="update-status" class:error={updateError}>{updateStatus}</div>
          {/if}
          {#if availableUpdate}
            <button class="btn" onclick={installAvailableUpdate} disabled={installingUpdate}>
              {installingUpdate ? 'Installing…' : `Install Pane ${availableUpdate.version}`}
            </button>
          {/if}
          
          <div class="about-links">
            <a href="https://paneapp.ai/" target="_blank">Website</a>
            <a href="https://paneapp.ai/docs" target="_blank">Documentation</a>
            <a href="https://discord.com/invite/PhRd2yGwR8" target="_blank">Support</a>
            <a href="https://x.com/paneaiapp" target="_blank">X/Twitter</a>
          </div>
          
          <div class="about-made">Made by Three Lane Studios LLC</div>
          <div class="about-legal">
            Copyright 2025-2026 Three Lane Studios LLC. All rights reserved.<br />
            Licensed under the terms of the Pane End User License Agreement.
          </div>
        </div>
      {/if}
      
    </div>
  </div>
</div>

{#if importToast}
  <div class="import-toast">{importToast}</div>
{/if}

<style>
  /* Base */
  .settings-container {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: #0a0a0f;
    color: #e8e8ed;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', system-ui, sans-serif;
  }
  
  /* Segmented Control */
  .segmented-wrapper {
    display: flex;
    justify-content: center;
    padding: 20px 20px 0;
    flex-shrink: 0;
    overflow-x: auto;
    -webkit-overflow-scrolling: touch;
  }
  
  .segmented-control {
    display: inline-flex;
    background: rgba(255,255,255,0.05);
    border-radius: 8px;
    padding: 3px;
    gap: 2px;
    border: 1px solid rgba(255,255,255,0.08);
    flex-shrink: 0;
  }
  
  .seg-btn {
    padding: 7px 16px;
    font-size: 13px;
    font-weight: 500;
    color: rgba(255,255,255,0.5);
    background: transparent;
    border: none;
    border-radius: 6px;
    cursor: pointer;
    transition: all 0.2s;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', system-ui, sans-serif;
    white-space: nowrap;
  }
  
  @media (max-width: 700px) {
    .seg-btn {
      padding: 7px 10px;
      font-size: 12px;
    }
  }
  
  .seg-btn:hover {
    color: #e8e8ed;
    background: rgba(255,255,255,0.05);
  }
  
  .seg-btn.active {
    background: rgba(255,255,255,0.12);
    color: #e8e8ed;
    box-shadow: 0 1px 3px rgba(0,0,0,0.3);
  }
  
  /* Tab scroll */
  .tab-scroll {
    flex: 1;
    overflow-y: auto;
    padding: 28px 40px 120px;
  }
  
  .tab-scroll::-webkit-scrollbar {
    width: 6px;
  }
  
  .tab-scroll::-webkit-scrollbar-track {
    background: transparent;
  }
  
  .tab-scroll::-webkit-scrollbar-thumb {
    background: rgba(255,255,255,0.1);
    border-radius: 3px;
  }
  
  .tab-content {
    /* #2536 / ADR-0063 D8: chat pane no longer sits beside Settings (see +page.svelte,
       which now skips mounting the chat sidebar entirely on the 'settings' view), so
       Settings is a single-column surface. Content measure widens from 600 to 860 and
       stays centred per README-group-b.md §11 ("Settings without the chat pane"). */
    max-width: 860px;
    margin: 0 auto;
  }
  
  /* Section */
  .section {
    margin-bottom: 32px;
  }
  
  .section-title {
    font-size: 12px;
    font-weight: 600;
    color: rgba(255,255,255,0.5);
    text-transform: uppercase;
    letter-spacing: 0.8px;
    margin-bottom: 14px;
  }
  
  .section-card {
    background: rgba(255,255,255,0.05);
    border: 1px solid rgba(255,255,255,0.08);
    border-radius: 12px;
    overflow: hidden;
  }
  
  /* Form rows */
  .form-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 14px 16px;
    border-bottom: 1px solid rgba(255,255,255,0.05);
    min-height: 48px;
  }
  
  .form-row:last-child {
    border-bottom: none;
  }
  
  .form-row-label {
    font-size: 14px;
    color: #e8e8ed;
    font-weight: 400;
  }
  
  .form-row-sublabel {
    font-size: 12px;
    color: rgba(255,255,255,0.3);
    margin-top: 2px;
  }

  /* #2522 design polish batch, item 3: current default agent name + runtime label
     on the AI tab's Default agent row ("Brit Freed · OpenClaw"), replacing the
     sublabel copy only when a default agent is actually loaded. */
  .default-agent-current {
    color: rgba(255,255,255,0.5);
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .default-agent-name {
    color: #e8e8ed;
    font-weight: 500;
  }

  .default-agent-sep {
    color: rgba(255,255,255,0.25);
  }

  .default-agent-runtime {
    color: rgba(255,255,255,0.5);
  }
  
  /* Agent name editing */
  .agent-name-display {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  
  .agent-edit-icon {
    opacity: 0;
    background: none;
    border: none;
    padding: 2px 4px;
    cursor: pointer;
    color: rgba(255,255,255,0.4);
    transition: opacity 0.2s, color 0.2s;
    display: inline-flex;
    align-items: center;
  }
  
  .agent-name-display:hover .agent-edit-icon {
    opacity: 1;
  }
  
  .agent-edit-icon:hover {
    color: #00E4E4;
  }
  
  .agent-name-edit {
    background: transparent;
    border: none;
    border-bottom: 1px solid rgba(255,255,255,0.2);
    padding: 2px 0;
    font-size: 14px;
    color: #e8e8ed;
    font-weight: 400;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', system-ui, sans-serif;
    outline: none;
    min-width: 200px;
  }
  
  .agent-name-edit:focus {
    border-bottom-color: #00E4E4;
  }

  /* #3074: model-edit pencil + inline picker, keyed off .form-row-sublabel
     (the model text's actual container) rather than .agent-name-display —
     the sublabel row isn't wrapped the way the name row is, so the hover
     reveal is scoped to the sublabel directly. */
  .agent-model-edit-icon {
    opacity: 0;
    background: none;
    border: none;
    padding: 2px 4px;
    margin-left: 2px;
    cursor: pointer;
    color: rgba(255,255,255,0.4);
    transition: opacity 0.2s, color 0.2s;
    display: inline-flex;
    align-items: center;
    vertical-align: middle;
  }

  .form-row-sublabel:hover .agent-model-edit-icon {
    opacity: 1;
  }

  .agent-model-edit-icon:hover {
    color: #00E4E4;
  }

  .agent-model-edit-wrap {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    vertical-align: middle;
    width: 260px;
  }

  .agent-model-edit-loading {
    font-size: 12px;
    color: rgba(255,255,255,0.3);
  }

  .agent-model-edit-cancel {
    background: none;
    border: none;
    padding: 2px 4px;
    cursor: pointer;
    color: rgba(255,255,255,0.4);
    font-size: 11px;
    display: inline-flex;
    align-items: center;
    flex-shrink: 0;
  }

  .agent-model-edit-cancel:hover {
    color: #00E4E4;
  }
  
  .openclaw-locked-label {
    font-size: 11px;
    color: rgba(255,255,255,0.4);
    margin-left: 8px;
  }

  /* #2271: Subagent role indicator in OC agents group */
  .subagent-label {
    font-size: 11px;
    color: rgba(255,255,255,0.35);
    margin-left: 6px;
    font-style: italic;
  }

  /* #2030 PR B: Agents tab two-group view styles */
  .section-header-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 14px;
  }

  .section-header-row .section-title {
    margin-bottom: 0;
  }

  .agents-captain-crew-helper {
    font-size: 12px;
    color: var(--fg-3, rgba(255, 255, 255, 0.5));
    margin: 0 0 12px;
  }

  .agents-group-label {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.8px;
    color: rgba(255,255,255,0.4);
    margin-bottom: 8px;
  }

  .agents-group-label--mt {
    margin-top: 20px;
  }

  .agents-main-badge {
    margin-left: 8px;
  }

  /* §24.7C: disabled ≠ hidden — agents outside active provider class get opacity reduction */
  /* Uses class modifier, NOT inline style, per must-fix §12 */
  .agents-row--disabled {
    opacity: 0.5;
    pointer-events: none;
  }

  /* Restore pointer events for reason chip text (read-only) */
  .agents-row--disabled .agents-reason-chip {
    pointer-events: auto;
  }

  .agents-reason-chip {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: 11px;
    color: rgba(255,255,255,0.4);
    margin-top: 3px;
  }

  .form-row-left {
    display: flex;
    flex-direction: column;
    gap: 2px;
    flex: 1;
    min-width: 0;
  }
  
  .form-row-right {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }
  
  /* Inputs */
  .input {
    background: rgba(255,255,255,0.06);
    border: 1px solid rgba(255,255,255,0.08);
    border-radius: 6px;
    padding: 8px 12px;
    font-size: 13px;
    color: #e8e8ed;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', system-ui, sans-serif;
    outline: none;
    transition: border-color 0.2s;
  }
  
  .input:focus {
    border-color: #00E4E4;
  }
  
  .input::placeholder {
    color: rgba(255,255,255,0.3);
  }
  
  .text-input {
    background: rgba(255, 255, 255, 0.06);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 4px;
    color: #e0e0e0;
    padding: 6px 10px;
    font-size: 13px;
    width: 200px;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', system-ui, sans-serif;
    outline: none;
    transition: border-color 0.2s;
  }
  
  .text-input:focus {
    border-color: #00E4E4;
    outline: none;
  }
  
  .text-input::placeholder {
    color: rgba(255, 255, 255, 0.3);
  }
  
  .input-wide {
    width: 100%;
  }
  
  .input-sm {
    width: 80px;
    text-align: center;
  }
  
  .select-input {
    appearance: none;
    -webkit-appearance: none;
    background: rgba(255,255,255,0.06);
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='10' height='6'%3E%3Cpath d='M0 0l5 6 5-6z' fill='rgba(255,255,255,0.4)'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: right 10px center;
    padding: 8px 28px 8px 12px;
    border: 1px solid rgba(255,255,255,0.08);
    border-radius: 6px;
    font-size: 13px;
    color: #e8e8ed;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', system-ui, sans-serif;
    outline: none;
    cursor: pointer;
    transition: border-color 0.2s;
  }
  
  .select-input:focus {
    border-color: #00E4E4;
  }
  
  .password-wrapper {
    position: relative;
    display: flex;
    align-items: center;
    width: 100%;
  }
  
  .password-wrapper .input {
    padding-right: 60px;
    flex: 1;
  }
  
  .password-toggle {
    position: absolute;
    right: 8px;
    background: none;
    border: none;
    color: rgba(255,255,255,0.3);
    cursor: pointer;
    font-size: 12px;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', system-ui, sans-serif;
  }
  
  .password-toggle:hover {
    color: rgba(255,255,255,0.5);
  }
  
  textarea.input {
    resize: vertical;
    min-height: 100px;
    line-height: 1.5;
  }
  
  /* Buttons */
  .btn {
    padding: 7px 16px;
    font-size: 13px;
    font-weight: 500;
    border-radius: 6px;
    border: 1px solid rgba(255,255,255,0.08);
    background: rgba(255,255,255,0.05);
    color: #e8e8ed;
    cursor: pointer;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', system-ui, sans-serif;
    transition: all 0.15s;
    white-space: nowrap;
  }
  
  .btn:hover {
    background: rgba(255,255,255,0.08);
  }
  
  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  
  .btn:focus {
    outline: none;
  }
  
  .btn:focus-visible {
    outline: 2px solid rgba(255,255,255,0.2);
    outline-offset: 2px;
  }
  
  .btn:active {
    transform: scale(0.98);
  }
  
  .btn-primary {
    background: #00E4E4;
    color: #0a0a0f;
    border-color: #00E4E4;
    font-weight: 600;
  }
  
  .btn-primary:hover {
    background: #00E4E4;
    color: #0a0a0f;
    border-color: #00E4E4;
    opacity: 0.9;
  }
  
  .btn-primary:focus {
    background: #00E4E4;
    color: #0a0a0f;
    border-color: #00E4E4;
  }
  
  .btn-primary:focus-visible {
    background: #00E4E4;
    color: #0a0a0f;
    border-color: #00E4E4;
    outline: 2px solid rgba(0,228,228,0.4);
    outline-offset: 2px;
  }
  
  .btn-primary:active {
    background: #00E4E4;
    color: #0a0a0f;
    border-color: #00E4E4;
    opacity: 0.85;
    transform: scale(0.98);
  }
  
  .btn-danger {
    background: rgba(255,68,68,0.1);
    color: #ff4444;
    border-color: rgba(255,68,68,0.2);
  }
  
  .btn-danger:hover {
    background: rgba(255,68,68,0.18);
  }
  
  .btn-sm {
    padding: 5px 10px;
    font-size: 12px;
  }
  
  .btn-icon {
    padding: 4px 8px;
    background: transparent;
    border: none;
    color: rgba(255,255,255,0.3);
    cursor: pointer;
    font-size: 14px;
    border-radius: 4px;
  }
  
  .btn-icon:hover {
    color: rgba(255,255,255,0.5);
    background: rgba(255,255,255,0.05);
  }
  
  .btn-icon.save {
    color: #00E4E4;
  }
  
  .btn-icon.delete:hover {
    color: #ff4444;
    background: rgba(255,68,68,0.1);
  }
  
  /* Toggle switch */
  .toggle {
    width: 42px;
    height: 24px;
    background: rgba(255,255,255,0.15);
    border-radius: 12px;
    position: relative;
    cursor: pointer;
    transition: background 0.2s;
    flex-shrink: 0;
  }
  
  .toggle.on {
    background: #00E4E4;
  }
  
  .toggle.disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }
  
  .toggle::after {
    content: '';
    position: absolute;
    width: 18px;
    height: 18px;
    background: white;
    border-radius: 50%;
    top: 3px;
    left: 3px;
    transition: transform 0.2s;
    box-shadow: 0 1px 3px rgba(0,0,0,0.3);
  }
  
  .toggle.on::after {
    transform: translateX(18px);
  }
  
  /* Status dot */
  .status-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
  }
  
  .status-dot.green {
    background: #22c55e;
    box-shadow: 0 0 6px rgba(34,197,94,0.4);
  }
  
  .status-dot.red {
    background: #ef4444;
    box-shadow: 0 0 6px rgba(239,68,68,0.4);
  }
  
  .status-dot.yellow {
    background: #eab308;
    box-shadow: 0 0 6px rgba(234,179,8,0.4);
  }

  /* #2641 D14 Commit 5: grey variant for the Smart Features status dot — "never
     configured this install", the same neutral state ProviderConnectionCard.svelte's
     .pcc-status--neutral renders for the provider cards. */
  .status-dot.grey {
    background: rgba(255, 255, 255, 0.25);
  }
  

  
  /* Category */
  .cat-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 16px;
    border-bottom: 1px solid rgba(255,255,255,0.05);
  }
  
  .cat-row:last-child {
    border-bottom: none;
  }
  
  .cat-dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    flex-shrink: 0;
  }
  
  .cat-name {
    flex: 1;
    font-size: 14px;
  }
  
  .cat-name-input {
    padding: 6px 10px;
    border: 1px solid rgba(255,255,255,0.1);
    border-radius: 4px;
    background: transparent;
    color: #e0e0e0;
    font-size: 13px;
    font-family: inherit;
    outline: none;
    width: 140px;
  }
  
  .cat-name-input:focus {
    border-color: #00E4E4;
  }
  
  .cat-color-row {
    display: flex;
    gap: 4px;
    align-items: center;
  }
  
  .cat-color-swatch {
    width: 18px;
    height: 18px;
    border-radius: 50%;
    border: 2px solid transparent;
    cursor: pointer;
    padding: 0;
  }
  
  .cat-color-swatch.selected {
    border-color: #e0e0e0;
  }
  
  .cat-color-swatch:hover {
    border-color: rgba(255,255,255,0.4);
  }
  
  /* Color picker swatch */
  .color-swatch {
    width: 24px;
    height: 24px;
    border-radius: 50%;
    border: 2px solid rgba(255,255,255,0.08);
    cursor: pointer;
    overflow: hidden;
    position: relative;
  }
  
  .color-swatch input[type="color"] {
    position: absolute;
    inset: -4px;
    width: 32px;
    height: 32px;
    border: none;
    cursor: pointer;
    opacity: 0;
  }
  
  /* Badge */
  .badge {
    padding: 3px 8px;
    font-size: 11px;
    border-radius: 4px;
    background: rgba(255,255,255,0.05);
    border: 1px solid rgba(255,255,255,0.08);
    color: rgba(255,255,255,0.5);
    font-weight: 500;
  }
  
  .badge-teal {
    background: rgba(0,228,228,0.15);
    color: #00E4E4;
    border-color: rgba(0,228,228,0.2);
  }
  
  /* Agent card */
  .agent-card {
    border-bottom: 1px solid rgba(255,255,255,0.05);
    cursor: pointer;
  }
  
  .agent-card:last-child {
    border-bottom: none;
  }
  
  .agent-header {
    display: flex;
    align-items: center;
    padding: 14px 16px;
    gap: 12px;
    transition: background 0.15s;
  }
  
  .agent-header:hover {
    background: rgba(255,255,255,0.08);
  }
  
  .agent-avatar {
    width: 32px;
    height: 32px;
    border-radius: 50%;
    background: rgba(255,255,255,0.12);
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 13px;
    font-weight: 600;
    color: #00E4E4;
    flex-shrink: 0;
  }
  
  .agent-info {
    flex: 1;
  }
  
  .agent-name {
    font-size: 14px;
    font-weight: 500;
  }
  
  .agent-meta {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  
  .agent-expand {
    padding: 0 16px 16px;
  }
  
  .agent-expand .field-group {
    margin-bottom: 14px;
  }
  
  .agent-expand .field-label {
    font-size: 12px;
    font-weight: 600;
    color: rgba(255,255,255,0.5);
    margin-bottom: 6px;
  }
  
  /* Empty state */
  .empty-state {
    text-align: center;
    padding: 48px 24px;
    color: rgba(255,255,255,0.3);
  }
  
  .empty-state p {
    font-size: 14px;
    line-height: 1.6;
  }
  
  /* Stat row */
  .stat-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 16px;
    border-bottom: 1px solid rgba(255,255,255,0.05);
    font-size: 13px;
  }
  
  .stat-row:last-child {
    border-bottom: none;
  }
  
  .stat-label {
    color: rgba(255,255,255,0.5);
  }
  
  .stat-value {
    color: #e8e8ed;
    font-weight: 500;
    margin-left: auto;
  }
  
  .stat-check {
    color: #34C759;
  }
  
  /* Placeholder box */
  .placeholder-box {
    border: 1px dashed rgba(255,255,255,0.08);
    border-radius: 8px;
    padding: 24px;
    text-align: center;
    color: rgba(255,255,255,0.3);
    font-size: 13px;
  }
  
  /* Path display */
  .path-display {
    font-size: 13px;
    color: rgba(255,255,255,0.5);
    font-family: 'SF Mono', 'Fira Code', monospace;
    background: rgba(255,255,255,0.03);
    padding: 6px 10px;
    border-radius: 4px;
    border: 1px solid rgba(255,255,255,0.05);
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  
  /* Sync history */
  .sync-history-toggle {
    padding: 12px 16px;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: space-between;
    font-size: 13px;
    color: rgba(255,255,255,0.5);
    border-top: 1px solid rgba(255,255,255,0.05);
  }
  
  .sync-history-toggle:hover {
    background: rgba(255,255,255,0.08);
  }
  
  .sync-history-content {
    padding: 0 16px 12px;
  }
  
  .sync-entry {
    display: flex;
    justify-content: space-between;
    padding: 8px 0;
    border-bottom: 1px solid rgba(255,255,255,0.05);
    font-size: 12px;
  }
  
  .sync-entry:last-child {
    border-bottom: none;
  }
  
  .sync-time {
    color: rgba(255,255,255,0.3);
  }
  
  .sync-detail {
    color: rgba(255,255,255,0.5);
  }
  
  /* About section */
  .about-center {
    text-align: center;
    padding: 20px 0;
  }
  
  .about-logo {
    font-size: 28px;
    font-weight: 700;
    letter-spacing: -0.5px;
    color: #e8e8ed;
    margin-bottom: 6px;
  }
  
  .about-version {
    font-size: 13px;
    color: rgba(255,255,255,0.3);
    margin-bottom: 20px;
  }
  
  .update-status {
    margin: 12px auto 8px;
    max-width: 480px;
    color: rgba(255,255,255,0.6);
    font-size: 12px;
    line-height: 1.5;
  }

  .update-status.error {
    color: #ff6b6b;
  }

  .about-links {
    display: flex;
    gap: 20px;
    justify-content: center;
    margin: 24px 0;
    flex-wrap: wrap;
  }
  
  .about-links a {
    color: #00E4E4;
    text-decoration: none;
    font-size: 13px;
  }
  
  .about-links a:hover {
    text-decoration: underline;
  }
  
  .about-made {
    font-size: 13px;
    color: rgba(255,255,255,0.5);
    margin-top: 32px;
  }
  
  .about-legal {
    font-size: 11px;
    color: rgba(255,255,255,0.3);
    margin-top: 8px;
    line-height: 1.5;
  }
  
  /* Messages */
  .error-message {
    font-size: 13px;
    color: #ff4444;
    margin: 12px 16px;
    padding: 12px;
    background: rgba(255,68,68,0.1);
    border: 1px solid rgba(255,68,68,0.2);
    border-radius: 6px;
  }
  
  .success-message {
    font-size: 13px;
    color: #34C759;
    margin: 12px 16px;
    padding: 12px;
    background: rgba(52,199,89,0.1);
    border: 1px solid rgba(52,199,89,0.2);
    border-radius: 6px;
  }
  
  /* Import toast */
  .import-toast {
    position: fixed;
    bottom: 24px;
    right: 24px;
    background: rgba(0,228,228,0.1);
    border: 1px solid rgba(0,228,228,0.3);
    color: #e0e0e0;
    padding: 10px 16px;
    border-radius: 6px;
    font-size: 13px;
    z-index: 9999;
    animation: toastSlideIn 0.3s ease;
  }
  
  @keyframes toastSlideIn {
    from {
      opacity: 0;
      transform: translateY(10px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }
  
  /* Fact browser */
  .fact-filters {
    display: flex;
    gap: 8px;
    padding: 16px;
    border-bottom: 1px solid rgba(255,255,255,0.06);
  }
  
  .fact-list {
    max-height: 600px;
    overflow-y: auto;
  }
  
  .fact-list::-webkit-scrollbar {
    width: 6px;
  }
  
  .fact-list::-webkit-scrollbar-track {
    background: transparent;
  }
  
  .fact-list::-webkit-scrollbar-thumb {
    background: rgba(255,255,255,0.1);
    border-radius: 3px;
  }
  
  .fact-row {
    display: grid;
    grid-template-columns: 1fr 80px 80px 60px;
    align-items: center;
    gap: 8px;
    padding: 14px 16px;
    border-bottom: 1px solid rgba(255,255,255,0.06);
    transition: background 0.15s;
  }
  
  .fact-row:last-child {
    border-bottom: none;
  }
  
  .fact-row:hover {
    background: rgba(255,255,255,0.03);
  }
  
  .fact-content {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  
  .fact-text {
    font-size: 13px;
    color: #e0e0e0;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
    white-space: normal;
    line-height: 1.4;
    flex: 1;
    min-width: 0;
  }
  
  .fact-category-badge {
    background: rgba(0,228,228,0.08);
    color: #00E4E4;
    padding: 1px 6px;
    border-radius: 3px;
    font-size: 11px;
    text-align: center;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  
  .fact-meta-actions {
    display: flex;
    align-items: center;
    gap: 4px;
    justify-content: flex-end;
  }
  
  .fact-time {
    font-size: 11px;
    color: rgba(255,255,255,0.3);
    text-align: right;
    white-space: nowrap;
  }
  
  .fact-edit-container {
    grid-column: 1 / -1;
  }
  
  .fact-edit-textarea {
    width: 100%;
    background: rgba(255,255,255,0.06);
    border: 1px solid rgba(255,255,255,0.08);
    border-radius: 6px;
    padding: 8px 12px;
    font-size: 13px;
    color: #e8e8ed;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', system-ui, sans-serif;
    outline: none;
    resize: vertical;
    line-height: 1.5;
    margin-bottom: 8px;
  }
  
  .fact-edit-textarea:focus {
    border-color: #00E4E4;
  }
  
  .fact-edit-actions {
    display: flex;
    gap: 6px;
    justify-content: flex-end;
  }
  
  .fact-delete-container {
    grid-column: 1 / -1;
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  
  .fact-delete-text {
    font-size: 13px;
    color: #e0e0e0;
  }
  
  .fact-delete-actions {
    display: flex;
    gap: 6px;
  }
  
  .fact-empty {
    padding: 48px 24px;
    text-align: center;
    color: rgba(255,255,255,0.3);
    font-size: 13px;
    line-height: 1.6;
  }
  
  .fact-load-more {
    padding: 12px 16px;
    text-align: center;
    border-top: 1px solid rgba(255,255,255,0.06);
  }
  
  /* Daily Logs Tab */
  .logs-container {
    display: flex;
    gap: 0;
    height: calc(100vh - 180px);
    margin: 0 -16px;
  }

  .logs-list {
    width: 280px;
    min-width: 280px;
    border-right: 1px solid rgba(255,255,255,0.06);
    overflow-y: auto;
    padding: 8px;
  }

  .log-card {
    display: block;
    width: 100%;
    text-align: left;
    padding: 12px;
    margin-bottom: 4px;
    background: rgba(255,255,255,0.05);
    border: 1px solid rgba(255,255,255,0.08);
    border-radius: 8px;
    cursor: pointer;
    color: #e0e0e0;
    font-family: inherit;
  }

  .log-card:hover {
    background: rgba(255,255,255,0.08);
  }

  .log-card.active {
    background: rgba(0,228,228,0.08);
    border-color: rgba(0,228,228,0.2);
  }

  .log-date {
    font-size: 14px;
    font-weight: 600;
    color: #e0e0e0;
  }

  .log-meta {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-top: 2px;
  }

  .log-agent {
    font-size: 12px;
    color: #888;
    margin-top: 0;
  }

  .log-size {
    font-size: 11px;
    color: #666;
    margin-top: 0;
  }

  .log-viewer {
    flex: 1;
    overflow-y: auto;
    padding: 16px 24px;
  }

  .log-viewer-header {
    padding-bottom: 12px;
    margin-bottom: 16px;
    border-bottom: 1px solid rgba(255,255,255,0.06);
  }

  .log-viewer-title {
    font-size: 15px;
    font-weight: 600;
    color: #e0e0e0;
  }

  .log-viewer-content {
    font-size: 14px;
    line-height: 1.6;
    color: rgba(255,255,255,0.8);
  }

  /* Markdown styling in log viewer */
  .log-viewer-content :global(h1),
  .log-viewer-content :global(h2),
  .log-viewer-content :global(h3) {
    color: #e0e0e0;
    margin-top: 20px;
    margin-bottom: 8px;
  }

  .log-viewer-content :global(h2) {
    font-size: 16px;
  }

  .log-viewer-content :global(h3) {
    font-size: 14px;
  }

  .log-viewer-content :global(p) {
    margin-bottom: 8px;
  }

  .log-viewer-content :global(ul),
  .log-viewer-content :global(ol) {
    padding-left: 20px;
    margin-bottom: 8px;
  }

  .log-viewer-content :global(li) {
    margin-bottom: 4px;
  }

  .log-viewer-content :global(code) {
    background: rgba(255,255,255,0.06);
    padding: 2px 5px;
    border-radius: 3px;
    font-size: 13px;
  }

  .log-viewer-content :global(pre) {
    background: rgba(255,255,255,0.04);
    padding: 12px;
    border-radius: 4px;
    overflow-x: auto;
    margin-bottom: 12px;
  }

  .log-viewer-content :global(strong) {
    color: #e0e0e0;
  }

  .log-viewer-empty,
  .logs-empty {
    color: #666;
    font-size: 13px;
    text-align: center;
    padding: 40px 16px;
  }

  /* Health Tab */
  .health-panel {
    padding: 16px;
  }

  .health-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 8px 0;
    font-size: 13px;
  }

  .health-label {
    color: rgba(255,255,255,0.6);
  }

  .health-value {
    color: #e8e8ed;
    font-weight: 500;
  }

  /* Context-window override UI (#1404-E §27.11) */
  .ctx-window-row {
    align-items: flex-start;
  }

  .ctx-window-controls {
    display: flex;
    flex-direction: column;
    gap: 4px;
    align-items: flex-end;
  }

  .ctx-window-input-row {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .ctx-window-input {
    width: 100px;
    background: rgba(255,255,255,0.07);
    border: 1px solid rgba(255,255,255,0.15);
    border-radius: 4px;
    color: #e8e8ed;
    font-size: 13px;
    padding: 3px 6px;
    text-align: right;
  }

  .ctx-window-input:focus {
    outline: none;
    border-color: #00E4E4;
  }

  .ctx-window-unit {
    color: rgba(255,255,255,0.5);
    font-size: 12px;
  }

  .ctx-clear-btn {
    background: rgba(255,255,255,0.07);
    border: 1px solid rgba(255,255,255,0.15);
    border-radius: 4px;
    color: rgba(255,255,255,0.6);
    font-size: 11px;
    padding: 3px 8px;
    cursor: pointer;
  }

  .ctx-clear-btn:hover:not(:disabled) {
    background: rgba(255,255,255,0.12);
    color: #e8e8ed;
  }

  .ctx-clear-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .ctx-error {
    font-size: 11px;
    color: #FF6B6B;
  }

  .ctx-note {
    font-size: 11px;
    color: rgba(255,255,255,0.4);
  }

  .ctx-estimated {
    font-size: 11px;
    color: rgba(255,255,255,0.4);
    margin-left: 4px;
  }

  .budget-tiers {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .tier-row {
    display: grid;
    grid-template-columns: 120px 1fr 120px;
    gap: 12px;
    align-items: center;
    font-size: 12px;
  }

  .tier-label {
    color: rgba(255,255,255,0.7);
    font-weight: 500;
  }

  .tier-bar-container {
    background: rgba(255,255,255,0.06);
    height: 18px;
    border-radius: 4px;
    overflow: hidden;
  }

  .tier-bar {
    height: 100%;
    background: linear-gradient(90deg, #00E4E4, #00B3B3);
    transition: width 0.3s ease;
  }

  .tier-tokens {
    text-align: right;
    color: rgba(255,255,255,0.5);
    font-size: 11px;
  }

  .backups-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .backup-card {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 12px;
    background: rgba(255,255,255,0.03);
    border-radius: 6px;
    border: 1px solid rgba(255,255,255,0.06);
  }

  .backup-info {
    flex: 1;
  }

  .backup-filename {
    font-size: 13px;
    color: #e8e8ed;
    font-weight: 500;
    margin-bottom: 4px;
  }

  .backup-meta {
    font-size: 11px;
    color: rgba(255,255,255,0.4);
  }

  .modal-overlay {
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    bottom: 0;
    background: rgba(0,0,0,0.7);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .modal-content {
    background: #1a1a24;
    border: 1px solid rgba(255,255,255,0.12);
    border-radius: 12px;
    padding: 24px;
    max-width: 400px;
    width: 90%;
  }
  
  /* Slider controls */
  .slider {
    width: 180px;
    height: 4px;
    border-radius: 2px;
    background: rgba(255,255,255,0.1);
    outline: none;
    -webkit-appearance: none;
    appearance: none;
  }
  
  .slider::-webkit-slider-thumb {
    -webkit-appearance: none;
    appearance: none;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: #00E4E4;
    cursor: pointer;
    box-shadow: 0 0 4px rgba(0,228,228,0.4);
    margin-top: -6px; /* Center the thumb on the track */
  }
  
  .slider::-moz-range-thumb {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: #00E4E4;
    cursor: pointer;
    border: none;
    box-shadow: 0 0 4px rgba(0,228,228,0.4);
  }
  
  .slider::-webkit-slider-runnable-track {
    height: 4px;
    background: rgba(255,255,255,0.1);
    border-radius: 2px;
  }
  
  .slider::-moz-range-track {
    height: 4px;
    background: rgba(255,255,255,0.1);
    border-radius: 2px;
  }
  
  .slider-value {
    font-size: 13px;
    color: #e8e8ed;
    min-width: 60px;
    text-align: right;
    font-weight: 500;
  }

  .memory-info-note {
    font-size: 12px;
    color: rgba(255, 255, 255, 0.5);
    padding: 8px 12px;
    background: rgba(255, 255, 255, 0.03);
    border: 1px solid rgba(255, 255, 255, 0.06);
    border-radius: 6px;
    margin-top: 8px;
    line-height: 1.5;
  }
  /* Cascade-UX animations */
  @keyframes spin {
    from { transform: rotate(0deg); }
    to { transform: rotate(360deg); }
  }
  
  .text-link {
    background: transparent;
    border: none;
    padding: 0;
    font-size: 13px;
    font-weight: 500;
    color: #00E4E4;
    cursor: pointer;
    transition: all 150ms ease;
  }
  
  .text-link:hover {
    color: #33EEDD;
    text-decoration: underline;
  }

  /* ==========================================================
     Templates tab (§32.4) — design tokens from templates.css
     ========================================================== */
  .tpl-set-header {
    display: flex;
    align-items: flex-end;
    gap: 14px;
    margin-bottom: 18px;
  }
  .tpl-set-header__txt { flex: 1; min-width: 0; }
  .tpl-set-eyebrow {
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: #00E4E4;
    margin-bottom: 6px;
  }
  .tpl-set-lede { font-size: 13px; color: #A0A0A0; line-height: 1.5; margin: 0; max-width: 520px; }
  .tpl-set-count { font-size: 12.5px; color: #5A5A5E; flex-shrink: 0; white-space: nowrap; }

  .tpl-set-list {
    display: flex;
    flex-direction: column;
    border: 1px solid rgba(240,237,232,0.10);
    border-radius: 11px;
    overflow: hidden;
  }
  .tpl-srow {
    display: flex;
    align-items: center;
    gap: 13px;
    padding: 13px 15px;
    border-top: 1px solid rgba(240,237,232,0.06);
    background: #1A1A1C;
    transition: background 120ms;
  }
  .tpl-srow:first-child { border-top: 0; }
  .tpl-srow:hover { background: #232327; }
  .tpl-srow__ico {
    width: 32px;
    height: 32px;
    border-radius: 6px;
    flex-shrink: 0;
    background: #232327;
    color: #A0A0A0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }
  .tpl-srow:hover .tpl-srow__ico { background: #2C2C30; }
  .tpl-srow__body { flex: 1; min-width: 0; }
  .tpl-srow__name {
    display: block;
    font-size: 14px;
    font-weight: 500;
    color: #F0EDE8;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tpl-srow__meta { display: block; font-size: 12px; color: #5A5A5E; margin-top: 2px; }
  .tpl-srow__actions {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
    opacity: 0;
    transition: opacity 120ms;
  }
  .tpl-srow:hover .tpl-srow__actions { opacity: 1; }
  .tpl-iconbtn {
    width: 30px;
    height: 30px;
    border-radius: 6px;
    border: 0;
    background: transparent;
    cursor: pointer;
    color: #A0A0A0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    transition: background 120ms, color 120ms;
  }
  .tpl-iconbtn:hover { background: rgba(240,237,232,0.06); color: #F0EDE8; }
  .tpl-iconbtn--danger:hover { background: rgba(239,68,68,0.10); color: #EF4444; }
  .tpl-srow__rename {
    flex: 1;
    min-width: 0;
    font-size: 14px;
    font-weight: 500;
    color: #F0EDE8;
    background: #0A0A0F;
    border: 1px solid #00E4E4;
    border-radius: 6px;
    padding: 5px 9px;
    outline: none;
    box-shadow: 0 0 0 3px rgba(0,228,228,0.12);
  }
</style>

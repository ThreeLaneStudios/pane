<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { activeChatThreadId, projects } from '$lib/stores/app';
  
  import { listen } from '@tauri-apps/api/event';
  import { renderMarkdown } from '$lib/utils/markdown';
  import ChatPanel from './ChatPanel.svelte';
  import WorkshopReview from './WorkshopReview.svelte';
  import { streamAndCollect, parseSoulAndMemory, buildUploadExtractionPrompt, buildInterviewExtractionPrompt } from '$lib/onboarding/seedGeneration';
  import { WORKSHOP_SCOPE_ID } from '$lib/constants';
  import { createWorkshopSession } from '$lib/onboarding/workshopSession';
  import SearchableModelSelect from './SearchableModelSelect.svelte';
  import * as api from '$lib/api';
  import { normalizeOcGatewayUrl, runOcPairingExchange, runOcImportPhase, parseGatewayError, type AgentCollision } from '$lib/services/ocPairingCeremony';
  import { normalizeOllamaEndpoint } from '$lib/utils/ollama';
  import { openExternalUrl } from '$lib/utils/externalLink';
  import ConfirmDialog from './ConfirmDialog.svelte';
  
  type Screen = 'restore-offer' | 'connect' | 'ollama-unreachable' | 'ollama-no-models' | 'ollama-custom-endpoint' | 'model-select' | 'main' | 'upload' | 'questions' | 'processing' | 'workshop' | 'openclaw-agents' | 'agent-names' | 'oc-url-entry' | 'oc-verify-connect' | 'oc-sync' | 'smart-features-picker' | 'smart-features-or-setup' | 'smart-features-anthropic-setup' | 'smart-features-venice-setup' | 'smart-features-ollama-setup';
  type StylePreset = 'concise' | 'balanced' | 'detailed' | 'custom';
  type ProcessingMethod = 'upload' | 'questions' | 'chat';

  // #2617: provider-specific labels for the empty-after-filter model-select
  // error message. Mirrors AgentCreateOverlay.svelte's RUNTIME_LABELS —
  // connectProvider at the model-select call site is always one of these four
  // (OpenClaw routes through oc-url-entry/oc-sync and skips this screen).
  // 'venice' added for the onboarding-picker ruling (#2455 venice live-test v2):
  // the provider picker must show every added provider dynamically, Venice
  // included, now and for future runtimes — see the `connect-providers` card
  // list below and this file's provider-enumeration entry-point census comment.
  const PROVIDER_LABELS: Record<string, string> = {
    openrouter: 'OpenRouter',
    anthropic: 'Anthropic',
    venice: 'Venice',
    ollama: 'Ollama',
  };
  
  let screen: Screen = $state('connect');
  let processingMethod: ProcessingMethod = $state('upload');
  let aiConnected = $state(false);
  let connectProvider = $state('openrouter');
  let connectKey = $state('');
  let connectModel = $state('');
  // Model selection state
  let selectedComplexModel = $state('');
  let selectedSimpleModel = $state('');
  let selectedRerankModel = $state('');
  let selectedModelId = $state(''); // Legacy state variable — no hardcoded default (#1322)
  // Primary model picker (onboarding model-select screen)
  let primaryModelList = $state<Array<{id: string; name: string; provider: string}>>([]);
  let primaryModelLoading = $state(false);
  let primaryModelError = $state('');
  let connectEndpoint = $state('http://localhost:11434');
  let connectTesting = $state(false);
  let connectStatus = $state<'idle' | 'success' | 'error'>('idle');
  let connectError = $state('');
  // Ollama-specific screen state
  let ollamaScreenReason = $state<'unreachable' | 'tags_failed' | ''>('');
  
  // OpenClaw state
  let ocGatewayUrl = $state('');
  let ocToken = $state('');
  let ocDetecting = $state(false);
  let ocDetectStatus = $state<'idle' | 'success' | 'error'>('idle');
  let ocDetectError = $state('');
  let ocAgents: { id: string, name: string }[] = $state([]);
  let ocDiscoveredAgents = $state<any[]>([]);
  let ocSelectedAgent = $state('main');
  let ocWorkspacePath = $state('');
  let ocMainAgentName = $state('');
  let ocMainAgentId = $state(''); // Store agent UUID for file operations
  let isOpenClawFlow = $state(false);
  let ocMemoryCount = $state(0);
  let editingAgentNames = $state<Record<string, string>>({});
  let syncError = $state('');
  let syncInProgress = $state(false);
  let syncAbortController: AbortController | null = null;
  // #2676: surfaces failures from ensureMainAgent()/createWorkshopSession() in
  // processUpload/processQuestions, which previously threw as unhandled
  // rejections and stranded the user on the 'processing' screen with no
  // recovery path. Shared across both entry points since processingMethod
  // already disambiguates which one was active (mirrors syncError's shape).
  let processingError = $state('');

  // Smart Features (§6.4) state
  let sfProvider = $state<'' | 'openrouter' | 'anthropic' | 'venice' | 'ollama'>('');
  let sfApiKey = $state('');
  let sfEndpoint = $state('http://localhost:11434');
  let sfTesting = $state(false);
  let sfConnectionStatus = $state<'idle' | 'success' | 'error'>('idle');
  let sfConnectionError = $state('');
  let sfModels: { id: string; name: string; provider: string }[] = $state([]);
  let sfExtractionModel = $state('');
  let sfSmartRoutingModel = $state('');
  let sfRerankModel = $state('');
  let sfStep = $state<'credentials' | 'models'>('credentials');
  let postSmartFeaturesScreen = $state<'main' | 'agent-names'>('main');
  let ocModels: { id: string }[] = $state([]);
  let ocSelectedModel = $state('');
  let ocModelsFetchError = $state('');
  let ocModelsFetching = $state(false);
  // SplitMachine onboarding state (#880)
  let ocUrlInput = $state('');
  let ocFingerprint = $state('');
  let ocCertPem = $state('');
  let ocPairingCode = $state('');
  let ocPairingError = $state('');
  let ocVersionWarning = $state('');
  let ocProbeError = $state('');
  let ocSyncError = $state('');
  // §22C.5.4: structured error code from gateway, used to discriminate recovery messages
  let ocSyncErrorCode = $state<string | null>(null);
  let ocProbing = $state(false);
  let ocPairing = $state(false);
  let ocSyncing = $state(false);
  // D20 (#2649): ConfirmDialog Promise-wrapper for the re-pair confirmation, copying
  // ProviderConnectionCard.svelte's showDisconnectConfirm pattern. Replaces the
  // native confirm() this screen previously passed as confirmCollision — confirm()
  // never renders in Tauri's WebView and silently resolved false, so no re-pair with
  // a collision ever actually reimported.
  let showRepairConfirm = $state(false);
  let repairCollisions = $state<AgentCollision[]>([]);
  let pendingRepairResolve: ((v: boolean) => void) | null = $state(null);
  // D20 (#2649): set when the user explicitly declines a re-pair confirmation.
  // Dismissable notice, NOT the ocSyncError error-banner styling — declining is a
  // user choice, not a failure.
  let ocRepairDeclined = $state(false);
  let stylePreset: StylePreset = $state('concise');
  let customSoul = $state('');
  

  
  // Upload state
  let uploadedFiles: { name: string, content: string }[] = $state([]);
  let isDragging = $state(false);
  
  // Questions state
  let qName = $state('');
  let qDo = $state('');
  let qWorking = $state('');
  let qExtra = $state('');
  // qComm mirrors the Communication Style path-chooser buttons (stylePreset) — the
  // sole source of comm-style preference after #2304 removed the duplicate
  // questions-screen dropdown. Default matches stylePreset's default ('concise').
  let qComm = $state('short');
  
  // Workshop state (rulesContent/rulesNoteId dropped — Rules step removed, #2288 PR 1;
  // chat state (chatMessages/chatInput/isStreaming/etc.) moved into WorkshopReview.svelte,
  // internal-only there)
  let soulContent = $state('');
  let memoryContent = $state('');
  let memoryNoteId = $state('');
  let soulNoteId = $state('');
  // Seeds WorkshopReview's opening assistant line — differs between the upload
  // and questions paths (see WorkshopReview's initialAssistantMessage prop doc).
  let workshopInitialMessage = $state('');
  // #2618: real chat_sessions row id for WorkshopReview's ai_chat_stream call,
  // resolved via createWorkshopSession() right before entering screen='workshop'.
  // See src/lib/onboarding/workshopSession.ts for the full rationale.
  let workshopSessionId = $state('');

  let { onComplete }: { onComplete: () => void } = $props();
  
  // Restore offer state
  let restoreInProgress = $state(false);
  let restoreError = $state('');
  let restoreStatus: 'idle' | 'schema-older' | 'schema-too-new' | 'error' = $state('idle');
  let pendingRestorePath = $state('');
  let restoreSummary: any = $state(null);
  
  // Auto-trigger sync when oc-sync screen is shown. D20 (#2649): also gated on
  // !ocRepairDeclined — without it, a declined re-pair (screen stays 'oc-sync',
  // ocSyncing resets to false, ocSyncError stays empty) would re-trigger runOcSync()
  // in an infinite loop the instant the user dismisses/re-renders the notice.
  $effect(() => {
    if (screen === 'oc-sync' && !ocSyncing && !ocSyncError && !ocRepairDeclined) {
      runOcSync();
    }
  });

  // Check for fresh install on mount (only run once)
  let freshInstallChecked = $state(false);
  $effect(() => {
    if (!freshInstallChecked) {
      freshInstallChecked = true;
      invoke('check_fresh_install').then((isFresh: any) => {
        if (isFresh) {
          screen = 'restore-offer';
        }
      }).catch((e: any) => {
        console.error('[ONBOARD] Failed to check fresh install:', e);
        // On error, continue to connect screen
      });
    }
  });

  // #2289 fix: `concise` used to duplicate DEFAULT_SOUL_FALLBACK (src-tauri/src/ai/commands.rs)
  // as a hardcoded string here — two copies with nothing forcing agreement. Now fetched from
  // the backend on mount so the Rust const is the single source of truth.
  let soulPresets: Record<string, string> = {
    concise: '',
    balanced: "Be helpful and conversational. Give enough detail to be useful but don't over-explain.\nMatch the user's energy -- short question gets a short answer, complex question gets a thorough one. Be natural and friendly without being chatty.",
    detailed: "Be thorough and proactive. Anticipate follow-up questions and address them.\nProvide context, examples, and explanations when helpful.\nThink out loud when working through complex problems.\nBe warm and engaged -- you're a collaborative partner, not just a tool.",
    custom: ''
  };

  // Fetch the authoritative default soul text for the "concise" preset from the backend
  // (see DEFAULT_SOUL_FALLBACK, ai/commands.rs) — only run once on mount.
  let conciseDefaultLoaded = $state(false);
  $effect(() => {
    if (!conciseDefaultLoaded) {
      conciseDefaultLoaded = true;
      invoke('ai_get_default_soul').then((value: any) => {
        soulPresets.concise = value;
      }).catch((e: any) => {
        console.error('[ONBOARD] Failed to fetch default soul preset:', e);
      });
    }
  });

  // Workshop stream-listener $effect, scrollChat(), and hasBeenEdited moved into
  // WorkshopReview.svelte (#2288 PR 1) — the stream listener is what makes the
  // workshop's live doc pane refresh after AI edit_note/append_to_note tool calls;
  // all now internal to the component.


  async function testConnection() {
    connectTesting = true;
    connectStatus = 'idle';
    connectError = '';
    
    try {
      // Key-only validation — no model needed at this step (model is set by picker later)
      if (connectProvider === 'openrouter') {
        await invoke('validate_openrouter_key', { apiKey: connectKey.trim() });
      } else if (connectProvider === 'anthropic') {
        await invoke('validate_anthropic_key', { apiKey: connectKey.trim() });
      } else if (connectProvider === 'venice') {
        // Onboarding-picker ruling (#2455 venice live-test v2): venice was entirely
        // absent from this dispatch even though the provider card below already
        // sets connectProvider = 'venice'.
        await invoke('validate_venice_key', { apiKey: connectKey.trim() });
      } else if (connectProvider === 'ollama') {
        // Probe and branch — probeOllama() handles save on success and screen
        // routing on error. If it returns false the user is already on an error
        // screen; bail out without proceeding.
        const ok = await probeOllama(connectEndpoint.trim());
        if (!ok) return;
      } else {
        throw new Error(`Unknown provider: ${connectProvider}`);
      }
      
      // Non-Ollama providers: save here (Ollama save is inside probeOllama)
      if (connectProvider !== 'ollama') {
        await invoke('ai_configure_and_save', {
          provider: connectProvider,
          apiKey: (connectProvider === 'anthropic' || connectProvider === 'openrouter' || connectProvider === 'venice') ? connectKey.trim() : null,
          model: null,
          endpoint: null,
        });
      }
      
      connectStatus = 'success';
      aiConnected = true;
    } catch (e: any) {
      connectStatus = 'error';
      connectError = e.toString();
    } finally {
      connectTesting = false;
    }
  }
  
  async function startChatting() {
    // Create default Soul from style preset + empty Memory.
    // #2277 Fix 1: no longer also write to the global ai/soul.md + workspace-memory.md —
    // per-agent SOUL.md/MEMORY.md (written below via ensureAgentFile) are the sole source.
    let preset = stylePreset === 'custom' ? customSoul : soulPresets[stylePreset];
    // #2277 fix 2: soulPresets.concise loads async on mount; if startChatting() fires
    // before that resolves, preset would be '' here, writing an empty SOUL.md — the exact
    // empty-skeleton failure this PR fixes. Fetch the authoritative default live instead.
    if (!preset) preset = await invoke('ai_get_default_soul');
    
    // Create main agent + write Soul/Memory files
    try {
      const agent = await ensureMainAgent();
      ocMainAgentName = agent.name; // Store for saveAndComplete
      ocMainAgentId = agent.id; // Store agent UUID for file operations
      if (!agent.existed) {
        await ensureAgentFile(agent.id, 'SOUL.md', preset);
        await ensureAgentFile(agent.id, 'MEMORY.md', 'This document will be updated as your AI learns about you through conversation.');
      }
    } catch (e) { console.error('Failed to create main agent:', e); }
    
    await saveAndComplete('chat');
  }

  /**
   * normalizeOllamaEndpoint is imported from $lib/utils/ollama (shared with Settings).
   * (ride-along refactor, PR fix/1627-1628-settings-ollama-probe)
   */

  /**
   * Shared Ollama probe function. Captures the OllamaProbeResult and branches
   * on error_class to route to the correct screen. Called from testConnection()
   * and from the Try Again buttons on all three Ollama error screens.
   *
   * Returns true on success (caller may proceed to model-select), false otherwise.
   */
  async function probeOllama(endpoint: string): Promise<boolean> {
    const normalized = normalizeOllamaEndpoint(endpoint);
    connectEndpoint = normalized;
    const result: any = await invoke('ollama_probe_two_step', { endpoint: normalized });
    if (result.error_class === null || result.error_class === undefined) {
      // Success — save and return true; caller proceeds to model-select
      await invoke('ai_configure_and_save', {
        provider: 'ollama',
        apiKey: null,
        model: null,
        endpoint: normalized,
      });
      connectStatus = 'success';
      aiConnected = true;
      return true;
    } else if (result.error_class === 'unreachable') {
      ollamaScreenReason = 'unreachable';
      connectStatus = 'idle';
      connectError = '';
      screen = 'ollama-unreachable';
      return false;
    } else if (result.error_class === 'no_models') {
      connectStatus = 'idle';
      connectError = '';
      screen = 'ollama-no-models';
      return false;
    } else if (result.error_class === 'tags_failed') {
      ollamaScreenReason = 'tags_failed';
      connectStatus = 'idle';
      connectError = '';
      screen = 'ollama-custom-endpoint';
      return false;
    } else {
      // Unknown error_class — surface as a raw error string
      connectStatus = 'error';
      connectError = result.error_message || 'Ollama probe failed.';
      return false;
    }
  }


  async function probeGatewayUrl() {
    eprintln('[OC-URL-ENTRY] probeGatewayUrl called with:', ocUrlInput);
    ocProbing = true;
    ocProbeError = '';
    try {
      const normalizedUrl = normalizeOcGatewayUrl(ocUrlInput);
      ocGatewayUrl = normalizedUrl;
      ocUrlInput = normalizedUrl;
      const result: any = await invoke('gateway_fetch_fingerprint', { url: normalizedUrl });
      ocFingerprint = result.tls_fingerprint || '';
      ocCertPem = result.cert_pem || '';

      // The pre-pairing health response is unauthenticated. Its metadata must
      // never change the canonical origin the user reviewed.
      screen = 'oc-verify-connect';
    } catch (e: any) {
      const msg = e?.message || String(e);
      if (msg.includes('Cannot reach')) {
        ocProbeError = "Can't reach that address. Check the URL and make sure Pane Gateway and Tailscale are running.";
      } else if (msg.includes('Invalid response') || msg.includes('No tls_fingerprint') || msg.includes('No tls_cert_pem')) {
        ocProbeError = "Something responded but it doesn't look like a Pane Gateway.";
      } else {
        ocProbeError = msg;
      }
    } finally {
      ocProbing = false;
    }
  }

  /**
   * #2649 (PR 3A): pairing-code exchange delegates to the shared
   * `runOcPairingExchange` (phase 1 of the shared ceremony — see
   * `$lib/services/ocPairingCeremony.ts` module doc). Kept as its own step
   * (rather than the combined `runOcPairingCeremony`) because the pairing
   * code is one-time-use: `runOcSync`'s Retry button below re-runs only
   * phase 2 (`runOcImportPhase`) against the already-obtained `ocToken`, and
   * must never re-submit a consumed code.
   */
  async function exchangePairingCode() {
    eprintln('[OC-VERIFY] exchangePairingCode called');
    ocPairing = true;
    ocPairingError = '';
    ocVersionWarning = '';
    try {
      const outcome = await runOcPairingExchange({
        url: ocGatewayUrl,
        code: ocPairingCode,
        fingerprint: ocFingerprint,
        certPem: ocCertPem,
      });
      if (!outcome.ok) {
        ocPairingError = outcome.errorMessage;
        return;
      }
      ocToken = outcome.token;
      // Surface non-blocking version warning if gateway is below MIN_GATEWAY_VERSION
      if (outcome.versionWarning) {
        ocVersionWarning = outcome.versionWarning;
      }
      screen = 'oc-sync';
    } finally {
      ocPairing = false;
    }
  }

  /**
   * #2649 (PR 3A): first-pair/re-pair import routing now goes through the
   * shared `runOcImportPhase` (phase 2 of the shared ceremony) instead of
   * always calling `sync_initial_import` — Onboarding previously had no
   * re-pair path at all (D19's root cause: a truly-fresh install could reach
   * `ensureMainAgent` before any import ran, minting a `runtime_ref=NULL` row
   * that never collides with the import's `runtime_ref='main'` row). Routing
   * through `oc_agent_row_exists` first, same as Settings, eliminates that by
   * construction. Confirmation is rendered via `ConfirmDialog.svelte` (D20,
   * #2649) — the native `confirm()` this previously used never renders in
   * Tauri's WebView (silently resolves `false`), so no re-pair with a
   * detected collision ever actually reimported.
   *
   * The redundant second `ai_configure_and_save` call and `fetchOcModels()`
   * call that used to run here after sync were removed — `ai_configure_and_save`
   * already ran once inside phase 1 (`runOcPairingExchange`) with the real
   * token, and re-calling it here with a possibly-stale `ocSelectedModel`
   * duplicated that write for no benefit (`fetchOcModels()`'s only consumer,
   * the model value, was already redundant per the #882 TODO this replaces).
   */
  async function runOcSync() {
    eprintln('[OC-SYNC] runOcSync called with gatewayUrl:', ocGatewayUrl);
    ocSyncing = true;
    ocSyncError = '';
    ocSyncErrorCode = null;
    ocRepairDeclined = false;
    try {
      // §22C.5.4: oc_handshake_timeout fast-fails after ~10 s (one attempt only).
      // That is safely under the 15 s timeout, so Promise.race remains — same
      // safety net the pre-shared-module version of this function had. This
      // wrapper is Onboarding-specific (not part of the shared module): Settings'
      // `completePairing()` has no equivalent timeout and this PR doesn't add one
      // there, since that's a pre-existing behavioral difference between the two
      // callers, not something the ceremony extraction should change.
      const importPromise = runOcImportPhase({
        url: ocGatewayUrl,
        token: ocToken,
        // D20 (#2649): ConfirmDialog Promise-wrapper, replacing the native confirm()
        // that never rendered in Tauri's WebView. Called unconditionally on every
        // re-pair now, including with an empty collisions array.
        confirmCollision: (collisions: AgentCollision[]) =>
          new Promise<boolean>((resolve) => {
            repairCollisions = collisions;
            pendingRepairResolve = resolve;
            showRepairConfirm = true;
          }),
      });
      const timeoutPromise = new Promise<never>((_, reject) => {
        setTimeout(() => reject(new Error('Gateway sync timeout')), 15000);
      });
      const outcome = await Promise.race([importPromise, timeoutPromise]);
      if (!outcome.ok) {
        ocSyncErrorCode = outcome.errorCode;
        ocSyncError = outcome.errorMessage;
        eprintln('[OC-SYNC] Sync failed (code=' + outcome.errorCode + '):', outcome.errorMessage);
        return;
      }
      // D20 (#2649): a declined re-pair is `ok: true` but structurally distinct
      // from a successful reimport — render a dismissable notice, not the
      // ocSyncError error-banner styling. This is a user choice, not a failure.
      if (outcome.declined) {
        ocRepairDeclined = true;
        return;
      }
      // get_agents → ensureMainAgent → agent-names
      const agentsAfterSync: any[] = await invoke('get_agents') as any[];
      ocDiscoveredAgents = agentsAfterSync || [];
      const agent = await ensureMainAgent();
      ocMainAgentName = agent.name;
      ocMainAgentId = agent.id;
      isOpenClawFlow = true;
      screen = 'agent-names';
    } catch (e: any) {
      // Only reachable via the timeoutPromise rejection above — runOcImportPhase
      // itself never throws (it returns a discriminated-union result).
      const rawMsg = e?.message || String(e);
      const { code, message } = parseGatewayError(rawMsg);
      ocSyncErrorCode = code;
      ocSyncError = message;
      eprintln('[OC-SYNC] Sync failed (code=' + code + '):', message);
    } finally {
      ocSyncing = false;
    }
  }

  function eprintln(msg: string, ...args: any[]) {
    console.error(msg, ...args);
  }

  /** D20 (#2649): confirm/cancel handlers for the re-pair ConfirmDialog, resolving
   *  the pending Promise `confirmCollision` (passed into runOcImportPhase above) is
   *  awaiting on. Mirrors ProviderConnectionCard.svelte's confirmRepair/cancelRepair. */
  function confirmRepair() {
    showRepairConfirm = false;
    pendingRepairResolve?.(true);
    pendingRepairResolve = null;
  }

  function cancelRepair() {
    showRepairConfirm = false;
    pendingRepairResolve?.(false);
    pendingRepairResolve = null;
  }

  function dismissRepairDeclinedNotice() {
    ocRepairDeclined = false;
  }

  // TODO (#882): fetchOcModels() is still called from the oc-sync inline path (multiple
  // call sites). The model-picker screen that previously consumed this result is now removed;
  // the model value is only used as a fallback before ai_configure_and_save calls
  // fetch_oc_agent_model (Rust). Consider removing when the oc-sync path is refactored
  // to pass model=null and let Cluster 1 always-probe handle resolution end-to-end.
  async function fetchOcModels() {
    ocModelsFetching = true;
    ocModelsFetchError = '';
    try {
      // Use Tauri command to bypass ATS restrictions on HTTP requests
      const modelIds = await invoke('fetch_gateway_models', {
        url: ocGatewayUrl.trim(),
        token: ocToken.trim()
      }) as string[];
      
      // Convert to object format expected by UI
      ocModels = modelIds.map(id => ({ id }));
      
      // Pre-select model: find 'default' in id, else 'main' in id, else first item
      const defaultModel = ocModels.find(m => m.id.toLowerCase().includes('default'));
      const mainModel = ocModels.find(m => m.id.toLowerCase().includes('main'));
      ocSelectedModel = defaultModel?.id || mainModel?.id || ocModels[0]?.id || '';
      console.log('[ONBOARD] Fetched models:', ocModels.length, 'Pre-selected:', ocSelectedModel);
    } catch (e: any) {
      console.error('[ONBOARD] Failed to fetch models:', e);
      ocModelsFetchError = e.message || String(e);
      // On fetch failure, allow manual input
    } finally {
      ocModelsFetching = false;
    }
  }

  async function proceedFromAgentNames() {
    // Save any renamed agents
    for (const agent of ocDiscoveredAgents) {
      const newName = editingAgentNames[agent.id];
      if (newName && newName.trim() && newName !== agent.name) {
        try {
          await invoke('update_agent', {
            request: {
              agent_id: agent.id,
              name: newName.trim()
            }
          });
          console.log(`[ONBOARD] Renamed agent ${agent.name} to ${newName}`);
        } catch (e) {
          console.error(`[ONBOARD] Failed to rename agent ${agent.name}:`, e);
        }
      }
    }
    
    // Complete onboarding
    await saveAndComplete('chat');
  }
  
    async function skipConnect() {
    // Cancel any pending sync operation
    if (syncInProgress && syncAbortController) {
      console.log('[ONBOARD] Cancelling pending sync operation');
      syncAbortController.abort();
    }
    
    // Create default agent for local-only use
    try {
      const agent = await ensureMainAgent();
      ocMainAgentName = agent.name;
      ocMainAgentId = agent.id;
      console.log('[ONBOARD] Default agent created for local-only mode:', agent.name);
    } catch (e) {
      console.error('[ONBOARD] Failed to create default agent:', e);
    }
    
    // Navigate to main screen to complete onboarding (agent naming + style)
    screen = 'main';
  }
  
  async function proceedFromConnect() {
    // Universal Agent Discovery (§6.1): Check for existing agents BEFORE wizard
    try {
      const agents: any[] = await invoke('get_agents');
      if (agents && agents.length > 0) {
        console.log('[ONBOARD] Found existing agents:', agents.length);
        ocDiscoveredAgents = agents;
        const mainAgent = agents.find((a: any) => a.is_default) || agents[0];
        ocMainAgentName = mainAgent.name || mainAgent.workspace_path || 'My Agent';
        ocMainAgentId = mainAgent.id;
        
        // For non-OpenClaw providers, all agents are renamable
        isOpenClawFlow = connectProvider === 'openclaw';
        
        // Skip wizard, go to agent-names screen
        screen = 'agent-names';
        return;
      }
    } catch (e) {
      console.log('[ONBOARD] No agents found, proceeding to wizard:', e);
    }
    
    // No agents exist — proceed to model selection (required for all providers)
    screen = 'model-select';
    fetchPrimaryModels();
  }

  /**
   * Gotcha 3: re-probe connectProvider's own reachability when fetchPrimaryModels()
   * sees zero models after filtering. Reuses the exact same live-probe commands
   * testConnection()/probeOllama() already called with these same credentials
   * (connectKey / connectEndpoint) moments earlier in this screen flow — no new
   * network contract, just re-running the known-good check to distinguish
   * "connected, zero models" from "this provider's fetch is failing silently
   * behind fetch_available_models' partial-success merge."
   * Returns true when connectProvider itself is reachable right now (so the
   * empty result reflects a genuine zero-models state), false when it is not
   * (so the empty result is actually a hidden fetch failure for this provider).
   */
  async function probeConnectProviderReachable(): Promise<boolean> {
    try {
      if (connectProvider === 'anthropic') {
        await invoke('validate_anthropic_key', { apiKey: connectKey.trim() });
        return true;
      } else if (connectProvider === 'openrouter') {
        await invoke('validate_openrouter_key', { apiKey: connectKey.trim() });
        return true;
      } else if (connectProvider === 'venice') {
        await invoke('validate_venice_key', { apiKey: connectKey.trim() });
        return true;
      } else if (connectProvider === 'ollama') {
        const result: any = await invoke('ollama_probe_two_step', { endpoint: connectEndpoint.trim() });
        // null/undefined error_class = reachable. 'no_models' also means reachable
        // (Ollama responded, just has zero models pulled) — that's case 1, not
        // case 2. 'unreachable'/'tags_failed' mean the provider itself couldn't
        // be reached or queried — case 2.
        return result.error_class === null || result.error_class === undefined || result.error_class === 'no_models';
      }
      // Unknown provider — treat as unreachable rather than silently claiming success.
      return false;
    } catch (e) {
      console.error('[ONBOARD] Reachability re-probe failed for', connectProvider, e);
      return false;
    }
  }

  // #2617 fix: fetch_primary_provider_models derived the provider from ai_agents
  // rows via derive_active_provider_label, which don't exist yet during onboarding
  // (chicken-and-egg — model selection happens before agent creation, so a fresh
  // install has zero ai_agents rows and every call errored with "No primary
  // provider configured", blocking onboarding entirely). fetch_available_models
  // derives connected runtimes per-runtime from credentials directly
  // (connected_runtime_providers/build_provider_for_runtime_type), so it works
  // pre-agent. Same pattern AgentCreateOverlay.svelte's loadModelsForRuntime()
  // already uses — filter the merged, provider-namespaced response down to the
  // provider the user just connected (connectProvider is always one of
  // openrouter|anthropic|venice|ollama at this call site; OpenClaw skips this screen).
  async function fetchPrimaryModels() {
    primaryModelLoading = true;
    primaryModelError = '';
    primaryModelList = [];
    try {
      const models = await invoke<Array<{id: string; name: string; provider: string}>>('fetch_available_models');
      primaryModelList = models.filter((m) => m.provider === connectProvider);
      if (primaryModelList.length === 0) {
        // Empty-after-filter has two distinct causes (gotcha 3), and they need
        // distinct copy:
        //  1. connectProvider IS connected/reachable but genuinely has zero
        //     models right now (e.g. an empty Ollama instance, unusual account) —
        //     "No models available for X."
        //  2. connectProvider's OWN fetch failed silently. fetch_available_models
        //     (commands.rs) only returns Err when EVERY connected runtime's fetch
        //     fails (`all_models.is_empty() && !errors.is_empty()`) — a partial
        //     failure where connectProvider errors but another connected runtime
        //     (e.g. a leftover Anthropic credential) succeeds still resolves Ok,
        //     with connectProvider's per-runtime error silently dropped and zero
        //     entries for it in the merged list — identical in shape to case 1.
        //     Needs a "provider unreachable" message, not "no models."
        // The merged ModelInfo array alone can't tell these apart (checking
        // `models.some(m => m.provider === connectProvider)` is a tautological
        // no-op here — it's the exact predicate that produced the empty filter
        // above, so it's always false at this point). The only real signal is a
        // live re-probe of connectProvider itself, using the same validate_*_key /
        // ollama_probe_two_step commands testConnection()/probeOllama() already
        // called moments ago with the same credentials.
        const reachable = await probeConnectProviderReachable();
        if (reachable) {
          primaryModelError = `No models available for ${PROVIDER_LABELS[connectProvider] ?? connectProvider}.`;
        } else {
          primaryModelError = `Could not reach ${PROVIDER_LABELS[connectProvider] ?? connectProvider}. Check your connection and try again.`;
        }
      }
      // No auto-select — user types to filter with the searchable input
    } catch (e: any) {
      console.error('[ONBOARD] Failed to fetch primary models:', e);
      primaryModelError = typeof e === 'string' ? e : (e?.message || 'Failed to fetch models');
    } finally {
      primaryModelLoading = false;
    }
  }

  async function proceedFromModelSelect() {
    // Save primary model only (Simple/Rerank are Smart Features — set in §6.4)
    // #2629 (D7 fix): the ai_settings.model write here was a zombie key — dispatch's
    // resolve_runtime_config_for_agent_id reads ai_agents.model only, never
    // ai_settings.model (see docs/spec note + Reed's audit). Removed; connectModel
    // is threaded into upsert_agent's CreateAgentRequest.model in ensureMainAgent()
    // instead, which is the unambiguous per-agent target dispatch actually reads.
    if (selectedComplexModel) {
      connectModel = selectedComplexModel;
    }
    
    // Re-configure runtime state with selected model
    await invoke('ai_configure_and_save', {
      provider: connectProvider,
      apiKey: connectKey || null,
      model: selectedComplexModel || null,
      endpoint: connectProvider === 'ollama' ? connectEndpoint : null,
      simpleTierModel: null,
    });
    
    // Universal Agent Discovery (§6.1): Check for existing agents BEFORE wizard
    try {
      const agents: any[] = await invoke('get_agents');
      if (agents && agents.length > 0) {
        console.log('[ONBOARD] Found existing agents after model selection:', agents.length);
        ocDiscoveredAgents = agents;
        const mainAgent = agents.find((a: any) => a.is_default) || agents[0];
        ocMainAgentName = mainAgent.name || mainAgent.workspace_path || 'My Agent';
        ocMainAgentId = mainAgent.id;
        
        // For non-OpenClaw providers, all agents are renamable
        isOpenClawFlow = connectProvider === 'openclaw';
        
        // Skip wizard, go to agent-names screen — but route through Smart Features first
        postSmartFeaturesScreen = 'agent-names';
        screen = 'smart-features-picker';
        return;
      }
    } catch (e) {
      console.log('[ONBOARD] No agents found after model selection, proceeding to wizard:', e);
    }
    
    postSmartFeaturesScreen = 'main';
    screen = 'smart-features-picker';
  }

  function startSmartFeaturesFlow(provider: 'openrouter' | 'anthropic' | 'venice' | 'ollama') {
    // Reset state — no caching across path switches per §6.4.1
    sfApiKey = '';
    sfEndpoint = 'http://localhost:11434';
    sfTesting = false;
    sfConnectionStatus = 'idle';
    sfConnectionError = '';
    sfModels = [];
    sfExtractionModel = '';
    sfSmartRoutingModel = '';
    sfRerankModel = '';
    sfStep = 'credentials';
    sfProvider = provider;

    if (provider === 'openrouter') screen = 'smart-features-or-setup';
    else if (provider === 'anthropic') screen = 'smart-features-anthropic-setup';
    else if (provider === 'venice') screen = 'smart-features-venice-setup';
    else if (provider === 'ollama') screen = 'smart-features-ollama-setup';
  }

  function skipSmartFeatures() {
    // No state writes needed — internal_provider stays null
    screen = postSmartFeaturesScreen;
  }

  async function submitSmartFeatures() {
    // internal_provider and internal_api_key/internal_endpoint already written during Test Connection
    // Now write the model selections
    await invoke('set_ai_setting', { key: 'extraction_model', value: sfExtractionModel });
    if (sfSmartRoutingModel) {
      await invoke('set_ai_setting', { key: 'simple_tier_model', value: sfSmartRoutingModel });
    }
    if (sfRerankModel) {
      await invoke('set_ai_setting', { key: 'memory_search_llm_rerank_model', value: sfRerankModel });
    }
    // Advance to the stored post-§6.4 destination
    screen = postSmartFeaturesScreen;
  }

  async function sfTestConnection() {
    eprintln('[SF] Testing connection for provider:', sfProvider);
    sfTesting = true;
    sfConnectionStatus = 'idle';
    sfConnectionError = '';
    try {
      if (sfProvider === 'openrouter') {
        await invoke('validate_openrouter_key', { apiKey: sfApiKey });
        // Write credentials
        await invoke('set_ai_setting', { key: 'internal_provider', value: 'openrouter' });
        await invoke('set_ai_setting', { key: 'internal_api_key', value: sfApiKey });
        // Fetch models
        const models: any[] = await invoke('fetch_internal_provider_models');
        sfModels = models.map((m: any) => ({ id: m.id, name: m.name || m.id, provider: m.provider || '' }));
        sfConnectionStatus = 'success';
        sfStep = 'models';
      } else if (sfProvider === 'anthropic') {
        await invoke('validate_anthropic_key', { apiKey: sfApiKey });
        // Write credentials
        await invoke('set_ai_setting', { key: 'internal_provider', value: 'anthropic' });
        await invoke('set_ai_setting', { key: 'internal_api_key', value: sfApiKey });
        // Fetch models
        const models: any[] = await invoke('fetch_internal_provider_models');
        sfModels = models.map((m: any) => ({ id: m.id, name: m.name || m.id, provider: m.provider || '' }));
        sfConnectionStatus = 'success';
        sfStep = 'models';
      } else if (sfProvider === 'venice') {
        // #2795 CRITICAL: Venice is keychain-only (D4, account `ai_venice_key`) —
        // save via `save_venice_key`, NOT `set_ai_setting('internal_api_key', ...)`
        // (the shared OR/Anthropic slot). Getting this wrong would either break
        // Venice (wrong keychain account) or leak a key into the shared slot.
        await invoke('validate_venice_key', { apiKey: sfApiKey });
        // Write credentials
        await invoke('set_ai_setting', { key: 'internal_provider', value: 'venice' });
        await invoke('save_venice_key', { key: sfApiKey });
        // Fetch models
        const models: any[] = await invoke('fetch_internal_provider_models');
        sfModels = models.map((m: any) => ({ id: m.id, name: m.name || m.id, provider: m.provider || '' }));
        sfConnectionStatus = 'success';
        sfStep = 'models';
      } else if (sfProvider === 'ollama') {
        const result: any = await invoke('ollama_probe_two_step', { endpoint: sfEndpoint });
        if (result.error_class === null || result.error_class === undefined) {
          // Success
          await invoke('set_ai_setting', { key: 'internal_provider', value: 'ollama' });
          await invoke('set_ai_setting', { key: 'internal_endpoint', value: sfEndpoint });
          sfModels = (result.models || []).map((m: any) => ({ id: m.id || m.name || m, name: m.name || m.id || m, provider: 'ollama' }));
          sfConnectionStatus = 'success';
          sfStep = 'models';
        } else {
          sfConnectionStatus = 'error';
          if (result.error_class === 'unreachable') {
            sfConnectionError = `Couldn't find Ollama at ${sfEndpoint}. Make sure Ollama is running.`;
          } else if (result.error_class === 'no_models') {
            sfConnectionError = 'Ollama is running but no models are installed. Run `ollama pull <model>` in Terminal — for example `ollama pull llama3.2:3b` — then retry.';
          } else if (result.error_class === 'tags_failed') {
            sfConnectionError = "Couldn't list models. Try again or check the Ollama logs.";
          } else {
            sfConnectionError = result.error_message || 'Connection failed.';
          }
        }
      }
    } catch (e: any) {
      sfConnectionStatus = 'error';
      sfConnectionError = e?.message || e?.toString() || 'Connection failed.';
    } finally {
      sfTesting = false;
    }
  }

  async function ensureMainAgent(): Promise<{ id: string; name: string; existed: boolean }> {
    // Check DB first
    try {
      const mainAgent: any = await invoke('get_main_agent');
      if (mainAgent) {
        console.log('[ONBOARD] Main agent already exists:', mainAgent);
        return { 
          id: mainAgent.id, 
          name: mainAgent.name || mainAgent.workspace_path || 'My Agent', 
          existed: true 
        };
      }
    } catch (e) {
      console.log('[ONBOARD] No main agent in DB, checking filesystem');
    }
    
    // Check filesystem — even without DB record, files may exist (DB reset scenario)
    // Use invoke to check if .pane/system/ has any agent folders with SOUL.md
    let existingName = ocMainAgentName || 'My Agent';
    try {
      const agents: any[] = await invoke('get_agents');
      if (agents && agents.length > 0) {
        // Agents were just discovered by integrity check but ensureMainAgent ran before DB was populated
        const main = agents.find((a: any) => a.is_default) || agents[0];
        existingName = main.name || main.workspace_path || existingName;
        console.log('[ONBOARD] Found existing agent from list_agents:', existingName);
        return { id: main.id, name: existingName, existed: true };
      }
    } catch (e) {
      console.log('[ONBOARD] list_agents not available, proceeding with creation');
    }
    
    // Truly fresh install — create the agent
    // Fallback chain: provided name → system username → "My Agent"
    let agentName = ocMainAgentName?.trim() || '';
    if (!agentName) {
      try {
        agentName = await invoke('get_system_username');
      } catch (e) {
        console.log('[ONBOARD] Failed to get system username:', e);
        agentName = 'My Agent';
      }
    }
    
    // #2629 (D7 fix): pass the user's actual provider choice (connectProvider) and
    // chosen model (connectModel, falling back to selectedComplexModel in case the
    // model-select screen never ran connectModel = selectedComplexModel, e.g. the
    // Ollama/OpenClaw flows that skip model-select) explicitly — both are already
    // in scope at every call site (startChatting, processUpload, processQuestions).
    // Backend's upsert_agent uses these directly on INSERT instead of guessing
    // runtime_type from ambient keychain state (which previously produced the wrong
    // provider whenever a leftover credential from a prior install was present, D8
    // #2631) and instead of leaving model NULL (the model was previously only ever
    // written to the retired ai_settings.model global key, never read by dispatch).
    const agent: any = await invoke('upsert_agent', {
      request: {
        name: agentName,
        role: 'main',
        is_default: true,
        source: 'onboarding',
        runtime_type: connectProvider,
        model: connectModel || selectedComplexModel || null,
      }
    });
    
    console.log('[ONBOARD] Created main agent:', agent);
    return { id: agent.id, name: agentName, existed: false };
  }

  async function ensureAgentFile(agentId: string, filename: string, content: string): Promise<void> {
    // Write identity file to .pane/system/{agent}/
    // #2277 Fix 1: no longer mirrors to the global ai_set_soul/ai_set_workspace_memory
    // commands (removed — they wrote the global .pane/ai/soul.md and workspace-memory.md
    // files that contaminated every agent's prompt). This per-agent file is the sole source.
    await invoke('write_system_file', {
      filename,
      content,
      agentId: agentId,
    });
    console.log(`[ONBOARD] Wrote ${filename} for agent ${agentId}`);
  }

  async function ensureNote(projectId: string, title: string, content: string): Promise<string> {
    // Check if note exists
    const notes: any[] = await invoke('get_notes', { projectId, folderId: null, includeDeleted: false });
    let note = notes.find((n: any) => n.title === title);
    
    if (note) {
      // Update existing
      await invoke('update_note', { request: { note_id: note.id, content } });
      return note.id;
    } else {
      // Create new
      const created: any = await invoke('create_note', { 
        request: { title, project_id: projectId, content, status: 'cabinet' }
      });
      return created.id;
    }
  }

  async function processUpload() {
    processingMethod = 'upload';
    screen = 'processing';
    const rawCombined = uploadedFiles.map(f => `--- ${f.name} ---\n${f.content}`).join('\n\n');
    // Truncate to ~20K chars to stay within context limits
    const combined = rawCombined.length > 20000 
      ? rawCombined.substring(0, 20000) + '\n\n[Documents truncated for processing — full versions saved separately]'
      : rawCombined;
    const preset = stylePreset === 'custom' ? customSoul : soulPresets[stylePreset];
    
    const extractionPrompt = buildUploadExtractionPrompt(combined, stylePreset, preset);

    // #2676: ensureMainAgent()/createWorkshopSession() (and everything that
    // depends on `agent` below) are wrapped in one outer try/catch. Previously
    // these were bare awaits — a throw here was an unhandled rejection that left
    // `screen` stuck on 'processing' with no error message and no Back button.
    processingError = '';
    try {
      // #2634 (D10 fix): create the agent BEFORE invoking synthesis. ai_chat_stream's
      // session/agent resolution (resolve_and_selfheal_session_agent, agents.rs) needs
      // at least one ai_agents row to exist — calling streamAndCollect() first (the
      // prior order) hit a hard "No agent is set up yet" Err on every fresh install,
      // which the catch block below then mislabeled as "Processing took too long."
      // See docs/audits/2026-08-21-d10-upload-synthesis-ordering.md and the
      // "Agent-before-synthesis ordering invariant" note in docs/spec/onboarding.md.
      const agent = await ensureMainAgent();
      ocMainAgentName = agent.name; // Store for saveAndComplete
      ocMainAgentId = agent.id; // Store agent UUID for file operations

      // D26/PR-B (#2673): createWorkshopSession MUST run before streamAndCollect,
      // not after — streamAndCollect now requires a real, agent-bound sessionId to
      // dispatch synthesis through the correct runtime (previously fell back to
      // whatever session happened to be active, dispatching through the WRONG
      // agent's runtime per live-log repro in #2673). agent was just created above
      // by ensureMainAgent(), so no chat_sessions row bound to it exists yet.
      workshopSessionId = await createWorkshopSession(agent.id);

      try {
        const response = await streamAndCollect(extractionPrompt, workshopSessionId);
        const parsed = parseSoulAndMemory(response, preset);
        soulContent = parsed.soul;
        memoryContent = parsed.memory;
      } catch (e) {
        const errMsg = e instanceof Error ? e.message : String(e);
        console.error('[ONBOARD] Extraction FAILED:', e, typeof e, JSON.stringify(e));
        // If we got partial content, use it
        soulContent = soulContent || preset;
        memoryContent = memoryContent || `(Synthesis failed: ${errMsg}. Ask me to create your memory in the chat below.)`;
      }

      // Write files (agent was already created above, before synthesis ran)
      if (!agent.existed) {
        await ensureAgentFile(agent.id, 'MEMORY.md', memoryContent);
        await ensureAgentFile(agent.id, 'SOUL.md', soulContent);
      }

      workshopInitialMessage = "This is your **Memory** -- the facts, context, and details I'll keep in mind whenever we talk. Take a look and let me know if anything needs adjusting.";

      memoryNoteId = await ensureNote(WORKSHOP_SCOPE_ID, 'Memory', memoryContent || '');
      soulNoteId = await ensureNote(WORKSHOP_SCOPE_ID, 'Soul', soulContent || '');
      screen = 'workshop';
    } catch (e) {
      console.error('[ONBOARD] processUpload FAILED:', e);
      processingError = e instanceof Error ? e.message : String(e);
      screen = 'upload';
      return;
    }
  }

  async function processQuestions() {
    processingMethod = 'questions';
    screen = 'processing';
    
    // Use comm preference to select soul preset
    const commToPreset: Record<string, string> = {
      short: 'concise',
      balanced: 'balanced', 
      detailed: 'detailed'
    };
    const presetKey = commToPreset[qComm] || 'balanced';
    const preset = soulPresets[presetKey as keyof typeof soulPresets];
    
    let answers = '';
    if (qName) answers += `Name: ${qName}\n`;
    if (qDo) answers += `What they do and what matters: ${qDo}\n`;
    if (qWorking) answers += `Currently working on: ${qWorking}\n`;
    if (qExtra) answers += `Additional context: ${qExtra}\n`;
    
    if (!answers.trim()) { await startChatting(); return; }

    const extractionPrompt = buildInterviewExtractionPrompt(answers, qComm, preset);

    // #2676: same outer try/catch treatment as processUpload — ensureMainAgent()/
    // createWorkshopSession() and everything downstream of `agent` are wrapped so
    // a throw doesn't strand the user on the 'processing' screen.
    processingError = '';
    try {
      // #2634 (D10 fix): same ordering fix as processUpload — the agent must exist
      // before streamAndCollect()/ai_chat_stream is invoked. See the comment in
      // processUpload() above and docs/spec/onboarding.md's ordering invariant note.
      const agent = await ensureMainAgent();
      ocMainAgentName = agent.name; // Store for saveAndComplete
      ocMainAgentId = agent.id; // Store agent UUID for file operations

      // D26/PR-B (#2673): createWorkshopSession MUST run before streamAndCollect —
      // see comment in processUpload() above for the full rationale.
      workshopSessionId = await createWorkshopSession(agent.id);

      try {
        const response = await streamAndCollect(extractionPrompt, workshopSessionId);
        const parsed = parseSoulAndMemory(response, preset);
        soulContent = parsed.soul;
        memoryContent = parsed.memory;
      } catch (e) {
        // #2634: processQuestions previously swallowed this error completely (no
        // console.error at all) — the real cause was invisible even in devtools.
        // Now logged to match processUpload's catch block.
        console.error('[ONBOARD] Questions extraction FAILED:', e, typeof e, JSON.stringify(e));
        soulContent = preset;
        memoryContent = qName ? `## About ${qName}\n${qDo || ''}\n${qWorking ? 'Currently: ' + qWorking : ''}\n${qExtra || ''}` : '';
      }

      // Write files (agent was already created above, before synthesis ran)
      if (!agent.existed) {
        await ensureAgentFile(agent.id, 'MEMORY.md', memoryContent);
        await ensureAgentFile(agent.id, 'SOUL.md', soulContent);
      }

      workshopInitialMessage = "I've created your profile from your answers. This is your **Memory** -- take a look and let me know if anything needs adjusting.";

      memoryNoteId = await ensureNote(WORKSHOP_SCOPE_ID, 'Memory', memoryContent || '');
      soulNoteId = await ensureNote(WORKSHOP_SCOPE_ID, 'Soul', soulContent || '');
      screen = 'workshop';
    } catch (e) {
      console.error('[ONBOARD] processQuestions FAILED:', e);
      processingError = e instanceof Error ? e.message : String(e);
      screen = 'questions';
      return;
    }
  }

  // advanceWorkshop/advanceToSoul/sendChatMessage moved into WorkshopReview.svelte
  // (#2288 PR 1). advanceToRules (formerly here) was dropped entirely — dead code
  // per the Rules-step removal, not carried into the extracted component.


  async function saveAndComplete(method: string, messages: Array<{role: string; content: string}> = []) {
    console.log('[ONBOARD] Starting saveAndComplete, method:', method);
    try {
      // Write files to .pane/system/[agent UUID]/
      // #2277 Fix 1: no longer also mirrors to ai_set_soul/ai_set_workspace_memory (removed —
      // those wrote the global .pane/ai/soul.md and workspace-memory.md files). Per-agent
      // SOUL.md/MEMORY.md written via write_system_file below are the sole source.
      if (soulContent) {
        console.log('[ONBOARD] Writing SOUL.md');
        await invoke('write_system_file', {
          filename: 'SOUL.md',
          content: soulContent,
          agentId: ocMainAgentId
        });
      }
      if (memoryContent) {
        console.log('[ONBOARD] Writing MEMORY.md');
        await invoke('write_system_file', {
          filename: 'MEMORY.md',
          content: memoryContent,
          agentId: ocMainAgentId
        });
      }
      // RULES.md authoring dropped per ADR-0060 — the Rules workshop step was
      // removed, so there is no rulesContent to write here anymore.
      console.log('[ONBOARD] Setting onboarding_completed = true');
      await invoke('set_ai_setting', { key: 'onboarding_completed', value: 'true' });
      await invoke('set_ai_setting', { key: 'onboarding_method', value: method });
      console.log('[ONBOARD] Onboarding settings saved successfully');
    } catch (e) { 
      console.error('[ONBOARD] Save failed:', e); 
    }
    
    // Persist onboarding chat to workspace BEFORE completing
    // #2308: was calling raw invoke('save_chat_message'/'load_chat_messages') with
    // { projectId, role, content } / { projectId, limit } — missing the required
    // threadId (save_chat_message) and sessionId (load_chat_messages) keys entirely,
    // so every save/verify silently failed at the Tauri invoke layer (wrong-shaped
    // args never reach the Rust handler, so the [CHAT-SAVE]/[CHAT-QUERY] eprintln!
    // traces never even fired). Routed through the same api.ts wrappers every other
    // call site (ChatSidebar.svelte, api.ts itself) already uses — those wrappers
    // resolve a real thread via getActiveSession and pass threadId/sessionId with
    // the exact keys the Rust commands expect (see save_chat_message/load_chat_messages
    // signatures in src-tauri/src/ai/commands.rs).
    const validMessages = messages.filter((m: {role: string; content: string}) => m.content.trim() && m.role !== 'system');
    if (validMessages.length > 0) {
      try {
        const session = await api.getActiveSession('workspace');
        const threadId = session.id;
        for (let i = 0; i < validMessages.length; i++) {
          const msg = validMessages[i];
          try {
            const msgId = Date.now().toString(36) + Math.random().toString(36).substr(2, 8) + '-' + i;
            if (msg.role === 'user') {
              await api.sendChatMessage(threadId, 'workspace', msg.content, msgId);
            } else {
              await api.saveAssistantMessage(threadId, 'workspace', msg.content, msgId);
            }
          } catch (e) {
            console.error('[ONBOARD] Failed to save message:', e);
          }
        }
        // Verify messages were actually saved
        try {
          const saved = await api.getChatMessages(threadId, 50);
          console.log('[ONBOARD] Verified', saved.length, 'message(s) saved to thread:', threadId);
        } catch (e) { console.error('[ONBOARD] Verification failed:', e); }
      } catch (e) {
        console.error('[ONBOARD] Failed to resolve active session for transcript save:', e);
      }
    }
    
    
    // Post-onboarding AI greeting is now handled by ChatSidebar after onboarding completes

    // Purge workshop notes (#2213 write-through variant).
    // Notes were created so the AI could live-edit them via edit_note during the workshop.
    // Content is now safely in .pane/system/{UUID}/. Remove the notes so they don't
    // appear as duplicates in Projects/My Notes/General/.
    const workshopNoteIds = [memoryNoteId, soulNoteId].filter(Boolean);
    for (const noteId of workshopNoteIds) {
      // #2302: purge failures must surface, not vanish silently. A failed
      // purge leaves a stale workshop-scope row behind (visible as a
      // duplicate note later), so log at error level rather than warn.
      try {
        await invoke('purge_note', { noteId });
      } catch (e) {
        console.error('[ONBOARD] Failed to purge workshop note:', noteId, e);
      }
    }
    if (workshopNoteIds.length > 0) {
      console.log('[ONBOARD] Purged', workshopNoteIds.length, 'workshop note(s):', workshopNoteIds);
    }

    window.dispatchEvent(new CustomEvent('pane-onboarding-complete'));
    onComplete();
    // Removed: setTimeout toggle that set activeChatThreadId to the literal 'workspace' string
    // (not a valid thread UUID), causing the home greeting to vanish immediately after seeding.
    // pane-onboarding-complete → handleOnboardingComplete in ChatSidebar calls
    // loadOrCreateThread('workspace') and resolves the real UUID correctly.
  }

  function handleDrop(e: DragEvent) {
    e.preventDefault(); isDragging = false;
    const files = e.dataTransfer?.files;
    if (!files) return;
    for (const file of Array.from(files)) {
      if (file.name.endsWith('.txt') || file.name.endsWith('.md')) {
        file.text().then(text => { if (text.trim()) uploadedFiles = [...uploadedFiles, { name: file.name, content: text }]; });
      }
    }
  }

  function handleFileInput(e: Event) {
    const input = e.target as HTMLInputElement;
    if (!input.files) return;
    for (const file of Array.from(input.files)) {
      if (file.name.endsWith('.txt') || file.name.endsWith('.md')) {
        file.text().then(text => { if (text.trim()) uploadedFiles = [...uploadedFiles, { name: file.name, content: text }]; });
      }
    }
  }

  // #2311: "What should I upload?" help link — opens in system browser, not in-app WebView.
  async function handleUploadHelpClick() {
    await openExternalUrl('https://paneapp.ai/system-files-creation', 'ONBOARD');
  }
  
  // Restore offer functions
  async function handleRestoreFromBackup() {
    restoreError = '';
    restoreStatus = 'idle';
    
    try {
      // Open file picker for .panebackup files
      const { open } = await import('@tauri-apps/plugin-dialog');
      const file = await open({
        filters: [{ name: 'Pane Backup', extensions: ['panebackup'] }],
        multiple: false
      });
      
      if (!file) return; // User cancelled
      
      const filePath = typeof file === 'string' ? file : (file as any).path as string;
      pendingRestorePath = filePath;
      restoreInProgress = true;
      
      // Attempt restore
      const summary = await invoke<{warnings?: Array<{message: string}>}>('restore_backup_from_file', { backupPath: filePath });
      
      // Success - show summary and complete onboarding
      restoreSummary = summary;
      if (summary.warnings && summary.warnings.length > 0) {
        const warningMsgs = summary.warnings.map((w: any) => w.message).join('\n');
        console.log('[RESTORE] Completed with warnings:', warningMsgs);
      }
      
      // Complete onboarding - skip to Home
      await invoke('set_ai_setting', { key: 'onboarding_completed', value: 'true' });
      onComplete();
    } catch (e: any) {
      const errorStr = String(e);
      console.error('[RESTORE] Error:', errorStr);
      
      if (errorStr.includes('SCHEMA_TOO_NEW')) {
        restoreStatus = 'schema-too-new';
        restoreError = 'This backup was created with a newer version of Pane. Please update Pane first.';
      } else if (errorStr.includes('SCHEMA_OLDER')) {
        restoreStatus = 'schema-older';
        restoreError = 'This backup was created with an older version of Pane. Restore anyway?';
      } else if (errorStr.includes('DB_FAILED') || errorStr.includes('UUID_FAILED')) {
        restoreStatus = 'error';
        const msg = errorStr.replace(/^(DB_FAILED|UUID_FAILED):\s*/, '');
        restoreError = `Restore failed: ${msg}`;
      } else {
        restoreStatus = 'error';
        restoreError = `Restore failed: ${errorStr}`;
      }
    } finally {
      restoreInProgress = false;
    }
  }
  
  async function handleRestoreConfirmed() {
    if (!pendingRestorePath) return;
    
    restoreError = '';
    restoreInProgress = true;
    
    try {
      const summary = await invoke<{warnings?: Array<{message: string}>}>('restore_backup_confirmed_cmd', { backupPath: pendingRestorePath });
      
      restoreSummary = summary;
      if (summary.warnings && summary.warnings.length > 0) {
        const warningMsgs = summary.warnings.map((w: any) => w.message).join('\n');
        console.log('[RESTORE] Completed with warnings:', warningMsgs);
      }
      
      // Complete onboarding - skip to Home
      await invoke('set_ai_setting', { key: 'onboarding_completed', value: 'true' });
      onComplete();
    } catch (e: any) {
      console.error('[RESTORE] Confirmed restore failed:', e);
      restoreStatus = 'error';
      restoreError = `Restore failed: ${e}`;
    } finally {
      restoreInProgress = false;
    }
  }
  
  function handleRestoreCancel() {
    restoreStatus = 'idle';
    restoreError = '';
    pendingRestorePath = '';
  }
  
  function handleStartFresh() {
    screen = 'connect';
  }
</script>

<div class="onboarding-overlay">
  {#if screen === 'restore-offer'}
    <div class="onboarding-center">
      <h1>Welcome to Pane</h1>
      <p class="subtitle">Do you have a backup from another machine?</p>
      
      <div class="restore-actions">
        <button 
          class="btn-primary" 
          onclick={handleRestoreFromBackup}
          disabled={restoreInProgress}
        >
          {restoreInProgress ? 'Restoring...' : 'Restore from backup →'}
        </button>
        
        <button 
          class="btn-ghost" 
          onclick={handleStartFresh}
          disabled={restoreInProgress}
        >
          Start fresh →
        </button>
      </div>
      
      {#if restoreStatus === 'schema-too-new'}
        <div class="restore-error">
          <p>{restoreError}</p>
          <button class="btn-primary" onclick={handleRestoreCancel}>OK</button>
        </div>
      {/if}
      
      {#if restoreStatus === 'schema-older'}
        <div class="restore-confirm">
          <p>{restoreError}</p>
          <div class="restore-confirm-actions">
            <button class="btn-primary" onclick={handleRestoreConfirmed} disabled={restoreInProgress}>
              {restoreInProgress ? 'Restoring...' : 'Restore'}
            </button>
            <button class="btn-ghost" onclick={handleRestoreCancel} disabled={restoreInProgress}>Cancel</button>
          </div>
        </div>
      {/if}
      
      {#if restoreStatus === 'error'}
        <div class="restore-error">
          <p>{restoreError}</p>
          <div class="restore-error-actions">
            <button class="btn-primary" onclick={handleRestoreFromBackup}>Try again</button>
            <button class="btn-ghost" onclick={handleStartFresh}>Start fresh</button>
          </div>
        </div>
      {/if}
    </div>
  {:else if screen === 'connect'}
    <div class="onboarding-center">
        <h1>Connect Your AI</h1>
        <p class="subtitle">
          {#if connectProvider === 'openclaw'}
            Connect to your OpenClaw gateway to import your AI team.
          {:else if connectProvider === 'ollama'}
            Connect to your local Ollama instance.
          {:else}
            Choose a provider and enter your API key to get started.
          {/if}
        </p>
        
        <div class="connect-form">
          <!--
            Onboarding-picker ruling (#2455 venice live-test v2): this picker must
            show every added provider, Venice included, now and for future
            runtimes — it is one of FOUR provider-enumeration entry points in this
            codebase that each independently list runtime types (the others:
            NewProviderModal.svelte's `PROVIDER_ROWS`, DefaultAgentPickerModal.
            svelte's `groupOrder`, ProviderConnectionsPanel.svelte's fixed-set
            fetch). A fifth card (Venice) fits cleanly in this flex layout with no
            overflow/redesign needed — see docs/spec/onboarding.md's
            #2455 dynamic-picker ruling note.
          -->
          <div class="connect-providers">
            <button class="provider-card" class:active={connectProvider === 'openrouter'} onclick={() => connectProvider = 'openrouter'}>
              <span class="provider-name">OpenRouter</span>
              <span class="provider-desc">Access multiple models</span>
            </button>
            <button class="provider-card" class:active={connectProvider === 'anthropic'} onclick={() => connectProvider = 'anthropic'}>
              <span class="provider-name">Anthropic</span>
              <span class="provider-desc">Claude direct</span>
            </button>
            <button class="provider-card" class:active={connectProvider === 'venice'} onclick={() => connectProvider = 'venice'}>
              <span class="provider-name">Venice</span>
              <span class="provider-desc">Your Venice API models</span>
            </button>
            <button class="provider-card" class:active={connectProvider === 'ollama'} onclick={() => connectProvider = 'ollama'}>
              <span class="provider-name">Ollama</span>
              <span class="provider-desc">Local models</span>
            </button>
            <button class="provider-card" class:active={connectProvider === 'openclaw'} onclick={() => { connectProvider = 'openclaw'; screen = 'oc-url-entry'; }}>
              <span class="provider-name">OpenClaw</span>
              <span class="provider-desc">Your OpenClaw gateway</span>
            </button>
          </div>
          
          {#if connectProvider !== 'ollama' && connectProvider !== 'openclaw'}
            <div class="connect-field">
              <label>API Key</label>
              <input type="password" bind:value={connectKey} placeholder={connectProvider === 'openrouter' ? 'sk-or-v1-...' : 'sk-ant-...'} />
            </div>
          {:else}
            <div class="connect-field">
              <label>Endpoint</label>
              <input type="text" bind:value={connectEndpoint} placeholder="http://localhost:11434" />
            </div>
          {/if}
          
          <div class="connect-actions">
            <div class="test-row">
              <button 
                class="btn-primary" 
                onclick={testConnection} 
                disabled={connectTesting || (connectProvider !== 'ollama' && !connectKey.trim())}
              >
                {connectTesting ? 'Testing...' : 'Test Connection'}
              </button>
              {#if connectStatus === 'success'}
                <span class="connect-tag success">Connected</span>
              {/if}
              {#if connectStatus === 'error'}
                <span class="connect-tag error">Disconnected</span>
              {/if}
            </div>
            {#if connectStatus === 'error'}
              <div class="connect-error-msg">{connectError}</div>
              {#if connectError && connectError.includes('openrouter.ai/settings/credits')}
                <div class="connect-error-action">
                  <a href="https://openrouter.ai/settings/credits" target="_blank" rel="noopener noreferrer">Add credits →</a>
                </div>
              {/if}
            {/if}
          </div>
          
          {#if connectStatus === 'success'}
            <button class="btn-primary connect-continue" onclick={proceedFromConnect}>Continue</button>
          {/if}
        </div>
        
        <div class="skip-row">
          <button class="btn-skip-connect" onclick={skipConnect}>Skip — I'll set this up later</button>
        </div>
    </div>

  {:else if screen === 'ollama-unreachable'}
    <div class="onboarding-center">
      <h1>Couldn't find Ollama</h1>
      <p class="subtitle">Ollama doesn't appear to be installed or isn't running on this machine.</p>
      <div class="connect-form">
        <p style="font-size: 14px; color: #A1A1AA; line-height: 1.6; margin-bottom: 20px;">
          Pane probed <code style="font-family: monospace; background: #18181B; padding: 2px 6px; border-radius: 3px; border: 1px solid #27272A;">{connectEndpoint || 'http://localhost:11434'}</code> but got no response.
          This usually means Ollama isn't installed yet, or it isn't running.
          Download and install Ollama, then start it from your Applications folder or the menu bar icon.
        </p>

        <a
          href="https://ollama.com/download"
          target="_blank"
          rel="noopener noreferrer"
          style="display: block; width: 100%; max-width: 440px; text-align: center; padding: 12px 20px;
                 background: #00E4E4; color: #09090B; border-radius: 4px; font-size: 14px;
                 font-weight: 600; text-decoration: none; margin-bottom: 20px; box-sizing: border-box;
                 font-family: 'Inter', sans-serif;"
        >
          Download Ollama
        </a>

        <div class="connect-actions" style="flex-wrap: wrap; gap: 10px;">
          <button
            class="btn-primary"
            onclick={async () => { connectTesting = true; try { const ok = await probeOllama(connectEndpoint); if (ok) proceedFromConnect(); } catch (e: any) { connectStatus = 'error'; connectError = e.toString(); } finally { connectTesting = false; } }}
            disabled={connectTesting}
          >
            {connectTesting ? 'Probing…' : 'Try Again'}
          </button>
          <button
            class="btn-ghost"
            onclick={() => { ollamaScreenReason = 'unreachable'; screen = 'ollama-custom-endpoint'; }}
          >
            Use Custom Endpoint
          </button>
          <a
            href="https://ollama.com/download"
            target="_blank"
            rel="noopener noreferrer"
            style="font-size: 13px; color: #60A5FA; text-decoration: none; align-self: center;"
          >
            Help with Ollama →
          </a>
        </div>
      </div>
      <div class="skip-row">
        <button class="btn-skip-connect" onclick={() => screen = 'connect'}>← Back</button>
      </div>
    </div>

  {:else if screen === 'ollama-no-models'}
    <div class="onboarding-center">
      <h1>No Models Installed</h1>
      <p class="subtitle">Ollama is running, but you haven't pulled any models yet.</p>
      <div class="connect-form">
        <p style="font-size: 14px; color: #A1A1AA; line-height: 1.6; margin-bottom: 16px;">
          Ollama is running at <code style="font-family: monospace; background: #18181B; padding: 2px 6px; border-radius: 3px; border: 1px solid #27272A;">{connectEndpoint || 'http://localhost:11434'}</code>,
          but no models are installed. Run the command below in your Terminal to pull a model, then click Try Again.
        </p>

        <div style="background: #18181B; border: 1px solid #27272A; border-radius: 4px; padding: 12px 16px; margin-bottom: 16px; font-family: monospace; font-size: 13px; color: #00E4E4; user-select: all;">
          ollama pull llama3.2:3b
        </div>

        <p style="font-size: 12px; color: #71717A; margin-bottom: 20px;">
          Not sure which model to pick?
          <a href="https://ollama.com/library" target="_blank" rel="noopener noreferrer" style="color: #60A5FA; text-decoration: none;">Browse models on ollama.com/library →</a>
        </p>

        <div class="connect-actions" style="flex-wrap: wrap; gap: 10px;">
          <button
            class="btn-primary"
            onclick={async () => { connectTesting = true; try { const ok = await probeOllama(connectEndpoint); if (ok) proceedFromConnect(); } catch (e: any) { connectStatus = 'error'; connectError = e.toString(); } finally { connectTesting = false; } }}
            disabled={connectTesting}
          >
            {connectTesting ? 'Probing…' : 'Try Again'}
          </button>
        </div>
      </div>
      <div class="skip-row">
        <button class="btn-skip-connect" onclick={() => screen = 'connect'}>← Back</button>
      </div>
    </div>

  {:else if screen === 'ollama-custom-endpoint'}
    <div class="onboarding-center">
      <h1>Connect to Ollama</h1>
      <p class="subtitle">
        {#if ollamaScreenReason === 'tags_failed'}
          Found Ollama but couldn’t list models — check the endpoint or retry.
        {:else}
          Couldn’t find Ollama at <code style="font-family: monospace; background: #18181B; padding: 2px 5px; border-radius: 3px; border: 1px solid #27272A; font-size: 13px;">{connectEndpoint || 'localhost:11434'}</code>.
        {/if}
      </p>
      <div class="connect-form">
        <div class="connect-field">
          <label class="connect-label">Endpoint URL</label>
          <input
            type="text"
            class="connect-input"
            bind:value={connectEndpoint}
            placeholder="http://localhost:11434"
            disabled={connectTesting}
            onkeydown={(e) => { if (e.key === 'Enter' && connectEndpoint.trim()) { connectEndpoint = normalizeOllamaEndpoint(connectEndpoint); testConnection(); } }}
          />
          <p style="font-size: 12px; color: #71717A; margin-top: 6px;">
            The URL is normalized automatically: <code style="font-family: monospace;">localhost</code> → <code style="font-family: monospace;">http://localhost:11434</code>.
          </p>
        </div>

        {#if connectStatus === 'error'}
          <div class="connect-error-msg">{connectError}</div>
        {/if}

        <div class="connect-actions" style="flex-wrap: wrap; gap: 10px;">
          <button
            class="btn-primary"
            onclick={async () => {
              connectTesting = true;
              try {
                const ok = await probeOllama(connectEndpoint);
                if (ok) proceedFromConnect();
              } catch (e: any) {
                connectStatus = 'error';
                connectError = e.toString();
              } finally {
                connectTesting = false;
              }
            }}
            disabled={connectTesting || !connectEndpoint.trim()}
          >
            {connectTesting ? 'Probing…' : 'Try Again'}
          </button>
          <a
            href="https://ollama.com/download"
            target="_blank"
            rel="noopener noreferrer"
            style="font-size: 13px; color: #60A5FA; text-decoration: none; align-self: center;"
          >
            Help with Ollama →
          </a>
        </div>
      </div>
      <div class="skip-row">
        <button class="btn-skip-connect" onclick={() => screen = ollamaScreenReason === 'unreachable' ? 'ollama-unreachable' : 'connect'}>← Back</button>
      </div>
    </div>

  {:else if screen === 'oc-url-entry'}
    <div class="onboarding-center">
      <h1>Connect to Pane Gateway</h1>
      <p class="subtitle">Pairing enables automatic two-way sync. The gateway receives supported Pane agent workspace context—including identity, instructions, memory, tools, and logs—and may update local allowlisted agent files.</p>
      <div class="connect-form">
        <div class="connect-field">
          <label>Gateway URL</label>
          <input
            type="text"
            bind:value={ocUrlInput}
            placeholder="https://100.x.x.x:18789"
            onkeydown={(e) => { if (e.key === 'Enter') probeGatewayUrl(); }}
          />
        </div>
        {#if ocProbeError}
          <div class="connect-error-msg">{ocProbeError}</div>
        {/if}
        <div class="connect-actions">
          <div class="test-row">
            <button class="btn-primary" onclick={probeGatewayUrl} disabled={ocProbing || !ocUrlInput.trim()}>
              {ocProbing ? 'Connecting...' : 'Continue'}
            </button>
          </div>
        </div>
      </div>
      <div class="skip-row">
        <button class="btn-skip-connect" onclick={() => { screen = 'connect'; connectProvider = 'openrouter'; }}>← Back</button>
      </div>
    </div>

  {:else if screen === 'oc-verify-connect'}
    <div class="onboarding-center">
      <h1>Verify & Connect</h1>
      <p class="subtitle">Confirm the fingerprint, then enter your pairing code.</p>
      <div class="connect-form">
        <div class="connect-field">
          <label>TLS Fingerprint</label>
          <div style="font-family: monospace; font-size: 12px; word-break: break-all; background: #18181B; padding: 10px; border-radius: 4px; border: 1px solid #27272A; color: #A1A1AA;">{ocFingerprint}</div>
          <p style="font-size: 12px; color: #71717A; margin-top: 6px;">Compare the fingerprint shown above against the one Pane Gateway emitted to its terminal on startup (on the OC machine). If they match, enter the pairing code below.</p>
        </div>
        <div class="connect-field">
          <label>Pairing Code</label>
          <input
            type="text"
            bind:value={ocPairingCode}
            placeholder="000000"
            maxlength="6"
            inputmode="numeric"
            onkeydown={(e) => { if (e.key === 'Enter') exchangePairingCode(); }}
          />
          <p style="font-size: 12px; color: #71717A; margin-top: 6px;">Enter the 6-digit code from Pane Gateway's terminal on the OC machine. Codes expire after 5 minutes. If expired, run: <code>pane-gateway pairing-code</code></p>
        </div>
        {#if ocPairingError}
          <div class="connect-error-msg">{ocPairingError}</div>
        {/if}
        <div class="connect-actions">
          <div class="test-row">
            <button class="btn-primary" onclick={exchangePairingCode} disabled={ocPairing || !ocPairingCode.trim()}>
              {ocPairing ? 'Connecting...' : 'Connect'}
            </button>
          </div>
        </div>
        <div style="margin-top: 16px;">
          <button class="btn-ghost" style="color: #EF4444; font-size: 12px;" onclick={() => { screen = 'oc-url-entry'; ocFingerprint = ''; ocCertPem = ''; ocPairingError = ''; }}>
            Fingerprint doesn't match
          </button>
        </div>
      </div>
      <div class="skip-row">
        <button class="btn-skip-connect" onclick={() => { screen = 'oc-url-entry'; ocFingerprint = ''; ocCertPem = ''; ocPairingError = ''; }}>← Back</button>
      </div>
    </div>

  {:else if screen === 'oc-sync'}
    <div class="onboarding-center">
      <h1>Syncing Your Agents</h1>
      <p class="subtitle">Importing your agent files from the gateway...</p>
      <div class="connect-form">
        {#if ocRepairDeclined}
          <!-- D20 (#2649): dismissable, non-error notice — the user explicitly
               declined the re-pair confirmation. Deliberately NOT the ocSyncError/
               connect-error-msg styling; this is a user choice, not a failure
               (Matt's ruling #3). -->
          <div class="connect-repair-declined" data-testid="oc-repair-declined-notice">
            <span>Re-pair cancelled, nothing changed</span>
          </div>
          <div class="connect-actions">
            <button class="btn-primary" onclick={dismissRepairDeclinedNotice}>Dismiss</button>
            <button class="btn-ghost" onclick={() => { screen = 'oc-verify-connect'; ocRepairDeclined = false; }}>← Back</button>
          </div>
        {:else if ocSyncing}
          <p style="color: #A1A1AA; text-align: center;">Syncing your agents...</p>
        {:else if ocSyncErrorCode === 'oc_handshake_timeout'}
          <!-- §22C.5.4 recovery message: stale OC_BEARER_TOKEN in gateway service file -->
          <div class="connect-error-msg" style="line-height:1.5">
            <strong>Can't finish connecting to OpenClaw — the gateway's OpenClaw token may be out of date.</strong>
            <p style="margin-top:8px">The Pane gateway reached OpenClaw but couldn't complete the authenticated handshake within 10 seconds. The most likely cause is that OpenClaw rotated its gateway token and the Pane gateway is still using the old one. (Less commonly, OpenClaw may be restarting or busy right after the connection opens — if this clears on its own, the token is fine.)</p>
            <p style="margin-top:8px"><strong>To update a stale token:</strong></p>
            <ol style="margin:6px 0 0 18px;padding:0;font-size:12px">
              <li style="margin-bottom:4px">On the OpenClaw machine, read the current token at <code>~/.openclaw/openclaw.json</code> → <code>gateway.auth.token</code>.</li>
              <li style="margin-bottom:4px">Find the Pane gateway's service file that sets <code>OC_BEARER_TOKEN</code>:
                <ul style="margin:4px 0 0 16px">
                  <li><strong>macOS (launchd):</strong> <code>grep -rl OC_BEARER_TOKEN ~/Library/LaunchAgents/ /Library/LaunchAgents/ /Library/LaunchDaemons/ 2&gt;/dev/null</code></li>
                  <li><strong>Linux (systemd):</strong> <code>grep -rl OC_BEARER_TOKEN /etc/systemd/system/ ~/.config/systemd/user/ 2&gt;/dev/null</code></li>
                </ul>
              </li>
              <li style="margin-bottom:4px">Restart the <strong>Pane gateway</strong> with the label/unit from that file:
                <ul style="margin:4px 0 0 16px">
                  <li><strong>macOS:</strong> <code>launchctl kickstart -k gui/$(id -u)/LABEL</code></li>
                  <li><strong>Linux:</strong> <code>systemctl restart UNIT</code> (add <code>--user</code> for <code>~/.config/systemd/user/</code>)</li>
                </ul>
              </li>
            </ol>
          </div>
          <div class="connect-actions">
            <button class="btn-primary" onclick={runOcSync}>Retry</button>
            <button class="btn-ghost" onclick={() => { screen = 'oc-verify-connect'; ocSyncError = ''; ocSyncErrorCode = null; ocSyncing = false; }}>← Back</button>
          </div>
        {:else if ocSyncError}
          <div class="connect-error-msg">{ocSyncError}</div>
          <div class="connect-actions">
            <button class="btn-primary" onclick={runOcSync}>Retry</button>
            <button class="btn-ghost" onclick={() => { screen = 'oc-verify-connect'; ocSyncError = ''; ocSyncErrorCode = null; ocSyncing = false; }}>← Back</button>
          </div>
        {/if}
        {#if syncError}
          <div class="connect-error-msg">{syncError}</div>
        {/if}
        {#if ocVersionWarning}
          <div style="display:flex;align-items:flex-start;gap:6px;margin-top:12px;font-size:13px;color:#f59e0b;line-height:1.4">
            <span style="flex-shrink:0">⚠️</span> {ocVersionWarning}
          </div>
        {/if}
      </div>
    </div>
    {#if !ocSyncing && !ocSyncError}
      <!-- Auto-trigger sync on mount -->
    {/if}

  {:else if screen === 'model-select'}
    <div class="onboarding-center">
      <h1>Choose Your Model</h1>
      <p class="subtitle">Select your primary AI model for chat and tasks.</p>

      <div style="max-width: 500px; margin: 0 auto; padding: 24px; background: #18181B; border-radius: 8px; border: 1px solid #27272A;">
        {#if primaryModelLoading}
          <p style="color: #A1A1AA; text-align: center; padding: 20px 0;">Loading models…</p>
        {:else if primaryModelError}
          <p style="color: #EF4444; font-size: 13px; margin-bottom: 12px;">{primaryModelError}</p>
          <div style="display: flex; flex-direction: column; gap: 8px;">
            <button class="btn-ghost" onclick={fetchPrimaryModels} style="width: 100%;">Retry</button>
            <button class="btn-ghost" onclick={() => { screen = 'connect'; primaryModelError = ''; }} style="width: 100%;">← Back</button>
          </div>
        {:else}
          <div style="margin-bottom: 0;">
            <label style="display: block; font-size: 13px; font-weight: 600; color: #FAFAFA; margin-bottom: 8px;">
              Model
            </label>
            <SearchableModelSelect
              models={primaryModelList}
              provider={connectProvider}
              selectedModel={selectedComplexModel}
              placeholder="Search models... (e.g. Claude, GPT, Gemini)"
              onSelect={(modelId) => { selectedComplexModel = modelId; }}
            />
          </div>
        {/if}
      </div>

      <button
        class="btn-primary connect-continue"
        style="max-width: 500px; margin: 24px auto 0; display: block; width: 100%;"
        onclick={proceedFromModelSelect}
        disabled={!selectedComplexModel || primaryModelLoading}
      >Continue</button>
    </div>


  {:else if screen === 'smart-features-picker'}
  <div class="onboarding-center">
    <h1>Connect Smart Features</h1>
    <p class="subtitle">Smart Features uses a separate AI connection for memory extraction, smart routing, and search reranking. You can set this up now or later in Settings.</p>

    <div class="connect-form">
      <div class="connect-providers">
        <button class="provider-card" class:active={sfProvider === 'openrouter'} onclick={() => { sfProvider = 'openrouter'; startSmartFeaturesFlow('openrouter'); }}>
          <span class="provider-name">OpenRouter</span>
          <span class="provider-desc">Access multiple models</span>
        </button>
        <button class="provider-card" class:active={sfProvider === 'anthropic'} onclick={() => { sfProvider = 'anthropic'; startSmartFeaturesFlow('anthropic'); }}>
          <span class="provider-name">Anthropic</span>
          <span class="provider-desc">Claude direct</span>
        </button>
        <!-- #2795 R3: Venice as the 4th hardcoded picker card, per #2792's convention
             (name + one-line descriptor). -->
        <button class="provider-card" class:active={sfProvider === 'venice'} onclick={() => { sfProvider = 'venice'; startSmartFeaturesFlow('venice'); }}>
          <span class="provider-name">Venice</span>
          <span class="provider-desc">Your Venice API models</span>
        </button>
        <button class="provider-card" class:active={sfProvider === 'ollama'} onclick={() => { sfProvider = 'ollama'; startSmartFeaturesFlow('ollama'); }}>
          <span class="provider-name">Ollama</span>
          <span class="provider-desc">Local models</span>
        </button>
      </div>

      <div class="skip-row" style="flex-direction: column; align-items: center; gap: 8px;">
        <button class="btn-skip-connect" onclick={skipSmartFeatures}>Skip — I'll set this up later</button>
        <button class="btn-ghost" onclick={() => { screen = 'model-select'; }}>Back</button>
      </div>
    </div>
  </div>

  {:else if screen === 'smart-features-or-setup'}
  <div class="onboarding-center">
    <h1 style="margin-bottom: 24px;">Smart Features — OpenRouter</h1>

    {#if sfStep === 'credentials'}
      <div class="connect-form" style="margin-top: 16px;">
        <div class="connect-field">
          <label class="connect-label">API Key</label>
          <input
            type="password"
            class="connect-input"
            bind:value={sfApiKey}
            placeholder="sk-or-v1-..."
            disabled={sfTesting}
          />
          <p style="font-size: 12px; color: #71717A; margin-top: 6px;">
            <a href="https://openrouter.ai/keys" target="_blank" style="color: #00E4E4;">Get a key → openrouter.ai/keys</a>
          </p>
        </div>

        {#if sfConnectionStatus === 'error'}
          <div class="connect-error-msg">{sfConnectionError}</div>
        {/if}

        <div class="connect-actions">
          <button class="btn-ghost" onclick={() => screen = 'smart-features-picker'} disabled={sfTesting}>Back</button>
          <button class="btn-primary" onclick={sfTestConnection} disabled={sfTesting || !sfApiKey}>
            {sfTesting ? 'Testing…' : 'Test Connection'}
          </button>
        </div>
      </div>
    {:else}
      <div class="connect-form">
        <p style="font-size: 13px; color: #A1A1AA; margin-bottom: 16px;">Select models for Smart Features. Only Extraction Model is required.</p>

        <div style="margin-bottom: 16px;">
          <label style="display: block; font-size: 13px; font-weight: 600; color: #FAFAFA; margin-bottom: 4px;">
            Extraction Model <span style="color: #EF4444;">*</span>
          </label>
          <p style="font-size: 11px; color: #71717A; margin: 0 0 8px;">Model for memory extraction from conversations</p>
          <SearchableModelSelect
            models={sfModels}
            provider={sfProvider}
            selectedModel={sfExtractionModel}
            placeholder="Search models..."
            onSelect={(modelId) => { sfExtractionModel = modelId; }}
          />
        </div>

        <div style="margin-bottom: 16px;">
          <label style="display: block; font-size: 13px; font-weight: 600; color: #FAFAFA; margin-bottom: 4px;">Smart Routing Model</label>
          <p style="font-size: 11px; color: #71717A; margin: 0 0 8px;">Used for message classification and simple queries. Pick a capable model — weak classifiers can degrade your answers.</p>
          <SearchableModelSelect
            models={sfModels}
            provider={sfProvider}
            selectedModel={sfSmartRoutingModel}
            placeholder="Search models..."
            onSelect={(modelId) => { sfSmartRoutingModel = modelId; }}
          />
        </div>

        <div style="margin-bottom: 16px;">
          <label style="display: block; font-size: 13px; font-weight: 600; color: #FAFAFA; margin-bottom: 4px;">Rerank Model</label>
          <p style="font-size: 11px; color: #71717A; margin: 0 0 8px;">Improves memory search accuracy</p>
          <SearchableModelSelect
            models={sfModels}
            provider={sfProvider}
            selectedModel={sfRerankModel}
            placeholder="Search models..."
            onSelect={(modelId) => { sfRerankModel = modelId; }}
          />
        </div>

        <div class="connect-actions">
          <button class="btn-ghost" onclick={() => sfStep = 'credentials'}>Back</button>
          <button class="btn-primary" onclick={submitSmartFeatures} disabled={!sfExtractionModel}>Submit</button>
        </div>
      </div>
    {/if}
  </div>

  {:else if screen === 'smart-features-anthropic-setup'}
  <div class="onboarding-center">
    <h1 style="margin-bottom: 24px;">Smart Features — Anthropic</h1>

    {#if sfStep === 'credentials'}
      <div class="connect-form" style="margin-top: 16px;">
        <div class="connect-field">
          <label class="connect-label">API Key</label>
          <input
            type="password"
            class="connect-input"
            bind:value={sfApiKey}
            placeholder="sk-ant-..."
            disabled={sfTesting}
          />
          <p style="font-size: 12px; color: #71717A; margin-top: 6px;">
            <a href="https://console.anthropic.com/settings/keys" target="_blank" style="color: #00E4E4;">Get a key → console.anthropic.com/settings/keys</a>
          </p>
        </div>

        {#if sfConnectionStatus === 'error'}
          <div class="connect-error-msg">{sfConnectionError}</div>
        {/if}

        <div class="connect-actions">
          <button class="btn-ghost" onclick={() => screen = 'smart-features-picker'} disabled={sfTesting}>Back</button>
          <button class="btn-primary" onclick={sfTestConnection} disabled={sfTesting || !sfApiKey}>
            {sfTesting ? 'Testing…' : 'Test Connection'}
          </button>
        </div>
      </div>
    {:else}
      <div class="connect-form">
        <p style="font-size: 13px; color: #A1A1AA; margin-bottom: 16px;">Select models for Smart Features. Only Extraction Model is required.</p>

        <div style="margin-bottom: 16px;">
          <label style="display: block; font-size: 13px; font-weight: 600; color: #FAFAFA; margin-bottom: 4px;">
            Extraction Model <span style="color: #EF4444;">*</span>
          </label>
          <p style="font-size: 11px; color: #71717A; margin: 0 0 8px;">Model for memory extraction from conversations</p>
          <SearchableModelSelect
            models={sfModels}
            provider={sfProvider}
            selectedModel={sfExtractionModel}
            placeholder="Search models..."
            onSelect={(modelId) => { sfExtractionModel = modelId; }}
          />
        </div>

        <div style="margin-bottom: 16px;">
          <label style="display: block; font-size: 13px; font-weight: 600; color: #FAFAFA; margin-bottom: 4px;">Smart Routing Model</label>
          <p style="font-size: 11px; color: #71717A; margin: 0 0 8px;">Used for message classification and simple queries. Pick a capable model — weak classifiers can degrade your answers.</p>
          <SearchableModelSelect
            models={sfModels}
            provider={sfProvider}
            selectedModel={sfSmartRoutingModel}
            placeholder="Search models..."
            onSelect={(modelId) => { sfSmartRoutingModel = modelId; }}
          />
        </div>

        <div style="margin-bottom: 16px;">
          <label style="display: block; font-size: 13px; font-weight: 600; color: #FAFAFA; margin-bottom: 4px;">Rerank Model</label>
          <p style="font-size: 11px; color: #71717A; margin: 0 0 8px;">Improves memory search accuracy</p>
          <SearchableModelSelect
            models={sfModels}
            provider={sfProvider}
            selectedModel={sfRerankModel}
            placeholder="Search models..."
            onSelect={(modelId) => { sfRerankModel = modelId; }}
          />
        </div>

        <div class="connect-actions">
          <button class="btn-ghost" onclick={() => sfStep = 'credentials'}>Back</button>
          <button class="btn-primary" onclick={submitSmartFeatures} disabled={!sfExtractionModel}>Submit</button>
        </div>
      </div>
    {/if}
  </div>

  {:else if screen === 'smart-features-venice-setup'}
  <div class="onboarding-center">
    <h1 style="margin-bottom: 24px;">Smart Features — Venice</h1>

    {#if sfStep === 'credentials'}
      <div class="connect-form" style="margin-top: 16px;">
        <div class="connect-field">
          <label class="connect-label">API Key</label>
          <input
            type="password"
            class="connect-input"
            bind:value={sfApiKey}
            placeholder="VEN-..."
            disabled={sfTesting}
          />
          <p style="font-size: 12px; color: #71717A; margin-top: 6px;">
            <a href="https://venice.ai/settings/api" target="_blank" style="color: #00E4E4;">Get a key → venice.ai/settings/api</a>
          </p>
        </div>

        {#if sfConnectionStatus === 'error'}
          <div class="connect-error-msg">{sfConnectionError}</div>
        {/if}

        <div class="connect-actions">
          <button class="btn-ghost" onclick={() => screen = 'smart-features-picker'} disabled={sfTesting}>Back</button>
          <button class="btn-primary" onclick={sfTestConnection} disabled={sfTesting || !sfApiKey}>
            {sfTesting ? 'Testing…' : 'Test Connection'}
          </button>
        </div>
      </div>
    {:else}
      <div class="connect-form">
        <p style="font-size: 13px; color: #A1A1AA; margin-bottom: 16px;">Select models for Smart Features. Only Extraction Model is required.</p>

        <div style="margin-bottom: 16px;">
          <label style="display: block; font-size: 13px; font-weight: 600; color: #FAFAFA; margin-bottom: 4px;">
            Extraction Model <span style="color: #EF4444;">*</span>
          </label>
          <p style="font-size: 11px; color: #71717A; margin: 0 0 8px;">Model for memory extraction from conversations</p>
          <SearchableModelSelect
            models={sfModels}
            provider={sfProvider}
            selectedModel={sfExtractionModel}
            placeholder="Search models..."
            onSelect={(modelId) => { sfExtractionModel = modelId; }}
          />
        </div>

        <div style="margin-bottom: 16px;">
          <label style="display: block; font-size: 13px; font-weight: 600; color: #FAFAFA; margin-bottom: 4px;">Smart Routing Model</label>
          <p style="font-size: 11px; color: #71717A; margin: 0 0 8px;">Used for message classification and simple queries. Pick a capable model — weak classifiers can degrade your answers.</p>
          <SearchableModelSelect
            models={sfModels}
            provider={sfProvider}
            selectedModel={sfSmartRoutingModel}
            placeholder="Search models..."
            onSelect={(modelId) => { sfSmartRoutingModel = modelId; }}
          />
        </div>

        <div style="margin-bottom: 16px;">
          <label style="display: block; font-size: 13px; font-weight: 600; color: #FAFAFA; margin-bottom: 4px;">Rerank Model</label>
          <p style="font-size: 11px; color: #71717A; margin: 0 0 8px;">Improves memory search accuracy</p>
          <SearchableModelSelect
            models={sfModels}
            provider={sfProvider}
            selectedModel={sfRerankModel}
            placeholder="Search models..."
            onSelect={(modelId) => { sfRerankModel = modelId; }}
          />
        </div>

        <div class="connect-actions">
          <button class="btn-ghost" onclick={() => sfStep = 'credentials'}>Back</button>
          <button class="btn-primary" onclick={submitSmartFeatures} disabled={!sfExtractionModel}>Submit</button>
        </div>
      </div>
    {/if}
  </div>

  {:else if screen === 'smart-features-ollama-setup'}
  <div class="onboarding-center">
    <h1 style="margin-bottom: 24px;">Smart Features — Ollama</h1>

    {#if sfStep === 'credentials'}
      <div class="connect-form" style="margin-top: 16px;">
        <div class="connect-field">
          <label class="connect-label">Ollama Endpoint</label>
          <input
            type="text"
            class="connect-input"
            bind:value={sfEndpoint}
            placeholder="http://localhost:11434"
            disabled={sfTesting}
          />
        </div>

        {#if sfConnectionStatus === 'error'}
          <div class="connect-error-msg">{sfConnectionError}</div>
        {/if}

        <div class="connect-actions">
          <button class="btn-ghost" onclick={() => screen = 'smart-features-picker'} disabled={sfTesting}>Back</button>
          <button class="btn-primary" onclick={sfTestConnection} disabled={sfTesting}>
            {sfTesting ? 'Testing…' : 'Test Connection'}
          </button>
        </div>
      </div>
    {:else}
      <div class="connect-form">
        <p style="font-size: 13px; color: #A1A1AA; margin-bottom: 16px;">Select models for Smart Features. Only Extraction Model is required.</p>

        <div style="margin-bottom: 16px;">
          <label style="display: block; font-size: 13px; font-weight: 600; color: #FAFAFA; margin-bottom: 4px;">
            Extraction Model <span style="color: #EF4444;">*</span>
          </label>
          <p style="font-size: 11px; color: #71717A; margin: 0 0 8px;">Model for memory extraction from conversations</p>
          <SearchableModelSelect
            models={sfModels}
            provider={sfProvider}
            selectedModel={sfExtractionModel}
            placeholder="Search models..."
            onSelect={(modelId) => { sfExtractionModel = modelId; }}
          />
        </div>

        <div style="margin-bottom: 16px;">
          <label style="display: block; font-size: 13px; font-weight: 600; color: #FAFAFA; margin-bottom: 4px;">Smart Routing Model</label>
          <p style="font-size: 11px; color: #71717A; margin: 0 0 8px;">Used for message classification and simple queries. Pick a capable model — weak classifiers can degrade your answers.</p>
          <SearchableModelSelect
            models={sfModels}
            provider={sfProvider}
            selectedModel={sfSmartRoutingModel}
            placeholder="Search models..."
            onSelect={(modelId) => { sfSmartRoutingModel = modelId; }}
          />
        </div>

        <div style="margin-bottom: 16px;">
          <label style="display: block; font-size: 13px; font-weight: 600; color: #FAFAFA; margin-bottom: 4px;">Rerank Model</label>
          <p style="font-size: 11px; color: #71717A; margin: 0 0 8px;">Improves memory search accuracy</p>
          <SearchableModelSelect
            models={sfModels}
            provider={sfProvider}
            selectedModel={sfRerankModel}
            placeholder="Search models..."
            onSelect={(modelId) => { sfRerankModel = modelId; }}
          />
        </div>

        <div style="background: rgba(234, 179, 8, 0.1); border: 1px solid rgba(234, 179, 8, 0.3); border-radius: 8px; padding: 12px; margin-bottom: 16px;">
          <p style="font-size: 12px; color: #EAB308; margin: 0; font-weight: 600;">⚠️ Model Size Recommendation</p>
          <p style="font-size: 12px; color: #A1A1AA; margin: 4px 0 0;">Smart Features work best with models 14B parameters or larger. Smaller models may not reliably extract memories or classify messages. We recommend <code style="background: rgba(255,255,255,0.1); padding: 2px 4px; border-radius: 3px;">qwen2.5:14b</code> or larger.</p>
        </div>

        <div class="connect-actions">
          <button class="btn-ghost" onclick={() => sfStep = 'credentials'}>Back</button>
          <button class="btn-primary" onclick={submitSmartFeatures} disabled={!sfExtractionModel}>Submit</button>
        </div>
      </div>
    {/if}
  </div>

  {:else if screen === 'main'}
  <div class="onboarding-center">
    <h1>Set Up Your AI Partner</h1>
    <p class="subtitle">Your AI adapts to you. Choose how to get started:</p>
    
    <div class="style-section" style="border-top: none; padding-top: 0; margin-top: 0;">
      <div class="style-label">What should your AI be called?</div>
      <input 
        type="text" 
        class="agent-name-input" 
        bind:value={ocMainAgentName} 
        placeholder="e.g., Atlas, Friday, Jarvis"
        style="width: 100%; padding: 10px; margin-bottom: 16px; background: rgba(255,255,255,0.05); border: 1px solid rgba(255,255,255,0.1); border-radius: 6px; color: white; font-size: 14px;" 
      />
      <div class="style-desc" style="margin-bottom: 20px; font-size: 12px; color: rgba(255,255,255,0.5);">
        Leave blank to use your system username. You can rename your AI later in Settings.
      </div>
    </div>
    
    <div class="style-section">
      <div class="style-label">Communication Style</div>
      <div class="style-pills">
        <button class="style-pill" class:active={stylePreset === 'concise'} onclick={() => { stylePreset = 'concise'; qComm = 'short'; }}>Concise</button>
        <button class="style-pill" class:active={stylePreset === 'balanced'} onclick={() => { stylePreset = 'balanced'; qComm = 'balanced'; }}>Balanced</button>
        <button class="style-pill" class:active={stylePreset === 'detailed'} onclick={() => { stylePreset = 'detailed'; qComm = 'detailed'; }}>Detailed</button>
        <button class="style-pill" class:active={stylePreset === 'custom'} onclick={() => { stylePreset = 'custom'; qComm = 'balanced'; if (!customSoul) customSoul = soulPresets.balanced; }}>Custom</button>
      </div>
      <div class="style-desc">
        {#if stylePreset === 'concise'}
          Short and direct. Matches your energy -- quick question gets a quick answer. No extra info unless you ask.
        {:else if stylePreset === 'balanced'}
          Conversational but efficient. Gives enough detail to be useful without over-explaining.
        {:else if stylePreset === 'detailed'}
          Thorough and proactive. Anticipates follow-ups, provides context and examples. A collaborative partner.
        {:else}
          Write your own personality instructions. Define exactly how your AI should communicate.
        {/if}
      </div>
      {#if stylePreset === 'custom'}
        <textarea class="custom-textarea" bind:value={customSoul} rows="5" placeholder="Describe how your AI should communicate..."></textarea>
      {/if}
    </div>
    
    <div class="path-options">
      <button class="path-card" disabled={!aiConnected} onclick={() => screen = 'upload'}>
        <div class="path-title">Upload Documents</div>
        <div class="path-desc">Drop in files that describe you -- resume, goals doc, journal, anything.</div>
      </button>
      <button class="path-card" disabled={!aiConnected} onclick={() => screen = 'questions'}>
        <div class="path-title">Answer a Few Questions</div>
        <div class="path-desc">4 quick questions. Takes about 30 seconds.</div>
      </button>
      <button class="path-card" onclick={startChatting}>
        <div class="path-title">Just Start Chatting</div>
        <div class="path-desc">Start with a default AI personality. Refine later.</div>
      </button>

    </div>
  </div>

  {:else if screen === 'upload'}
  <div class="onboarding-center">
    <h1>Upload Documents</h1>
    <p class="upload-intro">
      Drop in any files that describe who you are and what you do.<br/>
      Your AI will read them and build your profile.
    </p>

    <button type="button" class="upload-help-link" onclick={handleUploadHelpClick}>
      What should I upload?
      <span class="upload-help-icon">
        <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
          <path d="M7 17 17 7"/><path d="M7 7h10v10"/>
        </svg>
      </span>
    </button>

    <div class="drop-zone" class:dragging={isDragging}
      ondragover={(e) => { e.preventDefault(); isDragging = true; }}
      ondragleave={() => isDragging = false}
      ondrop={handleDrop}
      onclick={() => document.getElementById('file-input')?.click()}
      role="button" tabindex="0">
      <div class="drop-text">Drop files here or click to browse</div>
      <div class="drop-formats">.txt .md</div>
      <input id="file-input" type="file" accept=".txt,.md" multiple hidden onchange={handleFileInput} />
    </div>
    
    {#if uploadedFiles.length > 0}
      <div class="file-list">
        {#each uploadedFiles as file, i}
          <div class="file-chip">
            <span>{file.name}</span>
            <button class="file-x" onclick={() => uploadedFiles = uploadedFiles.filter((_, j) => j !== i)}>✕</button>
          </div>
        {/each}
      </div>
    {/if}

    {#if processingError}
      <div class="connect-error-msg">{processingError}</div>
    {/if}

    <div class="nav-row">
      <button class="btn-ghost" onclick={() => { screen = 'main'; processingError = ''; }}>Back</button>
      <button class="btn-primary" onclick={processUpload} disabled={uploadedFiles.length === 0}>Process</button>
    </div>
  </div>

  {:else if screen === 'questions'}
  <div class="onboarding-center">
    <h1>Tell Me About Yourself</h1>
    <p class="subtitle">4 quick questions so your AI can get to know you.</p>
    
    <div class="questions-form">
      <div class="field">
        <label for="q-name">What's your name?</label>
        <input id="q-name" type="text" bind:value={qName} placeholder="First name or however you'd like to be called" />
      </div>
      <div class="field">
        <label for="q-do">What do you do and what matters to you?</label>
        <textarea class="q-textarea" id="q-do" bind:value={qDo} placeholder='e.g., "I run a marketing agency. Family and creative work matter most."' rows="4"></textarea>
      </div>
      <div class="field">
        <label for="q-working">What are you working on right now?</label>
        <textarea class="q-textarea" id="q-working" bind:value={qWorking} placeholder="Building an app / Planning a move / Growing a business" rows="4"></textarea>
      </div>
      <div class="field">
        <label for="q-extra">Anything else your AI should know? <span style="color: #52525B; font-weight: 400;">(Optional)</span></label>
        <textarea class="q-textarea" id="q-extra" bind:value={qExtra} placeholder="Goals, preferences, context..." rows="4"></textarea>
      </div>
    </div>

    {#if processingError}
      <div class="connect-error-msg">{processingError}</div>
    {/if}

    <div class="nav-row">
      <button class="btn-ghost" onclick={() => { screen = 'main'; processingError = ''; }}>Back</button>
      <button class="btn-primary" onclick={processQuestions} disabled={!qName.trim()}>Continue</button>
    </div>
  </div>

  {:else if screen === 'processing'}
  <div class="onboarding-center">
    <div class="processing">
      <h2>
        {#if processingMethod === 'upload'}
          Reading your documents...
        {:else if processingMethod === 'questions'}
          Setting up your agent...
        {:else}
          Getting ready...
        {/if}
      </h2>
      <div class="progress-bar"><div class="progress-fill"></div></div>
    </div>
  </div>

  {:else if screen === 'agent-names'}
  <div class="onboarding-center">
    <h1>Your Agents</h1>
    <p class="subtitle">We found these agents. Rename them if you'd like:</p>
    
    <div class="agent-cards">
      {#each ocDiscoveredAgents as agent}
        <div class="agent-card">
          {#if agent.runtime_type === 'openclaw' && agent.is_default}
            <div style="flex: 1;">
              <input
                type="text"
                class="agent-name-edit disabled"
                value={agent.name}
                disabled
                placeholder={agent.name}
              />
              <span class="linked-label">Linked to OpenClaw</span>
            </div>
            <span class="badge badge-teal" style="margin-left: 8px;">Main</span>
          {:else}
            <input
              type="text"
              class="agent-name-edit"
              value={editingAgentNames[agent.id] ?? agent.name}
              oninput={(e) => editingAgentNames[agent.id] = e.currentTarget.value}
              placeholder={agent.name}
            />
            {#if agent.is_default}
              <span class="badge badge-teal" style="margin-left: 8px;">Main</span>
            {/if}
          {/if}
        </div>
      {/each}
    </div>
    
    <div class="nav-row" style="margin-top: 24px;">
      <button class="btn-ghost" onclick={() => saveAndComplete('chat')}>Skip</button>
      <button class="btn-primary" onclick={proceedFromAgentNames}>Continue</button>
    </div>
  </div>

  {:else if screen === 'workshop'}
  <WorkshopReview
    bind:soulContent
    bind:memoryContent
    bind:memoryNoteId
    bind:soulNoteId
    scopeProjectId={WORKSHOP_SCOPE_ID}
    sessionId={workshopSessionId}
    initialAssistantMessage={workshopInitialMessage}
    onFinish={(messages) => saveAndComplete(uploadedFiles.length > 0 ? 'upload' : 'questions', messages)}
  />
  {/if}
</div>

{#if showRepairConfirm}
  <!-- D20 (#2649): replaces the native confirm() this screen previously passed as
       confirmCollision — confirm() never renders in Tauri's WebView (silently
       resolves false), so every re-pair with a detected collision silently declined.
       Shown unconditionally on every re-pair now (ruling #2), including zero
       collisions — copy branches on whether any were detected. -->
  <ConfirmDialog
    title="Re-pair OpenClaw"
    message={repairCollisions.length > 0
      ? `Switching to OpenClaw will overwrite ${repairCollisions.length} local agent file${repairCollisions.length === 1 ? '' : 's'} with the gateway's versions. Local edits will be lost.`
      : 'This will re-import your agents from the gateway. Continue?'}
    confirmLabel="Continue"
    onConfirm={confirmRepair}
    onCancel={cancelRepair}
  />
{/if}

<style>
  .onboarding-overlay {
    position: fixed; top: 0; left: 0; right: 0; bottom: 0;
    background: #111113; z-index: 9999;
    display: flex; align-items: center; justify-content: center;
  }
  .onboarding-center {
    max-width: 600px; width: 100%; padding: 40px 24px; position: relative;
  }
  h1 { font-size: 26px; font-weight: 600; color: #FAFAFA; margin: 0 0 6px; text-align: center; }
  h2 { font-size: 18px; font-weight: 600; color: #FAFAFA; margin: 0; display: inline; }
  .subtitle { font-size: 14px; color: #71717A; text-align: center; margin: 0 0 28px; }
  .subtitle-long { font-size: 14px; color: #A1A1AA; text-align: center; margin: 0 0 10px; line-height: 1.6; }
  .subtitle-small { font-size: 13px; color: #52525B; text-align: center; margin: 0 0 20px; }
  .upload-intro { font-size: 15px; color: #A1A1AA; text-align: center; margin: 0 0 24px; line-height: 1.7; }
  .upload-help-link {
    display: flex; align-items: center; justify-content: center; gap: 4px;
    margin: 0 auto 20px; background: transparent; border: 0; padding: 0; cursor: pointer;
    font-family: inherit; font-size: 13px; font-weight: 500; color: #00E4E4; transition: color 0.15s ease;
  }
  .upload-help-link:hover { color: #00D4D4; }
  .upload-help-icon { display: inline-flex; transition: transform 0.15s ease; }
  .upload-help-link:hover .upload-help-icon { transform: translate(1px, -1px); }
  .path-options { display: flex; flex-direction: column; gap: 8px; margin-bottom: 28px; }
  .connect-form { width: 100%; max-width: 400px; margin: 0 auto; }
  /* #2455 venice live-test v2 onboarding-picker ruling: flex-wrap + min-width added
     so a 5th card (Venice) never overflows the row — wraps to a second line on
     narrow widths instead of squeezing every card unreadably thin. */
  .connect-providers { display: flex; flex-wrap: wrap; gap: 8px; margin-bottom: 20px; }
  /* #2792 card-height ruling: fixed uniform height (title + one descriptor line +
     padding, ~half the prior variable-height card) across every provider-card
     instance in this file (Connect Your AI §6.2 AND Connect Smart Features §6.4.1
     use this same class/height) — see docs/spec/onboarding.md's #2792 note. Name
     and descriptor both truncate with an ellipsis rather than wrap, which is what
     keeps every card the same height regardless of provider-name/descriptor length. */
  .provider-card {
    flex: 1 1 100px; min-width: 100px; height: 52px; padding: 8px 10px; background: #1E1E22; border: 1px solid #27272A;
    border-radius: 4px; cursor: pointer; text-align: center; transition: all 0.15s;
    display: flex; flex-direction: column; justify-content: center; overflow: hidden;
  }
  .provider-card:hover { border-color: #3F3F46; }
  .provider-card.active { border-color: #00E4E4; background: rgba(0,228,228,0.05); }
  .provider-name { display: block; font-size: 14px; font-weight: 600; color: #FAFAFA; margin-bottom: 2px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .provider-desc { display: block; font-size: 11px; color: #71717A; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .connect-field { margin-bottom: 16px; }
  .connect-field label { display: block; font-size: 12px; color: #71717A; margin-bottom: 6px; }
  .connect-field input { 
    width: 100%; padding: 10px 14px; background: #1E1E22; border: 1px solid #27272A; 
    border-radius: 4px; color: #FAFAFA; font-size: 14px; font-family: 'Inter', sans-serif;
  }
  .connect-field input:focus { border-color: #00E4E4; outline: none; }
  .connect-actions { margin-bottom: 16px; }
  .connect-result { padding: 10px 14px; border-radius: 4px; font-size: 13px; margin-bottom: 12px; }
  .connect-result.success { background: rgba(16,185,129,0.1); color: #10B981; border: 1px solid rgba(16,185,129,0.3); }
  .connect-result.error { background: rgba(239,68,68,0.1); color: #EF4444; border: 1px solid rgba(239,68,68,0.3); }
  .connect-continue { width: 100%; }
  .test-row { display: flex; align-items: center; gap: 12px; }
  .connect-tag {
    font-size: 13px; font-weight: 500; padding: 4px 12px; border-radius: 4px;
  }
  .connect-tag.success { color: #10B981; background: rgba(16,185,129,0.1); border: 1px solid rgba(16,185,129,0.3); }
  .connect-tag.error { color: #EF4444; background: rgba(239,68,68,0.1); border: 1px solid rgba(239,68,68,0.3); }
  .connect-error-msg { color: #EF4444; font-size: 13px; margin-top: 12px; margin-bottom: 4px; }
  /* D20 (#2649): dismissable, non-error notice for a declined re-pair confirmation.
     Deliberately distinct from .connect-error-msg — neutral colour, no red — since
     a decline is a user choice, not a failure. */
  .connect-repair-declined { color: #A1A1AA; font-size: 13px; text-align: center; margin-top: 12px; margin-bottom: 4px; }
  .connect-error-action { margin-top: 6px; margin-bottom: 8px; font-size: 13px; }
  .connect-error-action a { color: #00E4E4; text-decoration: none; }
  .connect-error-action a:hover { text-decoration: underline; }
  .sync-info-banner {
    background: rgba(59,130,246,0.1); border: 1px solid rgba(59,130,246,0.3);
    border-radius: 4px; padding: 16px; margin-top: 16px;
  }
  .sync-info-banner .info-msg {
    font-size: 13px; color: #3B82F6; margin: 0; line-height: 1.5;
  }
  .sync-error-banner {
    background: rgba(239,68,68,0.1); border: 1px solid rgba(239,68,68,0.3);
    border-radius: 4px; padding: 16px; margin-top: 16px;
  }
  .sync-error-banner .error-title {
    font-size: 14px; font-weight: 600; color: #EF4444; margin: 0 0 8px;
  }
  .sync-error-banner .error-msg {
    font-size: 13px; color: #EF4444; margin: 0 0 12px; line-height: 1.5;
  }
  .sync-error-banner .error-actions {
    display: flex; gap: 8px;
  }
  .skip-row { display: flex; justify-content: center; width: 100%; max-width: 480px; margin: 0 auto; }
  .btn-skip-connect { 
    background: none; border: none; color: #52525B; font-size: 13px; cursor: pointer; 
    margin-top: 24px; font-family: 'Inter', sans-serif;
  }
  .btn-skip-connect:hover { color: #71717A; }
  
  /* Restore offer styles */
  .restore-actions {
    display: flex; flex-direction: column; gap: 12px; 
    width: 100%; max-width: 400px; margin: 0 auto 24px;
  }
  .restore-error, .restore-confirm {
    background: rgba(239,68,68,0.1); border: 1px solid rgba(239,68,68,0.3);
    border-radius: 4px; padding: 20px; margin-top: 20px;
    max-width: 400px; margin-left: auto; margin-right: auto;
  }
  .restore-confirm {
    background: rgba(59,130,246,0.1); border: 1px solid rgba(59,130,246,0.3);
  }
  .restore-error p, .restore-confirm p {
    font-size: 14px; color: #FAFAFA; margin: 0 0 16px; line-height: 1.5;
  }
  .restore-error-actions, .restore-confirm-actions {
    display: flex; gap: 8px;
  }
  .btn-ghost {
    background: transparent; border: 1px solid #27272A; color: #A1A1AA;
    padding: 12px 20px; border-radius: 4px; font-size: 14px; font-weight: 500;
    cursor: pointer; font-family: 'Inter', sans-serif; transition: all 0.15s;
  }
  .btn-ghost:hover { border-color: #3F3F46; color: #FAFAFA; }
  .btn-ghost:disabled { opacity: 0.5; cursor: not-allowed; }
  .disabled-path { opacity: 0.35; cursor: not-allowed !important; }
  .disabled-path:hover { border-color: #27272A !important; background: #1E1E22 !important; }
  .path-card {
    display: block; width: 100%; text-align: left; padding: 18px 22px;
    background: #18181B; border: 1px solid rgba(255,255,255,0.06);
    border-radius: 4px; cursor: pointer; transition: all 0.15s;
  }
  .path-card:hover { background: #1E1E22; border-color: rgba(255,255,255,0.12); }
    .path-card:disabled { opacity: 0.35; cursor: not-allowed; }
  /* #2792: the .connect-providers/.provider-card/.provider-name/.provider-desc
     rules that previously lived here were an exact-selector duplicate of the
     ones defined earlier in this stylesheet (~L2327), left over from a prior
     merge. Being later in source order, they silently won the cascade on every
     provider-card instance in the file (both Connect Your AI and Connect Smart
     Features), overriding the intended flex-wrap row layout with a full-width
     flex-column block and no fixed height — the actual root cause of the
     oversized/variable-height cards reported in #2792. The removed block's
     `selected`-suffixed provider-card state selector was additionally dead
     code: no markup in this file ever applies a `selected` class
     (`class:active` is what every card button uses). Removed rather than
     merged — the earlier block is the one real markup instances style. See
     docs/spec/onboarding.md's #2792 note. */
  .connect-field { width: 100%; max-width: 440px; margin-bottom: 16px; }
  .connect-field label { display: block; font-size: 13px; color: #A1A1AA; margin-bottom: 6px; }
  .connect-field input {
    width: 100%; padding: 10px 14px; background: #18181B; border: 1px solid #27272A;
    border-radius: 4px; color: #FAFAFA; font-size: 14px; font-family: 'Inter', sans-serif;
    box-sizing: border-box;
  }
  .connect-field input:focus { outline: none; border-color: #00E4E4; }
  .connect-error { color: #EF4444; font-size: 13px; margin-bottom: 12px; max-width: 440px; }
  .connect-success { color: #10B981; font-size: 15px; font-weight: 600; margin-bottom: 12px; }
  .connect-actions { display: flex; gap: 12px; align-items: center; }
  .path-card.dim { opacity: 0.5; }
  .path-card.dim:hover { opacity: 0.7; }
  .path-title { font-size: 15px; font-weight: 600; color: #FAFAFA; margin-bottom: 2px; }
  .path-desc { font-size: 13px; color: #71717A; }
  .agent-cards { display: flex; flex-direction: column; gap: 12px; margin: 20px 0; }
  .agent-card { 
    display: flex; align-items: center; padding: 14px 18px; background: #18181B; 
    border: 1px solid rgba(255,255,255,0.06); border-radius: 4px;
  }
  .agent-name-edit {
    flex: 1; padding: 8px 12px; background: rgba(255,255,255,0.05); 
    border: 1px solid rgba(255,255,255,0.1); border-radius: 4px; 
    color: #FAFAFA; font-size: 14px; font-family: 'Inter', sans-serif;
  }
  .agent-name-edit:focus { outline: none; border-color: #00E4E4; }
  .agent-name-edit.disabled {
    opacity: 0.6; cursor: not-allowed; background: rgba(255,255,255,0.02);
    color: #A1A1AA;
  }
  .linked-label {
    display: block; font-size: 11px; color: #71717A; margin-top: 4px;
    font-style: italic;
  }
  .badge { font-size: 11px; padding: 4px 8px; border-radius: 3px; font-weight: 600; }
  .badge-teal { background: rgba(0,228,228,0.1); color: #00E4E4; }
  .style-section { border-top: 1px solid #27272A; padding-top: 18px; margin-top: 20px; }
  .style-label { font-size: 16px; font-weight: 600; color: #FAFAFA; margin-bottom: 12px; }
  .style-pills { display: flex; gap: 10px; margin-bottom: 10px; }
  .style-pill {
    padding: 8px 18px; border-radius: 4px; border: 1px solid rgba(255,255,255,0.06);
    background: #1E1E22; color: #71717A; font-size: 14px; cursor: pointer; transition: all 0.15s;
  }
  .style-pill:hover { background: #27272A; color: #FAFAFA; }
  .style-pill.active { background: transparent; border-color: #00E4E4; color: #00E4E4; }
  .style-desc { font-size: 14px; color: #A1A1AA; line-height: 1.5; margin-bottom: 10px; }
  .q-textarea {
    width: 100%; padding: 10px 14px; background: #1E1E22; border: 1px solid #27272A;
    border-radius: 4px; color: #FAFAFA; font-size: 14px; font-family: 'Inter', sans-serif;
    resize: vertical; min-height: 80px; box-sizing: border-box;
  }
  .q-textarea:focus { border-color: #00E4E4; outline: none; }
  .q-textarea::placeholder { color: #52525B; }
  .custom-textarea {
    width: 100%; background: #18181B; border: 1px solid #27272A; border-radius: 4px;
    color: #FAFAFA; font-family: 'Inter', sans-serif; font-size: 12px;
    padding: 10px 12px; resize: vertical; line-height: 1.5; min-height: 100px; max-height: 300px;
  }
  .custom-textarea:focus { outline: none; border-color: #00E4E4; }
  .drop-zone {
    border: 2px dashed #27272A; border-radius: 4px; padding: 36px 24px;
    text-align: center; cursor: pointer; transition: all 0.15s; margin-bottom: 12px;
  }
  .drop-zone:hover, .drop-zone.dragging { border-color: #00E4E4; background: rgba(0,228,228,0.02); }
  .drop-text { font-size: 13px; color: #71717A; margin-bottom: 6px; }
  .drop-formats { font-size: 11px; color: #52525B; }
  .file-list { display: flex; flex-wrap: wrap; gap: 6px; margin-bottom: 12px; }
  .file-chip {
    display: flex; align-items: center; gap: 6px; padding: 3px 10px;
    background: #27272A; border-radius: 4px; font-size: 11px; color: #FAFAFA;
  }
  .file-x { background: none; border: none; color: #52525B; cursor: pointer; font-size: 10px; padding: 0; }
  .file-x:hover { color: #EF4444; }
  .questions-form { display: flex; flex-direction: column; gap: 16px; margin-bottom: 20px; }
  .field label { display: block; font-size: 12px; font-weight: 500; color: #FAFAFA; margin-bottom: 5px; }
  .field input {
    width: 100%; padding: 9px 12px; background: #18181B; border: 1px solid #27272A;
    border-radius: 4px; color: #FAFAFA; font-family: 'Inter', sans-serif; font-size: 12px;
  }
  .field input:focus { outline: none; border-color: #00E4E4; }
  .field input::placeholder { color: #52525B; }
  .nav-row { display: flex; justify-content: space-between; margin-top: 20px; }
  .btn-ghost {
    padding: 7px 18px; border-radius: 4px; border: 1px solid #27272A;
    background: transparent; color: #71717A; font-size: 12px; cursor: pointer;
  }
  .btn-ghost:hover { background: #1E1E22; color: #FAFAFA; }
  .btn-primary {
    padding: 7px 18px; border-radius: 4px; border: none;
    background: #00E4E4; color: #111113; font-size: 12px; font-weight: 600; cursor: pointer;
  }
  .btn-primary:hover { background: #00C8C8; }
  .btn-primary:disabled { opacity: 0.4; cursor: not-allowed; }
  .processing { text-align: center; padding: 60px 0; }
  .processing h2 { display: block; font-size: 16px; color: #FAFAFA; }
  .progress-bar { width: 200px; height: 3px; background: #27272A; border-radius: 2px; margin: 20px auto; overflow: hidden; }
  .progress-fill { height: 100%; width: 60%; background: #00E4E4; border-radius: 2px; animation: pulse 1.5s ease-in-out infinite; }
  @keyframes pulse { 0%, 100% { width: 20%; margin-left: 0; } 50% { width: 60%; margin-left: 20%; } }

  /* Workshop styles extracted to WorkshopReview.svelte (#2288 PR 1). */
  .config-guidance {
    background: #18181B; border: 1px solid #27272A; border-radius: 4px;
    padding: 14px 18px; max-width: 480px; margin: 0 auto; text-align: left;
  }
  .config-guidance code {
    font-family: 'SF Mono', 'Fira Code', monospace; font-size: 13px; color: #00E4E4;
    word-break: break-all;
  }
  .agent-list { display: flex; flex-direction: column; gap: 8px; max-width: 400px; margin: 0 auto; }

  /* Model selection cards */
  .model-cards {
    display: flex; flex-direction: column; gap: 8px;
    max-width: 440px; margin: 0 auto;
  }
  .model-card {
    display: flex; flex-direction: column; gap: 4px; text-align: left;
    padding: 14px 18px; background: rgba(255, 255, 255, 0.04);
    border: 1px solid rgba(255, 255, 255, 0.06);
    border-radius: 4px; cursor: pointer; transition: border-color 0.15s;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    color: #e0e0e0;
  }
  .model-card:hover { border-color: rgba(255, 255, 255, 0.15); }
  .model-card.active { border-color: #00E4E4; background: rgba(0, 228, 228, 0.05); }
  .model-card-top { display: flex; align-items: center; gap: 8px; }
  .model-card-name { font-size: 14px; font-weight: 600; color: #e0e0e0; }
  .model-card-desc { font-size: 12px; color: #888; }
  .model-card-cost { font-size: 10px; color: rgba(255,255,255,0.25); }
  .model-badge {
    background: rgba(0, 228, 228, 0.15); color: #00E4E4;
    font-size: 10px; padding: 2px 8px; border-radius: 10px;
    font-weight: 500;
  }

  /* Home Setup */
  .onboarding-overlay-scroll {
    position: fixed; top: 0; left: 0; right: 0; bottom: 0;
    background: #111113; z-index: 9999;
    overflow-y: auto; display: flex; justify-content: center;
    padding: 40px 24px;
  }
  .home-setup-center {
    max-width: 560px; width: 100%;
  }
  .hs-card {
    background: rgba(255,255,255,0.05);
    border-radius: 6px;
    padding: 20px 24px;
    margin-bottom: 16px;
  }
  .hs-label {
    display: block;
    font-size: 14px;
    color: #e0e0e0;
    font-weight: 600;
    margin-bottom: 8px;
  }
  .hs-input {
    background: rgba(255,255,255,0.05);
    border: 1px solid rgba(255,255,255,0.1);
    border-radius: 4px;
    padding: 10px 14px;
    color: #e0e0e0;
    font-size: 14px;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    outline: none;
    width: 100%;
    box-sizing: border-box;
    resize: none;
  }
  .hs-input:focus {
    border-color: rgba(0, 228, 228, 0.4);
  }
  .hs-input::placeholder {
    color: rgba(255,255,255,0.25);
  }
  .hs-add {
    background: none;
    border: none;
    font-size: 12px;
    color: #00E4E4;
    cursor: pointer;
    padding: 0;
    margin-top: 8px;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }
  .hs-add:hover {
    text-decoration: underline;
  }
  .section-dismiss {
    position: absolute;
    top: 8px;
    right: 8px;
    background: none;
    border: none;
    color: rgba(255,255,255,0.3);
    font-size: 16px;
    cursor: pointer;
    padding: 4px 8px;
    line-height: 1;
  }
  .section-dismiss:hover { color: rgba(255,255,255,0.6); }
  .row-delete {
    background: none;
    border: none;
    color: rgba(255,255,255,0.2);
    font-size: 14px;
    cursor: pointer;
    padding: 4px 8px;
    flex-shrink: 0;
  }
  .row-delete:hover { color: rgba(255,68,68,0.6); }
</style>

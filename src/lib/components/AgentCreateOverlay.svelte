<!--
  AgentCreateOverlay.svelte — Full-window agent create flow (#2030 PR B)
  Spec: §24.10, §24.10.1, ADR-0059 D4
  Pattern: Reuses onboarding-overlay / onboarding-center structure (Onboarding.svelte)

  Steps:
    0 — Provider / connectivity (forced-create path only, when no saved config)
    R — Runtime picker: choose which connected provider owns the new agent (voluntary path only, #2522 Stage 6b)
    M — Model picker: choose a model for the chosen runtime (voluntary path only, #2522 Stage 6b)
    1 — Name the agent (always shown)
    2 — Seeding chooser (after name-commit)

  Entry points:
    - voluntary (Settings → "+ New agent"): Step R → Step M → Step 1
    - forced (provider switch, no saved config): Step 0 → Step 1 → Step 2
    - forced (provider switch, saved config): Step 1 only (pendingProvider pre-populated)
    - picker modal "Create new": Step R → Step M → Step 1

  #2522 Stage 6b (README-group-b.md §8 "New Agent — provider, model, name"):
    Forced-create (provider switch) already resolves its runtime via Step 0 — the
    just-tested `pendingProvider` IS the runtime, so Steps R/M are skipped there by
    design (re-showing a picker would contradict "every agent permanently belongs
    to one provider", the same ruling that removes any change-provider control).
    Model selection for the forced path stays optional/undefined, same as before
    6b — the freshly-probed provider isn't persisted yet (Settings.svelte defers
    ai_configure_and_save until after agent creation), so `fetch_available_models`
    (which reads persisted credentials) can't list its models yet. That's an
    existing backend gap, not something this stage's scope covers.

  ADR-0059 D4: Name-commit = transaction boundary.
    Pressing CTA on Step 1 calls create_local_agent (Rust).
    Cancellation before name-commit = clean no-op (nothing written).
    Cancellation after name-commit = seeding is optional; agent already live.

  CSS: All tokens from app.css; no hardcoded colors or raw px radii.
-->
<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { createLocalAgent, checkNameCollision, listRuntimeConnections, type RuntimeConnectionInfo } from '$lib/api';
  import { openExternalUrl } from '$lib/utils/externalLink';
  import type { Agent } from '$lib/types';
  import { WORKSHOP_SCOPE_ID } from '$lib/constants';
  import { createWorkshopSession } from '$lib/onboarding/workshopSession';
  import {
    streamAndCollect,
    parseSoulAndMemory,
    buildUploadExtractionPrompt,
    buildInterviewExtractionPrompt,
  } from '$lib/onboarding/seedGeneration';
  import WorkshopReview from './WorkshopReview.svelte';
  import SearchableModelSelect from './SearchableModelSelect.svelte';

  // ── Props ────────────────────────────────────────────────────────────────
  /**
   * Whether this is a forced-create from a provider switch.
   * When true and no saved provider config: show Step 0 first.
   */
  let {
    forcedCreate = false,
    pendingProvider = '',   // provider the user is switching TO
    // #2268 fold-in: credentials from Settings.svelte's confirmCascadeAndSave()
    // pendingProviderSave, threaded through so a probe that already succeeded
    // this session doesn't need to be re-entered/re-tested (see Step 0 skip below).
    pendingApiKey = undefined,
    pendingEndpoint = undefined,
    pendingModel = undefined,
    onClose,               // () => void  — dismiss overlay
    onAgentCreated,        // (agent: Agent) => void — called after name-commit
  }: {
    forcedCreate?: boolean;
    pendingProvider?: string;
    pendingApiKey?: string;
    pendingEndpoint?: string;
    pendingModel?: string;
    onClose: () => void;
    onAgentCreated?: (agent: Agent) => void;
  } = $props();

  // #2268 fold-in: a prior successful probe this session means we already have
  // valid credentials for `pendingProvider` (ollama uses pendingEndpoint as its
  // "key" field, matching Settings.svelte's pendingProviderSave.endpoint
  // convention). In that case Step 0 (API key entry) is redundant — go straight
  // to Step 1 (name/role).
  const hasValidPendingCredentials = Boolean(
    pendingProvider === 'ollama' ? pendingEndpoint?.trim() : pendingApiKey?.trim()
  );

  // ── Step state ───────────────────────────────────────────────────────────
  // 'provider' | 'runtime' | 'model' | 'name' | 'seeding'
  // #2522 Stage 6b: the voluntary path (forcedCreate=false) now starts at 'runtime'
  // instead of jumping straight to 'name' — the real runtime + model picker replaces
  // 6a's auto-resolve seam. The forced path (provider switch) is unchanged: Step 0
  // already IS the runtime choice for that entry point (see module comment above),
  // so it goes straight from 'provider'/'name' without visiting 'runtime'/'model'.
  let step = $state<'provider' | 'runtime' | 'model' | 'name' | 'seeding'>(
    forcedCreate ? (hasValidPendingCredentials ? 'name' : 'provider') : 'runtime'
  );

  // ── Step 0: Provider ─────────────────────────────────────────────────────
  let selectedProvider = $state(pendingProvider || 'openrouter');
  // #2268 fold-in: pre-fill from a prior successful probe instead of ''.
  // Ollama's "key" field is really the endpoint (see agent-conn-input above).
  let apiKey = $state(
    (pendingProvider === 'ollama' ? pendingEndpoint : pendingApiKey) || ''
  );
  // §28.1 invariant: status defaults to 'not-tested'; only goes 'success' after real probe.
  // #2268 fold-in: if we're skipping Step 0 because of a prior successful probe,
  // reflect that as already-successful so re-entering Step 0 (e.g. via Back) doesn't
  // look untested.
  let connectionStatus = $state<'not-tested' | 'testing' | 'success' | 'error'>(
    hasValidPendingCredentials ? 'success' : 'not-tested'
  );
  let connectionMessage = $state(hasValidPendingCredentials ? 'Connected' : '');

  // ── Step R: Runtime picker (#2522 Stage 6b) ─────────────────────────────
  // Design ruling (README-group-b.md "The model these surfaces express" #2):
  // "Every agent permanently belongs to one provider. No change-provider control
  // anywhere, by design." — this picker is the ONE place that choice gets made;
  // there is no equivalent control anywhere else in the app.
  const RUNTIME_LABELS: Record<string, string> = {
    openrouter: 'OpenRouter',
    anthropic: 'Anthropic',
    ollama: 'Ollama',
    venice: 'Venice',
    openclaw: 'OpenClaw',
  };
  let runtimeOptions = $state<RuntimeConnectionInfo[]>([]);
  let loadingRuntimes = $state(false);
  let runtimeLoadError = $state('');
  let selectedRuntimeType = $state('');

  // ── Step M: Model picker (#2522 Stage 6b) ────────────────────────────────
  let modelOptions = $state<Array<{ id: string; name: string; provider: string }>>([]);
  let loadingModels = $state(false);
  let modelLoadError = $state('');
  let selectedModel = $state<string | undefined>(undefined);

  /** Load connected, non-openclaw runtimes for Step R. */
  async function loadRuntimeOptions() {
    loadingRuntimes = true;
    runtimeLoadError = '';
    try {
      const connections = await listRuntimeConnections();
      // Design ruling: exclude openclaw — it has no local-agent creation UI of its
      // own (matches the seam comment `resolveRuntimeTypeForCreate` used to carry).
      runtimeOptions = connections.filter(
        (c) => c.connected && c.runtime_type !== 'openclaw'
      );
    } catch (e: any) {
      console.error('[AGENT-CREATE] Failed to load runtime connections:', e);
      runtimeLoadError = e?.message || String(e);
      runtimeOptions = [];
    } finally {
      loadingRuntimes = false;
    }
  }

  /** User picked a runtime on Step R — advance to Step M and fetch its models. */
  async function handlePickRuntime(runtimeType: string) {
    selectedRuntimeType = runtimeType;
    selectedModel = undefined;
    step = 'model';
    await loadModelsForRuntime(runtimeType);
  }

  /** Fetch models for the chosen runtime. `fetch_available_models` returns models for
   *  every connected runtime merged together (provider-namespaced), so filter down to
   *  the one the user just picked — same command Settings.svelte's AI tab already uses. */
  async function loadModelsForRuntime(runtimeType: string) {
    loadingModels = true;
    modelLoadError = '';
    try {
      const models = await invoke<Array<{ id: string; name: string; provider: string }>>(
        'fetch_available_models'
      );
      modelOptions = models.filter((m) => m.provider === runtimeType);
      if (modelOptions.length === 0) {
        modelLoadError = `No models available for ${RUNTIME_LABELS[runtimeType] ?? runtimeType}.`;
      }
    } catch (e: any) {
      console.error('[AGENT-CREATE] Failed to load models for', runtimeType, e);
      modelLoadError = e?.message || String(e);
      modelOptions = [];
    } finally {
      loadingModels = false;
    }
  }

  function handlePickModel(modelId: string) {
    selectedModel = modelId;
    step = 'name';
  }

  function handleSkipModel() {
    selectedModel = undefined;
    step = 'name';
  }

  // Kick off Step R's fetch immediately on mount when the overlay opens straight into
  // it (voluntary path) — mirrors the top-level-init pattern other overlays use instead
  // of onMount for a single fire-once async load (Svelte 5 <script> body runs once).
  if (step === 'runtime') {
    loadRuntimeOptions();
  }

  // ── Step 1: Name ─────────────────────────────────────────────────────────
  let agentName = $state('');
  let nameError = $state('');       // '' | 'collision' | 'empty'
  let collisionDebounceTimer: ReturnType<typeof setTimeout> | null = null;
  let checkingCollision = $state(false);
  let creating = $state(false);
  let createError = $state('');

  // ── Step 2: Seeding ──────────────────────────────────────────────────────
  let createdAgent = $state<Agent | null>(null);

  // Seeding sub-step: which card the user picked
  let seedingPath = $state<'none' | 'upload' | 'interview' | 'review'>('none');

  // Upload path
  let isDragging = $state(false);
  let uploadedFiles = $state<Array<{ name: string; content: string }>>([]);
  let uploading = $state(false);
  let uploadError = $state('');

  // Interview path — 5 questions covering specialization, tone, topics, audience, use-case
  const INTERVIEW_QUESTIONS = [
    { id: 'specialization', label: 'What should this agent specialize in?', placeholder: 'e.g. research, writing, coding, customer support…' },
    { id: 'tone',           label: 'What tone should it use?',              placeholder: 'e.g. concise and direct, warm and encouraging, professional…' },
    { id: 'topics',         label: 'What topics or domains should it know about?', placeholder: 'e.g. machine learning, tax law, cooking, SaaS sales…' },
    { id: 'audience',       label: 'Who will mainly be using it?',          placeholder: 'e.g. just me, a small team, customers, students…' },
    { id: 'usecase',        label: 'Describe a typical task for this agent.', placeholder: 'e.g. draft blog posts, summarize meetings, answer support tickets…' },
  ];
  let interviewIdx = $state(0);          // current question index
  let interviewAnswers = $state<string[]>(INTERVIEW_QUESTIONS.map(() => ''));
  let interviewSubmitting = $state(false);
  let interviewError = $state('');

  // ── Step 2 (generation): shared seed pipeline state (#2288 PR 3, §24.10.1c) ─
  // Replaces the broken seed_agent_uploads/seed_agent_interview Rust commands
  // (raw file-list/Q&A dumps, no model call) with the same generate-never-
  // transcribe pipeline onboarding uses (seedGeneration.ts). After generation,
  // seedingPath transitions to 'review' to mount WorkshopReview (PR 1) for the
  // same Memory→Soul review step onboarding gets.
  let soulContent = $state('');
  let memoryContent = $state('');
  let memoryNoteId = $state('');
  let soulNoteId = $state('');
  let workshopInitialMessage = $state('');
  // #2618: real chat_sessions row id for WorkshopReview's ai_chat_stream call,
  // resolved via createWorkshopSession() right before seedingPath = 'review'.
  // See src/lib/onboarding/workshopSession.ts for the full rationale.
  let workshopSessionId = $state('');

  // ── Derived ──────────────────────────────────────────────────────────────
  const slugPreview = $derived(
    agentName.trim().toLowerCase().replace(/\s+/g, '-').replace(/[^a-z0-9-]/g, '')
  );
  const hasCollision = $derived(nameError === 'collision');
  const ctaLabel = $derived((): string => {
    if (hasCollision) return 'Enter a different name';
    if (!agentName.trim()) return 'Name your agent to continue';
    // Forced path: indicate provider switch
    if (forcedCreate && step === 'name' && connectionStatus !== 'success' && pendingProvider) {
      return `Create ${agentName.trim()} & switch to ${selectedProvider}`;
    }
    return `Create ${agentName.trim()}`;
  });
  const ctaDisabled = $derived(
    hasCollision || !agentName.trim() || checkingCollision || creating
  );

  // Provider-step CTA: disabled until probe succeeds (§28.1 / §28.2)
  const providerCtaDisabled = $derived(
    connectionStatus !== 'success' || !apiKey.trim()
  );

  // Step indicator: forced path shows 3 dots (provider, name, seeding); voluntary
  // shows 4 (runtime, model, name, seeding) — #2522 Stage 6b adds the runtime + model
  // steps to the voluntary/picker-modal entry points only (see module comment).
  const stepCount = $derived(forcedCreate ? 3 : 4);
  const activeStepIdx = $derived((): number => {
    if (forcedCreate) {
      return step === 'provider' ? 0 : step === 'name' ? 1 : 2;
    }
    switch (step) {
      case 'runtime': return 0;
      case 'model': return 1;
      case 'name': return 2;
      default: return 3; // 'seeding'
    }
  });

  // ── Step 0: Provider probe + auto-advance ───────────────────────────────────
  // Bug 3 fix: single CTA tests the connection and advances to Step 1 on success
  async function handleTestProvider() {
    if (!apiKey.trim()) {
      connectionMessage = 'API key required';
      connectionStatus = 'error';
      return;
    }
    connectionStatus = 'testing';
    connectionMessage = '';
    try {
      if (selectedProvider === 'openrouter') {
        await invoke('validate_openrouter_key', { apiKey });
      } else if (selectedProvider === 'anthropic') {
        await invoke('validate_anthropic_key', { apiKey });
      } else if (selectedProvider === 'venice') {
        // F4 fix (#2455 venice live-test v2): venice was entirely absent from this
        // dispatch, even though the provider button above and `resolveRuntimeTypeForCreate`
        // both already accept it as a valid `selectedProvider` value.
        await invoke('validate_venice_key', { apiKey });
      } else if (selectedProvider === 'ollama') {
        const ok = await invoke<boolean>('ollama_probe_two_step', { endpoint: apiKey });
        if (!ok) throw new Error('Ollama not reachable at that endpoint');
      }
      connectionStatus = 'success';
      connectionMessage = 'Connected';
      // §28.2: advance to Step 1 after a short visual confirmation window
      await new Promise(resolve => setTimeout(resolve, 600));
      step = 'name';
    } catch (e: any) {
      connectionStatus = 'error';
      connectionMessage = e?.message || String(e);
    }
  }

  // ── Step 1: Name collision ────────────────────────────────────────────────
  function handleNameInput() {
    nameError = '';
    if (collisionDebounceTimer) clearTimeout(collisionDebounceTimer);
    if (!agentName.trim()) return;
    collisionDebounceTimer = setTimeout(async () => {
      checkingCollision = true;
      try {
        const taken = await checkNameCollision(agentName.trim());
        if (taken) nameError = 'collision';
      } catch {
        // Swallow — create_local_agent will also check via UNIQUE constraint
      } finally {
        checkingCollision = false;
      }
    }, 300);
  }

  // ── Step 1: Create agent (name-commit = D4 transaction boundary) ──────────
  // #2522 Stage 6b: resolve the runtime_type synchronously from picker state instead
  // of re-querying listRuntimeConnections here (6a's seam). The voluntary path already
  // picked + validated its runtime on Step R — the picker only ever renders connected
  // runtimes (loadRuntimeOptions filters on `connected === true`), and clicking a card
  // is both the selection and the advance, so `selectedRuntimeType` is always one of
  // those connected options by the time Step 1's CTA can even be reached. The forced
  // path (provider switch) never visits Step R/M at all: Step 0 IS its runtime choice,
  // so it uses the just-tested `pendingProvider` directly (module comment above
  // explains why re-showing a picker there would contradict the "agent permanently
  // belongs to one provider" rule). 'openrouter' fallback only exists to match
  // resolve_runtime_type_for_new_local_agent's own default (agents.rs) in the
  // defensive case both are somehow empty.
  function resolveRuntimeTypeForCreate(): string {
    if (forcedCreate) {
      return pendingProvider || 'openrouter';
    }
    return selectedRuntimeType || 'openrouter';
  }

  async function handleCreate() {
    if (ctaDisabled) return;

    creating = true;
    createError = '';
    try {
      const runtimeType = resolveRuntimeTypeForCreate();
      const agent = await createLocalAgent({
        name: agentName.trim(),
        model: forcedCreate ? undefined : selectedModel,
        description: undefined,
        runtime_type: runtimeType,
      });

      createdAgent = agent;
      onAgentCreated?.(agent);
      // §24.7B: dispatch agent-switched so chat panel refreshes
      window.dispatchEvent(new CustomEvent('pane-agent-switched'));
      window.dispatchEvent(new CustomEvent('pane-agent-created', { detail: { agent } }));
      step = 'seeding';
    } catch (e: any) {
      const msg = e?.message || String(e);
      if (msg.includes('already exists')) {
        nameError = 'collision';
      } else {
        createError = msg;
      }
    } finally {
      creating = false;
    }
  }

  // ── Step 2: Seeding paths ─────────────────────────────────────────────────
  function handleJustChat() {
    // ADR-0059 D4: seeding after name-commit is optional; agent is live
    onClose();
  }

  function handlePickUpload() {
    seedingPath = 'upload';
  }

  function handlePickInterview() {
    seedingPath = 'interview';
    interviewIdx = 0;
    interviewAnswers = INTERVIEW_QUESTIONS.map(() => '');
    interviewError = '';
  }

  // Upload path — drag/drop + file picker
  function handleDrop(e: DragEvent) {
    e.preventDefault();
    isDragging = false;
    const files = e.dataTransfer?.files;
    if (!files) return;
    for (const file of Array.from(files)) {
      if (file.name.endsWith('.txt') || file.name.endsWith('.md')) {
        file.text().then(text => {
          if (text.trim()) uploadedFiles = [...uploadedFiles, { name: file.name, content: text }];
        });
      }
    }
  }

  function handleFileInput(e: Event) {
    const input = e.target as HTMLInputElement;
    if (!input.files) return;
    for (const file of Array.from(input.files)) {
      if (file.name.endsWith('.txt') || file.name.endsWith('.md')) {
        file.text().then(text => {
          if (text.trim()) uploadedFiles = [...uploadedFiles, { name: file.name, content: text }];
        });
      }
    }
    // Reset input so same file can be re-added after removal
    input.value = '';
  }

  // #2311: "What should I upload?" help link — opens in system browser, not in-app WebView.
  async function handleUploadHelpClick() {
    await openExternalUrl('https://paneapp.ai/system-files-creation', 'AGENT-CREATE');
  }

  // §24.10.1c: fetch the same authoritative default-soul fallback text
  // onboarding uses (ai_get_default_soul / DEFAULT_SOUL_FALLBACK) rather than
  // hardcoding a second copy here.
  async function fetchDefaultSoulPreset(): Promise<string> {
    try {
      return await invoke('ai_get_default_soul') as string;
    } catch (e) {
      console.error('[AGENT-CREATE] Failed to fetch default soul preset:', e);
      return "Be helpful and conversational. Match the user's energy and length.";
    }
  }

  // Writes to .pane/system/{agent}/ — same mechanism onboarding's ensureAgentFile
  // uses (write_system_file), so both callers share the same file-write path.
  async function writeAgentFile(filename: string, content: string): Promise<void> {
    if (!createdAgent) return;
    await invoke('write_system_file', {
      filename,
      content,
      agentId: createdAgent.id,
    });
    console.log(`[AGENT-CREATE] Wrote ${filename} for agent ${createdAgent.id}`);
  }

  // Creates/updates a workshop-scoped note (WORKSHOP_SCOPE_ID, #2288 PR 2) so
  // WorkshopReview's chat rail can live-edit it via edit_note during review.
  // Mirrors Onboarding.svelte's ensureNote().
  async function ensureWorkshopNote(title: string, content: string): Promise<string> {
    const notes: any[] = await invoke('get_notes', { projectId: WORKSHOP_SCOPE_ID, folderId: null, includeDeleted: false });
    const existing = notes.find((n: any) => n.title === title);
    if (existing) {
      await invoke('update_note', { request: { note_id: existing.id, content } });
      return existing.id;
    }
    const created: any = await invoke('create_note', {
      request: { title, project_id: WORKSHOP_SCOPE_ID, content, status: 'cabinet' }
    });
    return created.id;
  }

  async function handleUploadConfirm() {
    if (!createdAgent || uploadedFiles.length === 0) { onClose(); return; }
    uploading = true;
    uploadError = '';
    try {
      const rawCombined = uploadedFiles.map(f => `--- ${f.name} ---\n${f.content}`).join('\n\n');
      // Truncate to ~20K chars to stay within context limits (matches
      // processUpload's truncation in Onboarding.svelte).
      const combined = rawCombined.length > 20000
        ? rawCombined.substring(0, 20000) + '\n\n[Documents truncated for processing — full versions saved separately]'
        : rawCombined;
      const preset = await fetchDefaultSoulPreset();
      const extractionPrompt = buildUploadExtractionPrompt(combined, 'balanced', preset);

      soulContent = preset;
      memoryContent = '(Processing took too long. Ask me to create your memory in the chat below.)';
      // D26/PR-B (#2673): createWorkshopSession MUST run before streamAndCollect,
      // not after — streamAndCollect now requires a real, agent-bound sessionId
      // to dispatch synthesis through the correct runtime (previously fell back
      // to whatever session happened to be active, dispatching through the WRONG
      // agent's runtime per live-log repro in #2673). The session is created
      // fresh here since createdAgent was just created in handleCreate() — no
      // chat_sessions row bound to it exists yet.
      workshopSessionId = await createWorkshopSession(createdAgent.id);
      try {
        const response = await streamAndCollect(extractionPrompt, workshopSessionId);
        const parsed = parseSoulAndMemory(response, preset);
        soulContent = parsed.soul;
        memoryContent = parsed.memory;
      } catch (e) {
        console.error('[AGENT-CREATE] Extraction FAILED:', e);
      }

      await writeAgentFile('SOUL.md', soulContent);
      await writeAgentFile('MEMORY.md', memoryContent);

      workshopInitialMessage = "This is your **Memory** -- the facts, context, and details I'll keep in mind whenever we talk. Take a look and let me know if anything needs adjusting.";
      memoryNoteId = await ensureWorkshopNote('Memory', memoryContent || '');
      soulNoteId = await ensureWorkshopNote('Soul', soulContent || '');

      seedingPath = 'review';
    } catch (e: any) {
      uploadError = e?.message || String(e);
    } finally {
      uploading = false;
    }
  }

  // Interview path
  function interviewNext() {
    if (interviewIdx < INTERVIEW_QUESTIONS.length - 1) {
      interviewIdx += 1;
    }
  }

  function interviewBack() {
    if (interviewIdx > 0) interviewIdx -= 1;
  }

  async function handleInterviewFinish() {
    if (!createdAgent) { onClose(); return; }
    interviewSubmitting = true;
    interviewError = '';
    try {
      const answers = INTERVIEW_QUESTIONS
        .map((q, i) => (interviewAnswers[i]?.trim() ? `${q.label} ${interviewAnswers[i].trim()}\n` : ''))
        .join('');

      const preset = await fetchDefaultSoulPreset();

      if (!answers.trim()) {
        // All questions skipped — ADR-0059 D4: seeding is optional, agent already live.
        onClose();
        return;
      }

      // 'balanced' communication style: the create-agent interview has no
      // qComm-equivalent selector (unlike onboarding's questions screen), so
      // this always uses the balanced default.
      const extractionPrompt = buildInterviewExtractionPrompt(answers, 'balanced', preset);

      soulContent = preset;
      memoryContent = answers;
      // D26/PR-B (#2673): createWorkshopSession MUST run before streamAndCollect —
      // see comment in handleUploadConfirm() above for the full rationale.
      workshopSessionId = await createWorkshopSession(createdAgent.id);
      try {
        const response = await streamAndCollect(extractionPrompt, workshopSessionId);
        const parsed = parseSoulAndMemory(response, preset);
        soulContent = parsed.soul;
        memoryContent = parsed.memory;
      } catch (e) {
        console.error('[AGENT-CREATE] Extraction FAILED:', e);
      }

      await writeAgentFile('SOUL.md', soulContent);
      await writeAgentFile('MEMORY.md', memoryContent);

      workshopInitialMessage = "I've created your profile from your answers. This is your **Memory** -- take a look and let me know if anything needs adjusting.";
      memoryNoteId = await ensureWorkshopNote('Memory', memoryContent || '');
      soulNoteId = await ensureWorkshopNote('Soul', soulContent || '');

      seedingPath = 'review';
    } catch (e: any) {
      interviewError = e?.message || String(e);
    } finally {
      interviewSubmitting = false;
    }
  }

  // ── Step 2 (review): WorkshopReview finish ───────────────────────────
  // After the Memory→Soul review, purge the workshop-scope DB-only notes
  // (content is already durable in .pane/system/{UUID}/*.md) and dismiss.
  // Mirrors Onboarding.svelte's saveAndComplete() purge step.
  async function handleReviewFinish() {
    // Persist any live edits made during review (WorkshopReview's edit_note
    // calls already wrote these via the backend; this writes the final
    // in-memory soulContent/memoryContent state back to disk in case the
    // component's bound state diverged from the last backend write).
    await writeAgentFile('SOUL.md', soulContent);
    await writeAgentFile('MEMORY.md', memoryContent);

    const workshopNoteIds = [memoryNoteId, soulNoteId].filter(Boolean);
    for (const noteId of workshopNoteIds) {
      // #2302: purge failures must surface, not vanish silently. A failed
      // purge leaves a stale workshop-scope row behind (visible as a
      // duplicate note later), so log at error level rather than warn.
      try {
        await invoke('purge_note', { noteId });
      } catch (e) {
        console.error('[AGENT-CREATE] Failed to purge workshop note:', noteId, e);
      }
    }
    onClose();
  }

  // Dismiss with Escape key
  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      if (step === 'seeding') {
        // After name-commit — agent exists, just dismiss
        onClose();
      } else {
        // Before name-commit — clean no-op
        onClose();
      }
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if step === 'seeding' && seedingPath === 'review'}
  <!-- §24.10.1c / PR 1: same Memory→Soul review step onboarding uses, full-window
       (not constrained to .agent-create-center's 540px — matches Onboarding.svelte's
       WorkshopReview mount, which is a sibling of .onboarding-center, not nested in it). -->
  <div class="agent-create-overlay agent-create-overlay--workshop" role="dialog" aria-modal="true" aria-label="Review agent seed">
    <WorkshopReview
      bind:soulContent
      bind:memoryContent
      bind:memoryNoteId
      bind:soulNoteId
      scopeProjectId={WORKSHOP_SCOPE_ID}
      sessionId={workshopSessionId}
      initialAssistantMessage={workshopInitialMessage}
      onFinish={() => handleReviewFinish()}
    />
  </div>
{:else}
<div class="agent-create-overlay" role="dialog" aria-modal="true" aria-label="Create agent">
  <div class="agent-create-center">

    <!-- Step indicator dots -->
    <div class="agent-create-steps">
      {#each Array(stepCount) as _, i}
        <span class="agent-create-step-dot" class:active={i === activeStepIdx()}></span>
      {/each}
    </div>

    <!-- ── STEP 0: Provider ── -->
    {#if step === 'provider'}
      <h1>Set up your AI provider</h1>
      <p class="agent-create-subtitle">
        Choose a provider and verify your connection before creating an agent.
      </p>

      <div class="agent-conn-block">
        <div class="agent-conn-providers">
          <button
            class="agent-conn-provider-btn"
            class:active={selectedProvider === 'openrouter'}
            onclick={() => { selectedProvider = 'openrouter'; connectionStatus = 'not-tested'; apiKey = ''; }}
          >OpenRouter</button>
          <button
            class="agent-conn-provider-btn"
            class:active={selectedProvider === 'anthropic'}
            onclick={() => { selectedProvider = 'anthropic'; connectionStatus = 'not-tested'; apiKey = ''; }}
          >Anthropic</button>
          <!-- F4 fix (#2455 venice live-test v2): Venice was missing from this hardcoded
               provider list, making it invisible to the forced-create Step 0 flow even
               though `selectedProvider`'s type and `handleTestProvider`'s dispatch below
               both already support it. -->
          <button
            class="agent-conn-provider-btn"
            class:active={selectedProvider === 'venice'}
            onclick={() => { selectedProvider = 'venice'; connectionStatus = 'not-tested'; apiKey = ''; }}
          >Venice</button>
          <button
            class="agent-conn-provider-btn"
            class:active={selectedProvider === 'ollama'}
            onclick={() => { selectedProvider = 'ollama'; connectionStatus = 'not-tested'; apiKey = ''; }}
          >Ollama</button>
        </div>

        <div class="agent-conn-field">
          <label class="agent-conn-label">
            {selectedProvider === 'ollama' ? 'Ollama endpoint' : 'API key'}
          </label>
          <div class="agent-conn-row">
            <input
              type={selectedProvider === 'ollama' ? 'text' : 'password'}
              class="agent-conn-input"
              placeholder={selectedProvider === 'ollama' ? 'http://localhost:11434' : 'sk-...'}
              bind:value={apiKey}
            />
            <button
              class="ag-btn ag-btn--primary ag-btn--compact"
              onclick={handleTestProvider}
              disabled={connectionStatus === 'testing' || !apiKey.trim()}
            >
              {connectionStatus === 'testing' ? 'Testing…' : 'Save & Test'}
            </button>
          </div>
        </div>

        <!-- §28.1: status dot — NEVER rendered as green without a successful probe -->
        {#if connectionStatus !== 'not-tested'}
          <div class="agent-status-row">
            <span
              class="agent-statusdot"
              class:agent-statusdot--testing={connectionStatus === 'testing'}
              class:agent-statusdot--success={connectionStatus === 'success'}
              class:agent-statusdot--error={connectionStatus === 'error'}
            ></span>
            <span class="agent-status-label">
              {connectionStatus === 'testing' ? 'Testing connection…' : connectionMessage}
            </span>
          </div>
        {/if}
      </div>

      <div class="agent-create-actions">
        <button class="ag-btn ag-btn--ghost" onclick={onClose}>Never mind — stay on OpenClaw</button>
      </div>
    {/if}

    <!-- ── STEP R: Runtime picker (#2522 Stage 6b) ── -->
    <!-- README-group-b.md §8 "New Agent — provider, model, name": pick the runtime
         first, from CONNECTED runtimes only (openclaw excluded — no local-agent
         creation UI of its own). This is the one place "which provider owns this
         agent" gets decided; nothing else in the app offers a change-provider control. -->
    {#if step === 'runtime'}
      <h1>Choose a provider</h1>
      <p class="agent-create-subtitle">
        Pick which connected provider this agent will run on. This can't be changed later.
      </p>

      {#if loadingRuntimes}
        <p class="agent-create-subtitle agent-create-subtitle--faint">Loading connected providers…</p>
      {:else if runtimeLoadError}
        <div class="agent-error">{runtimeLoadError}</div>
      {:else if runtimeOptions.length === 0}
        <div class="agent-error">
          No connected providers found. Connect a provider in Settings › AI before creating an agent.
        </div>
      {:else}
        <div class="path-options" role="radiogroup" aria-label="Provider">
          {#each runtimeOptions as opt (opt.runtime_type)}
            <button
              type="button"
              class="path-card"
              class:path-card--active={selectedRuntimeType === opt.runtime_type}
              role="radio"
              aria-checked={selectedRuntimeType === opt.runtime_type}
              data-runtime={opt.runtime_type}
              onclick={() => handlePickRuntime(opt.runtime_type)}
            >
              <div class="path-title">{RUNTIME_LABELS[opt.runtime_type] ?? opt.runtime_type}</div>
              <div class="path-desc">
                {opt.masked_credential ?? opt.endpoint ?? 'Connected'}
              </div>
            </button>
          {/each}
        </div>
      {/if}

      <div class="agent-create-actions">
        <button class="ag-btn ag-btn--ghost" onclick={onClose}>Never mind</button>
      </div>
    {/if}

    <!-- ── STEP M: Model picker (#2522 Stage 6b; #2590 UX batch 2 item 4: searchable
         dropdown, same SearchableModelSelect.svelte component Smart Features/Settings
         use, replacing the card-wall picker) ── -->
    {#if step === 'model'}
      <h1>Choose a model</h1>
      <p class="agent-create-subtitle">
        Pick a model for {RUNTIME_LABELS[selectedRuntimeType] ?? selectedRuntimeType}. You can change this later in agent settings.
      </p>

      {#if loadingModels}
        <p class="agent-create-subtitle agent-create-subtitle--faint">Loading available models…</p>
      {:else if modelLoadError}
        <div class="agent-error">{modelLoadError}</div>
      {:else}
        <div class="agent-model-picker" data-testid="agent-create-model-picker">
          <SearchableModelSelect
            models={modelOptions}
            provider={selectedRuntimeType}
            selectedModel={selectedModel ?? ''}
            placeholder="Search models... (e.g. Claude, GPT, Gemini)"
            onSelect={(modelId) => handlePickModel(modelId)}
          />
        </div>
      {/if}

      <div class="agent-create-actions">
        <button class="ag-btn ag-btn--ghost" onclick={() => { step = 'runtime'; selectedModel = undefined; }}>← Back</button>
        <button class="ag-btn ag-btn--ghost" onclick={handleSkipModel}>Skip — decide later</button>
      </div>
    {/if}

    <!-- ── STEP 1: Name ── -->
    {#if step === 'name'}
      <h1>Name your agent</h1>
      <p class="agent-create-subtitle">
        {#if forcedCreate && pendingProvider}
          You're switching to {pendingProvider}. Give your local agent a name to continue.
        {:else if selectedRuntimeType}
          Creating on {RUNTIME_LABELS[selectedRuntimeType] ?? selectedRuntimeType}. Give your agent a name to continue.
        {:else}
          Give your agent a name. You can always rename it later.
        {/if}
      </p>

      <div class="agent-name-block">
        <input
          type="text"
          class="agent-name-input"
          class:agent-name-input--error={hasCollision}
          placeholder="e.g. Research Agent"
          bind:value={agentName}
          oninput={handleNameInput}
          onkeydown={(e) => { if (e.key === 'Enter' && !ctaDisabled) handleCreate(); }}
          autofocus
        />
        {#if slugPreview}
          <div class="agent-slug-preview">slug: {slugPreview}</div>
        {/if}

        <!-- §10.6 / Collision CTA copy design ruling: show "Enter a different name" (disabled) -->
        {#if hasCollision}
          <div class="agent-error">
            An agent named <strong>{agentName.trim()}</strong> already exists — pick a different name.
          </div>
        {/if}

        {#if createError}
          <div class="agent-error">{createError}</div>
        {/if}
      </div>

      <div class="agent-create-actions">
        {#if forcedCreate}
          <button class="ag-btn ag-btn--ghost" onclick={() => step = 'provider'}>← Back</button>
        {:else}
          <button class="ag-btn ag-btn--ghost" onclick={() => step = 'model'}>← Back</button>
        {/if}
        <!-- §10.6: while collision error shows, CTA reads "Enter a different name" (disabled) -->
        <button
          class="ag-btn ag-btn--primary"
          onclick={handleCreate}
          disabled={ctaDisabled}
        >
          {creating ? 'Creating…' : ctaLabel()}
        </button>
      </div>
    {/if}

    <!-- ── STEP 2: Seeding chooser ── -->
    {#if step === 'seeding' && createdAgent}

      {#if seedingPath === 'none'}
        <!-- Card chooser -->
        <div class="agent-create-eyebrow">New agent · Get started</div>
        <h1>{createdAgent.name} is created and ready.</h1>
        <p class="agent-create-subtitle">
          Seed your agent with context, or dive straight in.
        </p>

        <div class="path-options">
          <button class="path-card" onclick={handlePickUpload}>
            <div class="path-title">Upload Documents</div>
            <div class="path-desc">Drop in .txt or .md files that describe your agent — docs, notes, anything relevant.</div>
          </button>
          <button class="path-card" onclick={handlePickInterview}>
            <div class="path-title">Answer a Few Questions</div>
            <div class="path-desc">Quick 5-question interview to build your agent’s personality and context.</div>
          </button>
          <button class="path-card" onclick={handleJustChat}>
            <div class="path-title">Just Start Chatting</div>
            <div class="path-desc">Start fresh. Refine your agent over time.</div>
          </button>
        </div>

      {:else if seedingPath === 'upload'}
        <!-- Upload documents sub-step -->
        <div class="agent-create-eyebrow">New agent · Upload documents</div>
        <h1>Add context files</h1>
        <p class="agent-create-subtitle">
          Drop .txt or .md files into the zone below. They’ll be saved to your agent’s workspace.
        </p>

        <button type="button" class="upload-help-link" onclick={handleUploadHelpClick}>
          What should I upload?
          <span class="upload-help-icon">
            <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
              <path d="M7 17 17 7"/><path d="M7 7h10v10"/>
            </svg>
          </span>
        </button>

        <div
          class="agent-drop-zone"
          class:agent-drop-zone--dragging={isDragging}
          ondragover={(e) => { e.preventDefault(); isDragging = true; }}
          ondragleave={() => isDragging = false}
          ondrop={handleDrop}
          onclick={() => document.getElementById('agent-file-input')?.click()}
          role="button"
          tabindex="0"
          onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') document.getElementById('agent-file-input')?.click(); }}
        >
          <div class="agent-drop-text">Drop files here or click to browse</div>
          <div class="agent-drop-formats">.txt · .md</div>
          <input id="agent-file-input" type="file" accept=".txt,.md" multiple hidden onchange={handleFileInput} />
        </div>

        {#if uploadedFiles.length > 0}
          <div class="agent-file-list">
            {#each uploadedFiles as file, i}
              <div class="file-chip">
                <span>{file.name}</span>
                <button class="file-x" onclick={() => uploadedFiles = uploadedFiles.filter((_, j) => j !== i)}>✕</button>
              </div>
            {/each}
          </div>
        {/if}

        {#if uploadError}
          <div class="agent-error">{uploadError}</div>
        {/if}

        <div class="agent-create-actions">
          <button class="ag-btn ag-btn--ghost" onclick={() => { seedingPath = 'none'; uploadedFiles = []; uploadError = ''; }}>← Back</button>
          <button class="ag-btn ag-btn--ghost" onclick={handleJustChat}>Skip</button>
          <button
            class="ag-btn ag-btn--primary"
            onclick={handleUploadConfirm}
            disabled={uploading || uploadedFiles.length === 0}
          >
            {uploading ? 'Saving…' : `Save ${uploadedFiles.length} file${uploadedFiles.length === 1 ? '' : 's'} & continue`}
          </button>
        </div>

      {:else if seedingPath === 'interview'}
        <!-- Interview sub-step — one question at a time -->
        {@const q = INTERVIEW_QUESTIONS[interviewIdx]}
        <div class="agent-create-eyebrow">New agent · Interview · {interviewIdx + 1} of {INTERVIEW_QUESTIONS.length}</div>
        <h1>{q.label}</h1>
        <p class="agent-create-subtitle agent-create-subtitle--faint">Press Enter or click Next to continue. You can skip any question.</p>

        <div class="agent-interview-block">
          <textarea
            class="agent-interview-answer"
            placeholder={q.placeholder}
            bind:value={interviewAnswers[interviewIdx]}
            rows="3"
            onkeydown={(e) => {
              if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault();
                if (interviewIdx < INTERVIEW_QUESTIONS.length - 1) interviewNext();
                else handleInterviewFinish();
              }
            }}
            autofocus
          ></textarea>
        </div>

        <!-- Progress dots -->
        <div class="agent-interview-dots">
          {#each INTERVIEW_QUESTIONS as _, i}
            <span class="agent-interview-dot" class:agent-interview-dot--active={i === interviewIdx} class:agent-interview-dot--done={i < interviewIdx}></span>
          {/each}
        </div>

        {#if interviewError}
          <div class="agent-error">{interviewError}</div>
        {/if}

        <div class="agent-create-actions">
          {#if interviewIdx === 0}
            <button class="ag-btn ag-btn--ghost" onclick={() => { seedingPath = 'none'; interviewError = ''; }}>← Back</button>
          {:else}
            <button class="ag-btn ag-btn--ghost" onclick={interviewBack}>← Prev</button>
          {/if}
          <!-- ADR-0059 D4: skippable at any point -->
          <button class="ag-btn ag-btn--ghost" onclick={handleJustChat}>Skip all</button>
          {#if interviewIdx < INTERVIEW_QUESTIONS.length - 1}
            <button class="ag-btn ag-btn--primary" onclick={interviewNext}>Next →</button>
          {:else}
            <button
              class="ag-btn ag-btn--primary"
              onclick={handleInterviewFinish}
              disabled={interviewSubmitting}
            >
              {interviewSubmitting ? 'Saving…' : 'Done — create agent'}
            </button>
          {/if}
        </div>
      {/if}

    {/if}

  </div><!-- .agent-create-center -->
</div><!-- .agent-create-overlay -->
{/if}

<style>
  /* ── Overlay shell — reuses onboarding-overlay pattern ── */
  .agent-create-overlay {
    position: fixed;
    inset: 0;
    z-index: 9999;
    background: var(--bg);          /* §12 token: no hardcoded #0a0a0f */
    display: flex;
    align-items: center;
    justify-content: center;
  }

  /* WorkshopReview renders its own full-frame layout (.workshop-frame is
     width:100%/height:100%, flex column) — don't center-constrain it like the
     540px step content above. */
  .agent-create-overlay--workshop {
    align-items: stretch;
    justify-content: stretch;
  }

  .agent-create-center {
    max-width: 540px;
    width: 100%;
    padding: 40px 24px;
  }

  /* ── Step dots ── */
  .agent-create-steps {
    display: flex;
    gap: 6px;
    margin-bottom: 28px;
    justify-content: center;
  }

  .agent-create-step-dot {
    width: 6px;
    height: 6px;
    border-radius: var(--radius-pill);
    background: var(--border-strong);
    transition: background 0.2s;
  }

  .agent-create-step-dot.active {
    background: var(--accent);
  }

  /* ── Typography ── */
  h1 {
    font-size: 28px;
    font-weight: 700;
    color: var(--fg-1);
    margin: 0 0 8px;
  }

  .agent-create-subtitle {
    color: var(--fg-2);
    margin: 0 0 28px;
    font-size: 15px;
    line-height: 1.5;
  }

  .agent-create-eyebrow {
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--fg-3);
    margin-bottom: 8px;
  }

  /* ── Step 0: Provider block ── */
  .agent-conn-block {
    background: var(--surface-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    padding: 20px;
    margin-bottom: 24px;
  }

  .agent-conn-providers {
    display: flex;
    gap: 8px;
    margin-bottom: 16px;
  }

  .agent-conn-provider-btn {
    flex: 1;
    padding: 8px 12px;
    border-radius: var(--radius-md);
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-2);
    cursor: pointer;
    font-size: 13px;
    transition: all 0.15s;
  }

  .agent-conn-provider-btn.active {
    border-color: var(--accent);
    color: var(--fg-1);
    background: var(--accent-soft);
  }

  .agent-conn-label {
    display: block;
    font-size: 12px;
    color: var(--fg-3);
    margin-bottom: 6px;
  }

  .agent-conn-row {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .agent-conn-input {
    flex: 1;
    padding: 8px 12px;
    border-radius: var(--radius-md);
    border: 1px solid var(--border);
    background: var(--surface-1);
    color: var(--fg-1);
    font-size: 14px;
  }

  /* ── Button system (§Bug3 fix: ag-btn classes so overlay isn't relying on Settings.svelte scoped .btn) ── */
  .ag-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    font-family: var(--font-sans);
    font-size: 13.5px;
    font-weight: 500;
    padding: 10px 17px;
    border-radius: var(--radius-md);
    border: 1px solid transparent;
    cursor: pointer;
    transition: background 0.15s, color 0.15s, border-color 0.15s;
    white-space: nowrap;
  }

  .ag-btn:disabled {
    cursor: not-allowed;
  }

  .ag-btn--primary {
    background: var(--accent);
    color: var(--fg-on-accent);
    font-weight: 600;
  }

  .ag-btn--primary:disabled {
    background: var(--surface-2);
    color: var(--fg-4);
    border-color: var(--border-faint);
  }

  .ag-btn--primary:hover:not(:disabled) {
    background: var(--accent-hover);
  }

  .ag-btn--ghost {
    background: transparent;
    color: var(--fg-2);
    border-color: var(--border);
  }

  .ag-btn--ghost:hover:not(:disabled) {
    background: var(--overlay-hover);
    color: var(--fg-1);
    border-color: var(--border-strong);
  }

  /* compact modifier for inline-row usage (e.g. beside the API key input) */
  .ag-btn--compact {
    padding: 8px 14px;
  }

  /* ── Status dot — §28.1: only rendered green after a successful probe ── */
  .agent-status-row {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 12px;
  }

  .agent-statusdot {
    width: 8px;
    height: 8px;
    border-radius: var(--radius-pill);
    background: var(--fg-3);
  }

  .agent-statusdot--testing {
    background: var(--fg-3);
    animation: dot-pulse 1s ease-in-out infinite;
  }

  .agent-statusdot--success {
    background: var(--status-success, #22c55e);
    box-shadow: 0 0 0 3px var(--status-success-glow, rgba(34, 197, 94, 0.15));
  }

  .agent-statusdot--error {
    background: var(--status-error, #ef4444);
  }

  .agent-status-label {
    font-size: 13px;
    color: var(--fg-2);
  }

  @keyframes dot-pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.4; }
  }

  /* ── Step 1: Name block ── */
  .agent-name-block {
    margin-bottom: 24px;
  }

  .agent-name-input {
    width: 100%;
    padding: 12px 16px;
    border-radius: var(--radius-md);
    border: 1px solid var(--border);
    background: var(--surface-1);
    color: var(--fg-1);
    font-size: 18px;
    box-sizing: border-box;
    transition: border-color 0.15s;
  }

  .agent-name-input:focus {
    outline: none;
    border-color: var(--accent);
  }

  .agent-name-input--error {
    border-color: var(--status-error, #ef4444);
  }

  .agent-slug-preview {
    margin-top: 6px;
    font-size: 12px;
    color: var(--fg-3);
    font-family: monospace;
  }

  .agent-error {
    margin-top: 10px;
    padding: 10px 14px;
    background: var(--status-error-soft, rgba(239, 68, 68, 0.08));
    border: 1px solid var(--status-error-border, rgba(239, 68, 68, 0.25));
    border-radius: var(--radius-md);
    font-size: 13px;
    color: var(--fg-2);
  }

  /* ── Actions row ── */
  .agent-create-actions {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
    margin-top: 8px;
  }

  /* ── Step M: model picker (#2590 UX batch 2 item 4) ── */
  .agent-model-picker {
    margin-bottom: 28px;
  }

  /* ── Step 2: seeding — self-contained card styles (Svelte CSS is scoped; cannot inherit from Onboarding.svelte) ── */
  .path-options {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-bottom: 28px;
  }

  .path-card {
    display: block;
    width: 100%;
    text-align: left;
    padding: 18px 22px;
    background: var(--surface-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    cursor: pointer;
    transition: background 0.15s, border-color 0.15s;
    color: var(--fg-1);
    font-family: var(--font-sans);
  }

  .path-card:hover {
    background: var(--surface-2);
    border-color: var(--border-strong);
  }

  .path-card:disabled {
    opacity: 0.35;
    cursor: not-allowed;
  }

  /* #2522 Stage 6b: selected-state for the runtime/model radiogroup cards (Steps R/M) */
  .path-card--active {
    border-color: var(--accent);
    background: var(--accent-soft);
  }

  .path-title {
    font-size: 15px;
    font-weight: 600;
    color: var(--fg-1);
    margin-bottom: 2px;
  }

  .path-desc {
    font-size: 13px;
    color: var(--fg-3);
  }

  /* ── agent-conn-field wrapper ── */
  .agent-conn-field {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  /* ── Upload help link (#2311) ── */
  .upload-help-link {
    display: flex; align-items: center; justify-content: center; gap: 4px;
    margin: 0 auto 20px; background: transparent; border: 0; padding: 0; cursor: pointer;
    font-family: inherit; font-size: 13px; font-weight: 500; color: var(--accent); transition: color 0.15s ease;
  }
  .upload-help-link:hover { color: var(--accent-hover); }
  .upload-help-icon { display: inline-flex; transition: transform 0.15s ease; }
  .upload-help-link:hover .upload-help-icon { transform: translate(1px, -1px); }

  /* ── Upload drop zone (seeding upload path) ── */
  .agent-drop-zone {
    border: 2px dashed var(--border-strong);
    border-radius: var(--radius-lg);
    padding: 32px;
    text-align: center;
    cursor: pointer;
    margin-bottom: 16px;
    transition: border-color 0.15s, background 0.15s;
  }

  .agent-drop-zone--dragging {
    border-color: var(--accent);
    background: var(--accent-soft);
  }

  .agent-drop-text {
    font-size: 14px;
    color: var(--fg-2);
    margin-bottom: 4px;
  }

  .agent-drop-formats {
    font-size: 12px;
    color: var(--fg-3);
  }

  .agent-file-list {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .file-chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px;
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-pill);
    font-size: 13px;
    color: var(--fg-2);
  }

  .file-x {
    background: none;
    border: none;
    padding: 0;
    cursor: pointer;
    color: var(--fg-3);
    font-size: 12px;
    line-height: 1;
  }

  .file-x:hover {
    color: var(--fg-1);
  }

  /* ── Interview sub-step ── */
  .agent-create-subtitle--faint {
    color: var(--fg-4);
    font-size: 13px;
    margin-bottom: 20px;
  }

  .agent-interview-block {
    margin-bottom: 16px;
  }

  .agent-interview-answer {
    width: 100%;
    padding: 12px 16px;
    border-radius: var(--radius-md);
    border: 1px solid var(--border);
    background: var(--surface-1);
    color: var(--fg-1);
    font-size: 15px;
    font-family: var(--font-sans);
    line-height: 1.5;
    resize: vertical;
    box-sizing: border-box;
    transition: border-color 0.15s;
  }

  .agent-interview-answer:focus {
    outline: none;
    border-color: var(--accent);
  }

  .agent-interview-dots {
    display: flex;
    gap: 6px;
    justify-content: center;
    margin-bottom: 20px;
  }

  .agent-interview-dot {
    width: 6px;
    height: 6px;
    border-radius: var(--radius-pill);
    background: var(--border-strong);
    transition: background 0.2s;
  }

  .agent-interview-dot--active {
    background: var(--accent);
    width: 16px; /* elongated for current */
  }

  .agent-interview-dot--done {
    background: var(--accent-soft);
  }
</style>

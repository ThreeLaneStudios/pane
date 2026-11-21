<script lang="ts">
  /**
   * WorkshopReview.svelte — shared Memory→Soul review step (#2288 PR 1)
   *
   * Extracted from Onboarding.svelte's `{:else if screen === 'workshop'}` block
   * (markup: Onboarding.svelte:2679-2717 pre-extraction; CSS: 2941-3040; state:
   * 17, 109-136, 139-164, 300; functions: advanceWorkshop/advanceToSoul/
   * sendChatMessage/scrollChat + the stream-listener $effect).
   *
   * Two-step flow only (Memory → Soul). The Rules step is intentionally dropped
   * per agents.md §24.10.1d — this component never renders a 'rules' branch.
   * AgentCreateOverlay integration (PR 3 of #2288) is out of scope here; this
   * PR wires Onboarding.svelte as the sole caller.
   */
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { renderMarkdown } from '$lib/utils/markdown';
  import ChatPanel from './ChatPanel.svelte';

  type WorkshopDoc = 'memory' | 'soul';

  interface Props {
    soulContent: string;
    memoryContent: string;
    memoryNoteId: string;
    soulNoteId: string;
    scopeProjectId: string;
    // Required chat_sessions row id, resolved by the caller BEFORE mounting
    // this component (#2618). Post-ADR-0062, ai_chat_stream's session-first
    // dispatch requires a real session id — passing scopeProjectId's
    // WORKSHOP_SCOPE_ID sentinel with no sessionId caused the fallback path
    // in commands.rs to treat that literal string as a session id, which
    // never existed in chat_sessions ("Session __workshop__ does not exist").
    // Callers create this via api.createSession(WORKSHOP_SESSION_SCOPE_ID,
    // agentId) (see src/lib/onboarding/workshopSession.ts) so the session is
    // explicitly bound to the agent being refined, not resolved from ambient
    // is_default state. Threaded straight into ai_chat_stream's sessionId arg
    // below, same as ChatSidebar.svelte's sessionIdForStream pattern.
    sessionId: string;

    // Deviation from the scope doc's literal 6-prop list, called out explicitly
    // in the PR 1 summary: onboarding's upload path and questions path each
    // seed a *different* opening assistant line before entering the workshop
    // ("This is your Memory -- the facts..." vs. "I've created your profile
    // from your answers..."). chatMessages is internal-only state (per spec),
    // so without a way to parameterize the first line, extraction would
    // collapse both callers onto one hardcoded message -- a behavior change.
    // Required (no default) so every caller makes an explicit choice.
    initialAssistantMessage: string;
    // NOTE: `messages` param is an intentional, documented addition beyond the
    // scope doc's literal `onFinish: () => void` signature. chatMessages is
    // internal-only state (per spec) — without surfacing the final transcript
    // here, Onboarding.svelte's saveAndComplete() would lose the ability to
    // persist the workshop chat to the 'workspace' thread (its pre-extraction
    // behavior), which would violate the "MUST work identically" constraint.
    // The param is optional so callers that don't need the transcript (future
    // AgentCreateOverlay wiring, PR 3) can ignore it.
    onFinish: (messages?: { role: string, content: string }[]) => void;
  }

  let {
    soulContent = $bindable(),
    memoryContent = $bindable(),
    memoryNoteId = $bindable(),
    soulNoteId = $bindable(),
    scopeProjectId,
    sessionId,
    initialAssistantMessage,
    onFinish,
  }: Props = $props();

  // Internal-only state — not exposed to callers (per #2288 PR 1 scope doc).
  let workshopDoc: WorkshopDoc = $state('memory');
  let chatMessages: { role: string, content: string }[] = $state([{
    role: 'assistant',
    content: initialAssistantMessage
  }]);
  let chatMessagesForPanel = $derived.by(() => {
    const base = chatMessages.map((m, i) => ({
      id: `workshop-${i}`,
      thread_id: 'workshop',
      role: m.role as 'user' | 'assistant' | 'system',
      content: m.content,
      created_at: new Date().toISOString(),
    }));
    if (isStreaming && streamingContent) {
      base.push({
        id: 'streaming-now',
        thread_id: 'workshop',
        role: 'assistant' as const,
        content: streamingContent,
        created_at: new Date().toISOString(),
      });
    }
    return base;
  });
  let chatInput = $state('');
  let isStreaming = $state(false);
  let currentStreamId: string | null = $state(null);
  let streamingContent = $state('');
  let chatContainer: HTMLElement | undefined;
  let unlistenStream: (() => void) | null = null;
  let hasBeenEdited = $state(false);

  function scrollChat() {
    setTimeout(() => { if (chatContainer) chatContainer.scrollTop = chatContainer.scrollHeight; }, 10);
  }

  // Stream listener — reloads note content from the backend after the AI calls
  // edit_note/append_to_note on the current workshop note, so the live doc pane
  // (`.workshop-doc`) reflects AI edits without a manual refresh. Must move with
  // this component (Onboarding.svelte:206-292 pre-extraction) — without it the
  // doc pane never updates after AI edits.
  $effect(() => {
    let mounted = true;
    listen('ai-stream-chunk', async (event: any) => {
      if (!mounted) return;
      const data = event.payload;
      if (!currentStreamId || data.id !== currentStreamId) return;

      if (data.done) {
        if (streamingContent.trim()) {
          chatMessages = [...chatMessages, { role: 'assistant', content: streamingContent }];
        }
        // Check for tool results = doc was updated
        if (data.tool_results?.length > 0) {
          // Reload note content from backend if AI used edit_note on a workshop note
          if (data.toolCalls) {
            const toolCalls = Array.isArray(data.toolCalls) ? data.toolCalls : [];
            for (const tc of toolCalls) {
              const name = tc.function?.name || tc.name || '';
              if (name !== 'edit_note' && name !== 'append_to_note') continue;

              let args: any = {};
              try {
                // Canonical backend format is { id, name, params }.
                // Legacy/provider formats may use tc.function.arguments or tc.input.
                // All three must be checked so edit_note/append_to_note args are found
                // regardless of which format the stream emits. (#2049)
                args = typeof tc.function?.arguments === 'string'
                  ? JSON.parse(tc.function.arguments)
                  : (tc.function?.arguments || tc.input || tc.params || {});
              } catch { continue; }

              const editedNoteId = args.note_id;
              if (!editedNoteId) continue;

              if (editedNoteId === memoryNoteId || editedNoteId === soulNoteId) {
                try {
                  const note: any = await invoke('get_note', { noteId: editedNoteId });
                  const newContent = note?.content || '';
                  if (editedNoteId === memoryNoteId && newContent) {
                    memoryContent = newContent;
                  } else if (editedNoteId === soulNoteId && newContent) {
                    soulContent = newContent;
                  }
                  console.log(`[WORKSHOP] Reloaded ${editedNoteId === memoryNoteId ? 'memory' : 'soul'} content from backend (${newContent.length} chars)`);
                } catch (e) {
                  console.error('[WORKSHOP] Failed to reload note content:', e);
                }
              }
            }
          }
          const toolText = data.tool_results
            .filter((r: string) => r.startsWith('\u2705'))
            .map((r: string) => r.replace(/^\u2705\s*/, ''))
            .join('\n');
          if (toolText && !streamingContent.trim()) {
            chatMessages = [...chatMessages, { role: 'assistant', content: toolText }];
          } else if (toolText) {
            chatMessages = chatMessages.map((m, i) =>
              i === chatMessages.length - 1 ? { ...m, content: m.content + '\n' + toolText } : m
            );
          }
        }

        streamingContent = '';
        currentStreamId = null;
        isStreaming = false;
        scrollChat();

        // After first AI response, show navigation hint
        if (!hasBeenEdited) {
          hasBeenEdited = true;
          setTimeout(() => {
            const hint = workshopDoc === 'memory'
              ? "When you're happy with your Memory, click **Next: Soul →** above to continue."
              : "When you're happy with your Soul, click **Finish →** above to wrap up.";
            chatMessages.push({ role: 'system', content: hint });
            scrollChat();
          }, 1500);
        }
      } else if (data.text) {
        streamingContent += data.text;
        scrollChat();
      }
    }).then(fn => { if (mounted) unlistenStream = fn; });
    return () => { mounted = false; if (unlistenStream) unlistenStream(); };
  });

  function advanceWorkshop() {
    if (workshopDoc === 'memory') {
      advanceToSoul();
    } else {
      onFinish(chatMessages);
    }
  }

  function advanceToSoul() {
    hasBeenEdited = false;
    chatMessages = [...chatMessages, { role: 'system', content: '— Moving to Soul Document —' }];
    workshopDoc = 'soul';
    chatMessages = [...chatMessages, {
      role: 'assistant',
      content: "Now let's look at your **Soul** -- this defines how I communicate with you. My tone, style, and approach are all shaped by this. Want to change anything about how I talk to you?"
    }];
    scrollChat();
  }

  async function sendChatMessage(inputContent?: string) {
    const content = inputContent || chatInput.trim();
    if (!content || isStreaming) return;
    chatInput = '';

    chatMessages = [...chatMessages, { role: 'user', content }];
    isStreaming = true;
    streamingContent = '';
    scrollChat();

    try {
      const currentNoteId = workshopDoc === 'memory' ? memoryNoteId : soulNoteId;
      const currentDoc = workshopDoc === 'memory' ? memoryContent : soulContent;
      const docType = workshopDoc === 'memory' ? 'Memory' : 'Soul';

      const history = chatMessages.map(m => ({
        role: m.role === 'assistant' ? 'assistant' as const : 'user' as const,
        content: m.content
      }));

      // Use ai_chat_stream with context that includes the note info
      // The AI has edit_note tool - tell it which note to edit
      currentStreamId = await invoke('ai_chat_stream', {
        messages: history,
        // #2618: explicit sessionId short-circuits ai_chat_stream's
        // project_id-based fallback resolution entirely (see this
        // component's sessionId prop doc comment above for the full root
        // cause). Same pattern as ChatSidebar.svelte's sessionIdForStream.
        sessionId,
        context: {
          // D26/PR-B (#2673): renamed from 'onboarding' — currentView is
          // display/logging-only (see build_context_prompt, commands.rs)
          // and is NEVER used as a workshop-detection gate (that's
          // is_workshop_session, keyed off noteId/projectId). Safe rename,
          // no backend match arm depends on this literal.
          currentView: 'agent-seeding',
          // projectId comes from the caller's scopeProjectId prop — that's
          // already the right mechanism (Onboarding.svelte still passes
          // paneProjectId here; it becomes WORKSHOP_SCOPE_ID in PR 4 of #2288
          // when resolveWorkshopProjectId() is replaced — no change needed here).
          projectId: scopeProjectId,
          // Matches COALESCE label in execute_list_notes LEFT JOIN fix (#2288 PR 2)
          projectName: 'Workshop',
          noteId: currentNoteId,
          noteTitle: docType,
          navigationContext: `ONBOARDING MODE: You are helping the user refine their ${docType} document. The current ${docType} note (id: ${currentNoteId}, title: "${docType}") contains:\n\n${currentDoc}\n\n${workshopDoc === 'memory' ? 'The Memory contains facts about the user -- who they are, what they do, what matters to them. IMPORTANT: After the user is satisfied with their Memory doc, YOU must proactively say \"Great, let\'s move on to your Soul document next -- it defines how I communicate with you.\" and navigate to the Soul note. Do NOT ask what they want to do next -- the next step is always the Soul doc. Navigate by opening the Soul note.' : 'The Soul defines how you communicate -- your tone, style, approach. This is the FINAL step of onboarding. When the user is satisfied with the Soul doc, say \"Perfect -- you\'re all set! Let\'s go.\" Do NOT try to navigate anywhere -- the user will click the Let\'s Go button.'}\n\nWhen the user asks to change something, use the edit_note tool with id "${currentNoteId}" to update the content. Write the COMPLETE updated document, not just the changes. The left panel updates live when you edit the note.\n\nDO NOT infer or guess facts not in the document. If unsure, ask the user.\n\nKeep your conversational responses concise.`
        }
      }) as string;
    } catch (e) {
      console.error('Chat failed:', e);
      isStreaming = false;
      chatMessages = [...chatMessages, { role: 'assistant', content: `Something went wrong: ${e}` }];
    }
  }
</script>

<div class="workshop-frame">
  <div class="workshop-topbar">
    <div class="workshop-topbar-left">
      <div class="steps">
        <div class="dot" class:active={workshopDoc === 'memory'} class:done={workshopDoc === 'soul'}></div>
        <div class="dot" class:active={workshopDoc === 'soul'}></div>
      </div>
      <span class="step-label">{workshopDoc === 'memory' ? 'Memory -- 1 of 2' : 'Soul -- 2 of 2'}</span>
    </div>
    <div class="workshop-topbar-right">
      <button class="btn-primary" class:btn-pulse={hasBeenEdited} onclick={advanceWorkshop}>
        {workshopDoc === 'memory' ? 'Next: Soul →' : 'Finish →'}
      </button>
    </div>
  </div>
  <div class="workshop-content">
    <div class="workshop-doc">
      <h2>{workshopDoc === 'memory' ? 'Memory' : 'Soul'} <span class="live-badge">Live</span></h2>
      <div class="doc-sub">{workshopDoc === 'memory' ? 'What your AI remembers about you' : 'How your AI communicates with you'}</div>
      <div class="doc-prose">
        {@html renderMarkdown(workshopDoc === 'memory' ? memoryContent : soulContent)}
      </div>
    </div>
    <div class="workshop-chat">
      <!-- #2604 delta audit: WorkshopReview has no real agent roster/session
           binding to wait for (onboarding + agent-create are synthetic,
           non-session-backed chat surfaces) — 'Pane' is always the correct
           attribution here, so agentsLoadedProp is always true, never gated
           behind a getAgents() fetch like the two session-backed surfaces. -->
      <ChatPanel
        messages={chatMessagesForPanel}
        {isStreaming}
        {streamingContent}
        onSendMessage={(content) => sendChatMessage(content)}
        placeholder="Ask to edit or refine..."
        disabled={isStreaming}
        agentsLoadedProp={true}
      />
    </div>
  </div>
</div>

<style>
  /* Shared button tokens — duplicated from Onboarding.svelte (scoped CSS; per
     ADR-0060's "scoped-CSS trap" note, these are NOT auto-inherited from the
     parent's <style> block, so they must be defined here too). */
  .btn-primary {
    padding: 7px 18px; border-radius: 4px; border: none;
    background: #00E4E4; color: #111113; font-size: 12px; font-weight: 600; cursor: pointer;
  }
  .btn-primary:hover { background: #00C8C8; }
  .btn-primary:disabled { opacity: 0.4; cursor: not-allowed; }

  /* Workshop */
  .workshop-frame { width: 100%; height: 100%; display: flex; flex-direction: column; background: #111113; }
  .workshop-topbar {
    height: 48px; border-bottom: 1px solid #27272A;
    display: flex; align-items: center; justify-content: space-between;
    padding: 0 20px; flex-shrink: 0;
  }
  .workshop-topbar-left { display: flex; align-items: center; gap: 10px; }
  .steps { display: flex; gap: 6px; align-items: center; }
  .dot { width: 8px; height: 8px; border-radius: 4px; background: #27272A; transition: all 0.2s; }
  .dot.active { background: #00E4E4; width: 20px; }
  .dot.done { background: #10B981; }
  .step-label { font-size: 12px; color: #71717A; }
  .workshop-topbar-right { display: flex; gap: 8px; }
  .workshop-content { flex: 1; display: flex; min-height: 0; }
  .workshop-doc {
    flex: 6; padding: 28px 32px; overflow-y: auto;
    border-right: 1px solid #27272A;
  }
  .live-badge {
    display: inline-block; font-size: 10px; color: #00E4E4;
    border: 1px solid #00E4E4; padding: 1px 6px; border-radius: 4px;
    margin-left: 8px; text-transform: uppercase; letter-spacing: 0.5px;
    vertical-align: middle;
  }
  .doc-sub { font-size: 13px; color: #71717A; margin: 8px 0 24px; }
  .doc-prose { font-size: 15px; line-height: 1.8; color: #A1A1AA; }
  .doc-prose :global(p) { margin-bottom: 12px; }
  .doc-prose :global(h2) { font-size: 17px; color: #FAFAFA; margin: 16px 0 8px; font-weight: 600; }
  .doc-prose :global(h3) { font-size: 16px; color: #FAFAFA; margin: 14px 0 6px; font-weight: 600; }
  .doc-prose :global(strong) { color: #FAFAFA; }
  .doc-prose :global(ul) { padding-left: 18px; margin-bottom: 12px; }
  .doc-prose :global(.diff-remove) {
    background: rgba(239,68,68,0.15);
    color: #EF4444;
    text-decoration: line-through;
    transition: opacity 0.3s ease;
  }
  .doc-prose :global(.diff-add) {
    background: rgba(0,228,228,0.12);
    color: #00E4E4;
    border-left: 2px solid #00E4E4;
    padding-left: 8px;
    animation: fadeIn 0.4s ease;
  }
  @keyframes fadeIn { from { opacity: 0; } to { opacity: 1; } }

  .doc-prose :global(li) { margin-bottom: 6px; }
  .workshop-chat {
    flex: 4; display: flex; flex-direction: column; background: #18181B;
  }
  .btn-primary.btn-pulse { animation: btnPulse 2.5s ease-in-out infinite; }
  @keyframes btnPulse {
    0% { opacity: 0.4; box-shadow: none; }
    50% { opacity: 1; box-shadow: 0 0 20px 6px rgba(0,228,228,0.5); border-color: #00E4E4; }
    100% { opacity: 0.4; box-shadow: none; }
  }
</style>

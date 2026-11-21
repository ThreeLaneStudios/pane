/**
 * seedGeneration.ts — shared seed generation pipeline (#2288 PR 3, §24.10.1c)
 *
 * Extracted from Onboarding.svelte's `streamAndCollect` (Onboarding.svelte:1014,
 * pre-extraction), `parseSoulAndMemory` (1052), and the `extractionPrompt`
 * builders inside `processUpload()` (1078-1130) and `processQuestions()`
 * (1165-1210).
 *
 * "Generate, never transcribe" (ruling 4, §24.10.1c): both onboarding and
 * create-agent (AgentCreateOverlay.svelte) must synthesize SOUL.md/MEMORY.md
 * via a real model call — never write raw uploaded/interview content
 * verbatim. This module is the single source of truth for that pipeline so
 * both callers stay identical.
 *
 * `parseSoulAndMemory` deviates from the pre-extraction version: instead of
 * mutating outer `$state` variables as a side effect, it returns
 * `{ soul, memory }`. Callers assign the result to their own state.
 */
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

/**
 * Streams a single-shot prompt through `ai_chat_stream` and collects the
 * full response via the `ai-stream-chunk` event.
 *
 * `sessionId` is REQUIRED (D26/PR-B, #2673) — a real `chat_sessions` row id,
 * explicitly bound to the target agent, created by the caller BEFORE this
 * function runs (via `createWorkshopSession`-style helpers, see
 * `$lib/onboarding/workshopSession.ts`). Threaded into `ai_chat_stream` as a
 * top-level `sessionId` field, matching `WorkshopReview.svelte`'s existing
 * pattern (`sessionId` short-circuits `ai_chat_stream`'s `context.projectId`
 * fallback resolution entirely — see ADR-0062 D5 and docs/spec/sessions.md
 * §30.7.1a). Without this, the backend falls back to whatever session
 * happens to be active in that project scope, which may dispatch synthesis
 * through the WRONG agent's runtime (confirmed via live logs, #2673) — the
 * exact bug this parameter closes.
 *
 * Other than `sessionId`, no caller-side state is closed over — this
 * function only needs the prompt text and the session to dispatch through.
 */
export async function streamAndCollect(prompt: string, sessionId: string): Promise<string> {
  if (!sessionId) {
    throw new Error('streamAndCollect requires a non-empty sessionId (#2673, D26/PR-B)');
  }
  return new Promise(async (resolve, reject) => {
    let collected = '';
    let id: string;
    try {
      id = await invoke('ai_chat_stream', {
        messages: [{ role: 'user', content: prompt }],
        sessionId,
        context: { currentView: 'agent-seeding', lightweight: true }
      }) as string;
    } catch (e) {
      console.error('[SEED-GEN] ai_chat_stream invoke FAILED:', e);
      reject(e);
      return;
    }
    const ul = await listen('ai-stream-chunk', async (event: any) => {
      const data = event.payload;
      if (data.id !== id) return;
      if (data.error) { ul(); reject(data.error); return; }
      if (data.text) collected += data.text;
      if (data.done) {
        ul();
        resolve(collected);
      }
    });
    const timer = setTimeout(() => {
      ul();
      reject('Timed out');
    }, 90000);
    // Clear timeout on successful resolve
    const origResolve = resolve;
    resolve = (val: string | PromiseLike<string>) => { clearTimeout(timer); origResolve(val); };
  });
}

/**
 * Splits a generation response into `{ soul, memory }` on the model's
 * `---SOUL---` / `---MEMORY---` markers (with `## `/`# ` fallback variants).
 *
 * Returns a value instead of mutating outer state (deviation from the
 * pre-extraction `parseSoulAndMemory`, which set `soulContent`/`memoryContent`
 * directly) — callers assign the result to their own state.
 *
 * - Missing both markers → `soul` falls back to `fallbackSoul`, `memory` is
 *   the raw response.
 * - Present but in either order → handled (soul-then-memory or
 *   memory-then-soul).
 */
export function parseSoulAndMemory(
  response: string,
  fallbackSoul: string
): { soul: string; memory: string } {
  const markers = [
    { soul: '---SOUL---', memory: '---MEMORY---' },
    { soul: '## SOUL', memory: '## MEMORY' },
    { soul: '# SOUL', memory: '# MEMORY' },
  ];
  for (const { soul: sm, memory: mm } of markers) {
    if (response.includes(sm) && response.includes(mm)) {
      const si = response.indexOf(sm);
      const mi = response.indexOf(mm);
      let soul: string;
      let memory: string;
      if (si < mi) {
        soul = response.substring(si + sm.length, mi).trim();
        memory = response.substring(mi + mm.length).trim();
      } else {
        memory = response.substring(mi + mm.length, si).trim();
        soul = response.substring(si + sm.length).trim();
      }
      soul = soul.replace(/^[\n\r]+/, '').replace(/^[-=]+\n/, '');
      memory = memory.replace(/^[\n\r]+/, '').replace(/^[-=]+\n/, '');
      return { soul, memory };
    }
  }
  return { soul: fallbackSoul, memory: response };
}

/**
 * Builds the extraction prompt for the upload path (documents → SOUL.md +
 * MEMORY.md). Extracted from `processUpload()`'s `extractionPrompt`
 * construction (Onboarding.svelte:1094-1128, pre-extraction).
 *
 * @param combined - concatenated (and truncated) upload content
 * @param stylePreset - the style preset key the user chose (e.g. 'balanced')
 * @param preset - the resolved preset body text (or custom soul text)
 */
export function buildUploadExtractionPrompt(
  combined: string,
  stylePreset: string,
  preset: string
): string {
  return `You are setting up an AI partner in the Pane app. The user uploaded documents about themselves and chose the "${stylePreset}" communication style.

Read their documents and generate TWO separate files.

RULES:
- Synthesize the documents into a concise profile. Do not copy verbatim.
- ONLY include facts explicitly stated in the documents. NEVER guess, infer, or fabricate details — especially age, birthdate, location details, relationship status, or any personal facts not directly written.
- If the documents don't mention something, leave it out. Better to be incomplete than wrong.
- Write as prose paragraphs, not raw bullet dumps.
- Maximum 15-20 key points for Memory.
- The documents ARE from/about the user. Do not question this.

---MEMORY---
Write a concise factual profile. Structure as prose paragraphs (not raw bullet dumps). Cover:
- Who they are (name, location, role)
- What they do (business, projects, creative work)
- What they're working toward (goals, ambitions)
- What matters to them (values, philosophy, interests)
- How they work (preferences, daily practices, working style)

---SOUL---
Write a personality definition for their AI partner. Base on the "${stylePreset}" style:
${preset}

Adapt based on what you learned. Include:
- Communication style tailored to this person
- How to approach conversations with them
- What to prioritize and what to avoid

Keep both sections concise and well-structured. Use the exact markers ---SOUL--- and ---MEMORY--- to separate sections.

Documents:
${combined}`;
}

/**
 * Builds the extraction prompt for the interview path (Q&A answers → SOUL.md
 * + MEMORY.md). Extracted from `processQuestions()`'s `extractionPrompt`
 * construction (Onboarding.svelte:1165-1210, pre-extraction).
 *
 * @param answers - formatted "Label: answer\n" block for each answered question
 * @param qComm - communication preference key ('short' | 'balanced' | 'detailed')
 * @param preset - the resolved soul preset body text for the comm-style
 */
export function buildInterviewExtractionPrompt(
  answers: string,
  qComm: string,
  preset: string
): string {
  return `You are setting up an AI partner in the Pane app. The user answered these questions about themselves:

${answers}

Communication preference: ${qComm === 'short' ? 'concise and direct' : qComm === 'detailed' ? 'thorough and detailed' : 'balanced'}

Base communication style:
${preset}

RULES:
- Synthesize their answers into a natural profile. Do not repeat their words back verbatim.
- Only include information they explicitly provided. Do not guess or infer.
- Write as SHORT paragraphs (2-3 sentences each), separated by blank lines. NOT one long block.
- Cover different topics in separate paragraphs (who they are, what they do, what they're working on, their values).

---MEMORY---
Multiple short paragraphs covering who they are, what they do, what they're working on, and their values. Each topic gets its own paragraph separated by a blank line.

---SOUL---
A personality definition in short paragraphs, adapted from the base style, tailored to this person's communication preference.

Use the exact markers ---SOUL--- and ---MEMORY--- to separate sections.`;
}

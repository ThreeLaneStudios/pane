// #470 PR 4 — the first journey: "Create an agent".
//
// A non-developer should be able to read this file top to bottom and see
// every caption that ships to paneapp.ai/docs without touching any of the
// `action` closures below. All screenshot/manifest mechanics live in
// ./_runner.ts; this file is the journey's content only.
//
// Captions are Matt's, copied character-for-character from the source he
// approved (issue #470, "the first journey, Create an agent"). Do not
// reword them, do not "fix" the quoting or punctuation, do not touch the
// `›` in the prerequisite line below. See docs/spec/testing.md §25.13
// rule 3 — this journey is the reference for voice for every journey
// after it.
//
// Entry point traced against src/lib/components/AgentCreateOverlay.svelte
// (module header comment, "voluntary" path) and Settings.svelte's
// `openCreateFlow()` (forcedCreate=false): Settings → Agents tab → "+ New
// agent" opens the overlay straight onto Step R ("Choose a provider").
// Neither Settings.svelte nor Onboarding.svelte is modified by this PR —
// both are reached and read through their existing, already-shipped
// surface only.
//
// Each step's `action` drives the app from the PREVIOUS step's screen to
// THIS step's screen; the runner captures the screenshot immediately
// after the action returns, so the image always shows the screen the
// step's own caption is describing.
import { test } from '../fixtures';
import { runJourney, type JourneyStep, type Journey } from './_runner';

// Page-level front matter — NOT a step: no action, no screenshot. Rendered
// above the step list by the eventual docs template (separate PR).
const PREREQUISITE = `**Before you start:** connect a provider in Settings › AI.`;

const steps: JourneyStep[] = [
  {
    name: 'choose-provider',
    caption: `Choose a provider. Pick which connected provider this agent runs on. You can't change it later, so choose the one you want this agent to live on.`,
    action: async (tauriPage) => {
      // Settings → Agents tab → "+ New agent" — the only path to the
      // voluntary create flow (forcedCreate=false), which opens the
      // overlay straight onto Step R, "Choose a provider".
      await tauriPage.getByTitle('Settings').click();
      await tauriPage.getByText('Agents', { exact: true }).click();
      await tauriPage.getByText('+ New agent', { exact: true }).click();
      // PR #473's seeded fixture is the only connected, non-openclaw
      // runtime, so its provider card is the one and only thing to wait
      // for here (loadRuntimeOptions() fires on mount for this path).
      await tauriPage.waitForSelector('[data-runtime="ollama"]', 10000);
    },
  },
  {
    name: 'choose-model',
    caption: `Choose a model. Search for a model and select it. You can change the model later in the agent's settings.`,
    action: async (tauriPage) => {
      // Picking the provider card advances Step R → Step M and fires
      // fetch_available_models (→ GET /api/tags on the mock Ollama
      // endpoint, §3 of the PR 4 scope). Opening the search input shows
      // the fetched model in the dropdown for the screenshot.
      await tauriPage.click('[data-runtime="ollama"]');
      await tauriPage.waitForSelector('[data-testid="agent-create-model-search-input"]', 10000);
      await tauriPage.click('[data-testid="agent-create-model-search-input"]');
      await tauriPage.waitForSelector('[data-testid="model-option-llama3.2"]', 10000);
    },
  },
  {
    name: 'name-your-agent',
    caption: `Name your agent. Type a name, such as "Research Agent," then create it. You can rename it any time.`,
    action: async (tauriPage) => {
      // Picking the model advances Step M → Step 1, an empty, focused
      // name field — nothing is typed yet in this step's own action, so
      // the screenshot shows the blank field the caption describes.
      await tauriPage.click('[data-testid="model-option-llama3.2"]');
      await tauriPage.waitForSelector('[data-testid="agent-create-name-input"]', 10000);
    },
  },
  {
    name: 'agent-created',
    caption: `Your agent is ready. Pane confirms the agent is created. From here you can upload documents, answer a few questions to shape it, or start chatting right away.`,
    action: async (tauriPage) => {
      // ADR-0059 D4: this click is the transaction boundary — the real
      // create_local_agent command runs here. It advances Step 1 → Step 2
      // (the seeding chooser), which is the confirmation screen this
      // step's caption and screenshot are about.
      await tauriPage.fill('[data-testid="agent-create-name-input"]', 'Research Agent');
      await tauriPage.click('[data-testid="agent-create-cta"]');
      await tauriPage.waitForSelector('[data-testid="agent-create-seed-skip"]', 10000);
    },
  },
  {
    name: 'just-start-chatting',
    caption: `Start chatting. Choose "Just Start Chatting" to go straight to your new agent. You can refine it over time.`,
    action: async (tauriPage) => {
      // handleJustChat() just calls onClose() — no further write. The
      // overlay dismisses and the main app's chat pane, now scoped to the
      // just-created agent, is what this step's screenshot shows.
      await tauriPage.click('[data-testid="agent-create-seed-skip"]');
      await tauriPage.waitForSelector('.sidebar', 10000);
    },
  },
];

const journey: Journey = {
  name: 'create-an-agent',
  prerequisite: PREREQUISITE,
  steps,
};

test('create-an-agent', async ({ tauriPage }) => {
  await runJourney(journey, tauriPage);
});

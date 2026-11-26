// #470 PR 4 — the first journey: "Create an agent".
//
// A non-developer should be able to read this file top to bottom and see
// every caption that ships to paneapp.ai/docs without touching any of the
// `action`/`check` closures below. All screenshot/manifest mechanics live
// in ./_runner.ts; this file is the journey's content only.
//
// Captions are Matt's, originally copied character-for-character from the
// source he approved (issue #470, "the first journey, Create an agent").
// Round 2 (PR 474) rewrote the captions whose screenshots no longer
// matched them, and added one for the new opening step -- see this PR's
// own description for the old/new text of every caption that changed.
// Do not reword a caption without that same side-by-side review. Do not
// touch the `›` in the prerequisite line below. See docs/spec/testing.md
// §25.13 rule 3 -- this journey is the reference for voice for every
// journey after it.
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
// THIS step's screen; the runner asserts the step's `check`, THEN
// captures the screenshot, so the image always shows the screen the
// step's own caption is describing, proven rather than assumed.
//
// Round 3's fix for the same wrinkle round 2 worked around (steps
// 'choose-model' and 'name-your-agent'): selecting a model
// (AgentCreateOverlay.svelte's `handlePickModel`) advances Step M → Step 1
// synchronously -- there is no app state where a model is "selected" AND
// the model-picker screen is still on screen, short of an app-code change
// (out of scope, forbidden for this PR). Round 2's workaround was a real,
// existing-UI path (select → "← Back", which does not reset
// `selectedModel` → re-select) but Matt didn't want a reader walking a
// path no real user would take. Round 3 instead screenshots the moment
// BEFORE the click: 'choose-model' opens the picker and waits for the
// target option to be visible in the open list -- the check asserts
// exactly that -- then its screenshot captures the list with the option
// found but not yet selected. 'name-your-agent' clicks that same option
// first (the actual selection), then fills the name field, same as any
// other step whose action continues from where the previous one left the
// screen.
import { test, expect } from '../fixtures';
import { runJourney, type JourneyStep, type Journey } from './_runner';

// Page-level front matter — NOT a step: no action, no screenshot. Rendered
// above the step list by the eventual docs template (separate PR).
const PREREQUISITE = `**Before you start:** connect a provider in Settings › AI.`;

const steps: JourneyStep[] = [
  {
    // New in round 2 (§25.13 rule 7, PR 474): a journey starts from Home
    // and shows how to reach the feature — this journey previously
    // started with the create-agent overlay already open, skipping the
    // path a reader actually has to walk.
    name: 'open-agents-settings',
    caption: `Open Settings. In the Agents tab, click "+ New agent" to create one.`,
    action: async (tauriPage) => {
      await tauriPage.getByTitle('Settings').click();
      await tauriPage.getByText('Agents', { exact: true }).click();
    },
    check: async (tauriPage) => {
      // The Agents-tab-only helper line, not just any "+ New agent" text
      // match -- confirms we actually landed on the Agents tab, not some
      // other screen that happens to share wording.
      await tauriPage.waitForSelector('[data-testid="agents-captain-crew-helper"]', 10000);
      expect(await tauriPage.getByText('+ New agent', { exact: true }).isVisible()).toBe(true);
      // Round 3 (Reed's audit): screenshot 01 was still showing "General"
      // highlighted in the tab bar, not "Agents" -- a CSS-transition
      // timing artefact, the screenshot firing before the active-tab
      // class settles. Fixed here, not by adding a sleep: `waitForFunction`
      // polls the real DOM (same auto-waiting idiom `waitForSelector`
      // already uses elsewhere in this file) until the Agents tab button
      // itself (Settings.svelte's `class:active={activeTab === 'agents'}`)
      // genuinely carries the "active" class -- waiting out the
      // transition rather than racing it, then asserted explicitly below
      // so a regression here fails loudly, not silently.
      await tauriPage.waitForFunction(
        `Array.from(document.querySelectorAll('button')).some(el => el.textContent.trim() === 'Agents' && el.classList.contains('active'))`,
        10000
      );
      const agentsTabIsActive = await tauriPage.evaluate(
        `Array.from(document.querySelectorAll('button')).some(el => el.textContent.trim() === 'Agents' && el.classList.contains('active'))`
      );
      expect(agentsTabIsActive).toBe(true);
    },
  },
  {
    name: 'choose-provider',
    caption: `Choose a provider. Pick which connected provider this agent runs on. You can't change it later, so choose the one you want this agent to live on.`,
    action: async (tauriPage) => {
      // The only path to the voluntary create flow (forcedCreate=false),
      // which opens the overlay straight onto Step R, "Choose a provider".
      await tauriPage.getByText('+ New agent', { exact: true }).click();
      // PR #473's seeded fixture is the only connected, non-openclaw
      // runtime, so its provider card is the one and only thing to wait
      // for here (loadRuntimeOptions() fires on mount for this path).
      await tauriPage.waitForSelector('[data-runtime="ollama"]', 10000);
    },
    check: async (tauriPage) => {
      expect(await tauriPage.isVisible('[data-runtime="ollama"]')).toBe(true);
    },
  },
  {
    name: 'choose-model',
    caption: `Choose a model. Search for a model, then click it in the list to select it. You can change the model later in the agent's settings.`,
    action: async (tauriPage) => {
      // Picking the provider card advances Step R → Step M and fires
      // fetch_available_models (→ GET /api/tags on the mock Ollama
      // endpoint, §3 of the PR 4 scope). The dropdown opens on the input's
      // `focus` event (SearchableSelect.svelte's handleFocus) -- using
      // `.focus()` rather than `.click()` here because the plugin's click
      // command is a bare DOM `el.click()`, which fires a `click` event
      // only and isn't guaranteed to also focus a text input the way a
      // real user's click does.
      await tauriPage.click('[data-runtime="ollama"]');
      await tauriPage.waitForSelector('[data-testid="agent-create-model-search-input"]', 10000);
      await tauriPage.focus('[data-testid="agent-create-model-search-input"]');
      await tauriPage.waitForSelector('[data-testid="model-option-llama3.2"]', 10000);

      // Round 3: deliberately does NOT click the option. Selecting it
      // (SearchableSelect.svelte's `selectItem()`) advances Step M → Step
      // 1 synchronously -- there is no app state where the model is
      // "selected" AND the picker is still on screen, short of an
      // app-code change (out of scope). Rather than walk a path no real
      // user would take to fake that state (round 2's select → Back →
      // re-select), this step stops here: the list is open and the
      // target option is visible, in the DOM, in the list the user is
      // actually searching -- the moment before the click, not a state
      // the app passes through after it. The click itself happens as the
      // next step's action (see 'name-your-agent' below).
    },
    check: async (tauriPage) => {
      // The target option is genuinely visible in the open list --
      // not merely attached to the DOM -- before the screenshot captures
      // it, same real-settled-state guarantee every other step's check
      // provides.
      expect(await tauriPage.isVisible('[data-testid="model-option-llama3.2"]')).toBe(true);
    },
  },
  {
    name: 'name-your-agent',
    caption: `Name your agent. Type a name, such as "Research Agent," then create it. You can rename it any time.`,
    action: async (tauriPage) => {
      // Click the option the previous step's screenshot showed open and
      // visible in the list -- the real selection, once, not a round
      // trip. Dispatched as `mousedown`, not clicked:
      // SearchableSelect.svelte's row button only listens for
      // `onmousedown` (so it can preventDefault() and dodge the input's
      // own blur-before-click race) -- it has no `onclick` handler at
      // all, so the plugin's bare `el.click()` would never reach
      // `selectItem()`. This advances Step M → Step 1 (Name).
      await tauriPage.dispatchEvent('[data-testid="model-option-llama3.2"]', 'mousedown');
      await tauriPage.waitForSelector('[data-testid="agent-create-name-input"]', 10000);
      await tauriPage.fill('[data-testid="agent-create-name-input"]', 'Research Agent');
    },
    check: async (tauriPage) => {
      expect(await tauriPage.inputValue('[data-testid="agent-create-name-input"]')).toBe('Research Agent');
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
      await tauriPage.click('[data-testid="agent-create-cta"]');
      await tauriPage.waitForSelector('[data-testid="agent-create-seed-skip"]', 10000);
    },
    check: async (tauriPage) => {
      expect(await tauriPage.isVisible('[data-testid="agent-create-seed-skip"]')).toBe(true);
      // The confirmation heading names the just-created agent by the
      // name typed in the previous step -- proves this is a real
      // create_local_agent result, not a stale/default screen.
      expect(await tauriPage.getByText('Research Agent', { exact: false }).isVisible()).toBe(true);
    },
  },
  {
    name: 'just-start-chatting',
    // Rewritten in round 2 -- see this PR's description for the old
    // caption. handleJustChat() (AgentCreateOverlay.svelte) just calls
    // onClose(); there is no further navigation. That lands back on the
    // Settings Agents tab the journey opened from, not a chat pane. This
    // is a known, pre-existing navigation gap, filed as issue #475 --
    // not this journey's to fix — the caption now describes the actual
    // destination.
    caption: `Choose "Just Start Chatting" to finish. Pane returns you to the Agents list, where your new agent now appears.`,
    action: async (tauriPage) => {
      await tauriPage.click('[data-testid="agent-create-seed-skip"]');
      await tauriPage.waitForSelector('[data-testid="agents-captain-crew-helper"]', 10000);
    },
    check: async (tauriPage) => {
      // Overlay is gone and we're back on the Agents tab specifically
      // (not just "some screen with a sidebar") -- same helper-line
      // selector the opening step's own check uses.
      expect(await tauriPage.getByText('+ New agent', { exact: true }).isVisible()).toBe(true);
      expect(await tauriPage.getByText('Research Agent', { exact: false }).isVisible()).toBe(true);
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

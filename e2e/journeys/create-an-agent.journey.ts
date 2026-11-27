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
//
// Round 4 (PR 2 of #470's "journey hardening" group, §25.13 rule 8's
// amendment): #476 ("wrong progress dot lit") was closed not-a-bug --
// Reed proved `activeStepIdx` (AgentCreateOverlay.svelte:282-291) is
// correctly called as a function at its one call site
// (AgentCreateOverlay.svelte:665, `class:active={i === activeStepIdx()}`)
// and that `$derived.by` would crash every other call site that still
// calls it with `()`. The real defect was this journey, not the app:
// `.agent-create-step-dot` (AgentCreateOverlay.svelte:1065-1071) carries
// `transition: background 0.2s`, so the dot that just became active is
// still mid-fade from `--border-strong` to `--accent` in the same window
// a screenshot can land in -- exactly the class of gap rule 8's
// amendment names. None of the four overlay steps ('choose-provider',
// 'choose-model', 'name-your-agent', 'agent-created') had ANY check on
// the dots at all before this round; each now polls the real painted
// background of its own dot before the screenshot, same mechanism as
// the opening step's `activeTabContrastExpr` below (alpha-of-target vs.
// alpha-of-a-known-inactive-sibling, via a shared `dotPaintedActiveExpr`
// so the comparison logic exists in exactly one place in this file).
// Dot indices verified against the real voluntary-path markup
// (AgentCreateOverlay.svelte:282-291's `activeStepIdx`, voluntary
// `stepCount = 4`): provider=0 (1st dot), model=1 (2nd), name=2 (3rd),
// seeding/agent-created=3 (4th) -- the dots render via
// `{#each Array(stepCount) as _, i}` in that same index order, so
// markup order and step order agree here (confirmed by reading the
// template, not assumed).
//
// The other checks in steps 'choose-provider' and 'choose-model'
// (`isVisible` on the provider card / model option) are left alone: both
// elements are brought into the DOM by a plain Svelte `{#each}`/`{#if}`
// with no Svelte `transition:` directive and no CSS opacity/visibility
// transition on first appearance (grepped `AgentCreateOverlay.svelte`
// and `SearchableSelect.svelte` for `transition:` -- every hit is a CSS
// property transition on background/border/color for hover or
// active-state restyling, not on becoming visible), so there is no
// mid-transition frame for an `isVisible` check to miss.
import { test, expect } from '../fixtures';
import { runJourney, type JourneyStep, type Journey } from './_runner';

// Shared by every per-step progress-dot check below (§25.13 rule 8's
// amendment): `.agent-create-step-dot` (AgentCreateOverlay.svelte) paints
// `background: var(--border-strong)` inactive and `var(--accent)` active,
// with a 0.2s CSS transition between the two -- the class flips the
// instant `activeStepIdx()` changes, but the painted pixels can lag it by
// up to 200ms, the same gap the opening step's `activeTabContrastExpr`
// (below) already catches for the Agents/General tab buttons. One
// function, not four copies: every overlay step calls this with its own
// dot index rather than re-deriving the alpha-matching logic (One
// Function Per Operation). `targetIdx` is compared against its very next
// neighbour (wrapping), which is always a different dot and therefore
// always the inactive one, since only one dot is ever active at a time.
function dotPaintedActiveExpr(targetIdx: number): string {
  return `(() => {
    function alphaOf(el) {
      const m = getComputedStyle(el).backgroundColor.match(/rgba?\\(([^)]+)\\)/);
      if (!m) return 0;
      const parts = m[1].split(',').map(s => parseFloat(s.trim()));
      return parts.length === 4 ? parts[3] : 1;
    }
    const dots = Array.from(document.querySelectorAll('.agent-create-step-dot'));
    const target = dots[${targetIdx}];
    if (!target || dots.length < 2) return false;
    const sibling = dots[(${targetIdx} + 1) % dots.length];
    if (!sibling) return false;
    return (alphaOf(target) - alphaOf(sibling)) > 0.05;
  })()`;
}

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
      // highlighted in the tab bar, not "Agents". First attempt here
      // checked the Agents button's own `class:active` membership
      // (Settings.svelte's `class:active={activeTab === 'agents'}`) --
      // that class flips the instant `activeTab` changes, same reactive
      // tick as the click, so it was already true well before the
      // screenshot and did not catch the real defect: `.seg-btn.active`
      // (Settings.svelte CSS) carries `transition: all 0.2s` -- the class
      // is applied immediately, but the actual PAINTED background/
      // box-shadow animates for up to 200ms afterward, and a screenshot
      // taken in that window can still show General's box mid-fade-out
      // and/or Agents' box not yet faded in. A class check can never
      // catch that -- it has to read the real, rendered style.
      //
      // Fixed by polling the two buttons' own computed
      // `background-color` alpha directly (`getComputedStyle`, read
      // fresh on every `waitForFunction` poll -- no event listener, so no
      // race with a transition that may already be finishing by the time
      // this runs) until the Agents button is genuinely, visibly more
      // opaque than General by a real margin -- the exact visual fact
      // Reed's screenshot review was checking for, not a proxy for it.
      // No bare sleep: this waits out the transition's actual paint,
      // it does not guess a fixed delay.
      const activeTabContrastExpr = `(() => {
        function alphaOf(btn) {
          const m = getComputedStyle(btn).backgroundColor.match(/rgba?\\(([^)]+)\\)/);
          if (!m) return 0;
          const parts = m[1].split(',').map(s => parseFloat(s.trim()));
          return parts.length === 4 ? parts[3] : 1;
        }
        const buttons = Array.from(document.querySelectorAll('button'));
        const agentsBtn = buttons.find(el => el.textContent.trim() === 'Agents');
        const generalBtn = buttons.find(el => el.textContent.trim() === 'General');
        if (!agentsBtn || !generalBtn) return false;
        return (alphaOf(agentsBtn) - alphaOf(generalBtn)) > 0.05;
      })()`;
      await tauriPage.waitForFunction(activeTabContrastExpr, 10000);
      expect(await tauriPage.evaluate(activeTabContrastExpr)).toBe(true);
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
      // §25.13 rule 8 amendment: the voluntary path's 1st dot (index 0,
      // AgentCreateOverlay.svelte's `activeStepIdx`) must be genuinely
      // painted active before this screenshot, not just class-flipped.
      const dot0Expr = dotPaintedActiveExpr(0);
      await tauriPage.waitForFunction(dot0Expr, 10000);
      expect(await tauriPage.evaluate(dot0Expr)).toBe(true);
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
      // §25.13 rule 8 amendment: 2nd dot (index 1) painted active.
      const dot1Expr = dotPaintedActiveExpr(1);
      await tauriPage.waitForFunction(dot1Expr, 10000);
      expect(await tauriPage.evaluate(dot1Expr)).toBe(true);
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
      // Reed's audit (docs/audits/2026-11-26-scope-470-fonts.md §3): the
      // name field being filled does NOT mean the Create button is ready
      // to screenshot. AgentCreateOverlay.svelte's handleNameInput
      // (:330-343) arms a 300ms debounce, then awaits a real
      // checkNameCollision() IPC round-trip, before ctaDisabled (:269-271)
      // settles back to false -- a screenshot taken inside that window
      // (as the last run's did) shows a correctly-filled name next to a
      // disabled, greyed Create button.
      //
      // First attempt here used a bare `waitForSelector(':not([disabled])')`
      // -- that resolved (the `disabled` attribute genuinely was gone) but
      // a real CI run still captured the button in its grey, disabled
      // *paint* (rgb(28,28,32), exactly `--surface-2`, `.ag-btn--primary:disabled`'s
      // background), not the enabled accent background. That's the SAME
      // class of gap round 3 found on the Agents tab -- the DOM attribute
      // flips one tick before WebKitGTK's own repaint lands, and
      // `.ag-btn--primary`'s `transition: background 0.15s` (:1169 in this
      // file) means there's a real, if short, window where `disabled` is
      // false but the painted pixels still show the old (grey) state. A
      // selector/attribute check alone cannot catch that -- it has to read
      // the real, rendered style, same fix as step 1's
      // activeTabContrastExpr above.
      const ctaPaintedEnabledExpr = `(() => {
        const btn = document.querySelector('[data-testid="agent-create-cta"]');
        if (!btn || btn.disabled) return false;
        const m = getComputedStyle(btn).backgroundColor.match(/rgba?\\(([^)]+)\\)/);
        if (!m) return false;
        const p = m[1].split(',').map(s => parseFloat(s.trim()));
        // Disabled paints --surface-2 (rgb(28,28,32) dark theme). Enabled
        // paints --accent (#00E4E4 = rgb(0,228,228)). Rather than assert
        // an exact accent match (theme-dependent), assert the background
        // has moved well away from the known disabled grey -- true the
        // instant the repaint actually lands, false for however long the
        // stale frame persists after the attribute flips.
        return !(Math.abs(p[0] - 28) < 12 && Math.abs(p[1] - 28) < 12 && Math.abs(p[2] - 32) < 12);
      })()`;
      await tauriPage.waitForFunction(ctaPaintedEnabledExpr, 10000);
      expect(await tauriPage.evaluate(ctaPaintedEnabledExpr)).toBe(true);

      // §25.13 rule 8 amendment: 3rd dot (index 2) painted active. This
      // is separate from the CTA check above -- the step → dot transition
      // starts the instant the previous step's action advances
      // `step = 'name'`, which is a different 0.2s window than the CTA's
      // own enable transition, so one passing does not imply the other.
      const dot2Expr = dotPaintedActiveExpr(2);
      await tauriPage.waitForFunction(dot2Expr, 10000);
      expect(await tauriPage.evaluate(dot2Expr)).toBe(true);
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
      // §25.13 rule 8 amendment: 4th dot (index 3) painted active. This
      // is the known case from #476 -- the prior journey had no dot
      // check here at all, so a screenshot taken while dot 3 (name's
      // dot) was still fading out and dot 4 was still fading in could
      // pass every other assertion above and still show the wrong dot.
      const dot3Expr = dotPaintedActiveExpr(3);
      await tauriPage.waitForFunction(dot3Expr, 10000);
      expect(await tauriPage.evaluate(dot3Expr)).toBe(true);
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

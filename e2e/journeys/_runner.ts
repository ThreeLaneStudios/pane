// #470 PR 4: the one place that knows how to run a journey's step list.
//
// A journey (the test file next to this one) is a plain-object array --
// every step is a named record with its own caption right there on the
// object literal. This file owns all the *mechanics*: running each step's
// action, asserting its check, capturing+validating a screenshot via the
// EXISTING shared helper (e2e/helpers/screenshot.ts, reused verbatim -- no
// second PNG decoder), and writing the deterministic manifest.json a later
// docs-template PR will render. Zero screenshot/manifest code lives in the
// journey file itself -- see create-an-agent.journey.ts.
//
// Round 2 (PR 474): the order is now fixed as action -> check ->
// screenshot, and a step with no check is a hard failure (see
// `runJourney` below). Before this, a step's screenshot was taken
// immediately after its action with nothing in between proving the
// resulting screen actually matched the step's own caption -- which is
// exactly how "Create an agent"'s first draft shipped a model-picker
// screenshot captured before any model was selected. The check is the
// thing that would have caught that, so it's no longer optional.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from '../fixtures';
import { captureAndValidateScreenshot } from '../helpers/screenshot';
import type { TauriPage } from '@srsholmes/tauri-playwright';

// Resolved from import.meta.url, not __dirname -- same reason
// e2e/tests/smoke.spec.ts already does this (see that file's own
// comment): correct regardless of where `playwright test` is invoked
// from.
const JOURNEYS_DIR = path.dirname(fileURLToPath(import.meta.url));

export type JourneyStep = {
  /** kebab-case, used in the screenshot filename and the manifest's "name" field. */
  name: string;
  /** End-user-facing prose, verbatim from whoever wrote the journey (§25.13 rule 3). */
  caption: string;
  /** Drives the app from the previous step's screen to this step's screen. */
  action: (tauriPage: TauriPage) => Promise<void>;
  /**
   * Asserts the screen `action` just produced actually matches what this
   * step's `caption` is about to claim. Runs AFTER `action`, BEFORE the
   * screenshot -- the runner's whole order is action -> check -> screenshot,
   * so a check failure means the screenshot is never taken for a state that
   * doesn't match its own caption. Required: a step with no check is a bug
   * in the journey file, not an optional nicety (see `runJourney` below for
   * the hard-failure enforcement).
   */
  check: (tauriPage: TauriPage) => Promise<void>;
};

export type ManifestStep = { index: number; name: string; caption: string; image: string };

export type Journey = {
  /** Used for the output directory and the manifest's "journey" field. */
  name: string;
  /**
   * Page-level front matter, NOT a step -- no action, no screenshot. Renders
   * above the step list in the eventual docs template (e.g. "Before you
   * start: connect a provider in Settings › AI."). Omitted entirely from
   * the manifest when a journey has none.
   */
  prerequisite?: string;
  steps: JourneyStep[];
};

/**
 * Runs every step of `journey` in order against the real app, writing one
 * PNG per step plus a deterministic manifest.json to
 * e2e/journeys-output/<journey.name>/.
 *
 * No wall-clock timestamp field anywhere in the manifest -- §25.13 "docs
 * generation must be deterministic" means the manifest's content (order,
 * names, captions) must not vary run to run. A `generatedAt` field would
 * make byte-identical reproduction impossible for no benefit the docs
 * template needs; only the PNGs' pixel bytes are allowed to vary
 * (anti-aliasing/render-timing noise), which is why the blank-frame
 * rejection in the shared helper validates *variation*, not byte-identity.
 */
export async function runJourney(journey: Journey, tauriPage: TauriPage): Promise<void> {
  const outDir = path.join(JOURNEYS_DIR, '..', 'journeys-output', journey.name);
  fs.mkdirSync(outDir, { recursive: true });
  const manifestSteps: ManifestStep[] = [];

  for (const [i, step] of journey.steps.entries()) {
    await test.step(step.name, async () => {
      await step.action(tauriPage);

      // Hard failure, not a warning/skip: the runtime check backs up the
      // TypeScript `check` field being non-optional. A step without one is
      // exactly the bug that let step 2 of this journey's first draft get
      // screenshotted before its own action had actually finished (the
      // model picker, captured with nothing selected yet) -- the check is
      // what proves the screen matches the caption before a single PNG
      // byte is written.
      if (typeof step.check !== 'function') {
        throw new Error(
          `journey step "${step.name}" has no check -- every step must assert the ` +
          `screen "action" just produced before its screenshot is taken (§25.13 ` +
          `"each screenshot is taken after its action, so the result is visible").`
        );
      }
      await step.check(tauriPage);

      const image = `${String(i + 1).padStart(2, '0')}-${step.name}.png`;
      await captureAndValidateScreenshot(tauriPage, path.join(outDir, image));
      manifestSteps.push({ index: i + 1, name: step.name, caption: step.caption, image });
    });
  }

  const manifest = {
    journey: journey.name,
    ...(journey.prerequisite !== undefined ? { prerequisite: journey.prerequisite } : {}),
    steps: manifestSteps,
  };

  fs.writeFileSync(path.join(outDir, 'manifest.json'), JSON.stringify(manifest, null, 2));
}

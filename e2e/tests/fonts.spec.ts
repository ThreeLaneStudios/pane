import { test, expect } from '../fixtures';

// #470 (fonts PR): no existing check in this repo is font-aware.
// captureAndValidateScreenshot (e2e/helpers/screenshot.ts) only rejects a
// blank/uniform frame -- any rendered text, Inter or a Linux fallback
// face, produces plenty of distinct colors and channel range from
// anti-aliasing alone, so that heuristic passes identically regardless of
// which font actually rendered. This test is the one check in the suite
// that fails the run (not warns) when the app's root font stack does not
// actually resolve to Inter -- e.g. if fonts-inter failed to install, or
// the e2e/ci-fonts/local.conf alias wasn't copied/picked up by
// fontconfig.
//
// Lives in the `smoke` Playwright project (e2e/playwright.config.ts),
// alongside the existing "Pane app loads"/"Pane sidebar is visible"
// checks -- this is a CI/environment-correctness proof, not journey
// content, so it runs once, not duplicated per journey screenshot.
//
// Honest about scope (Reed's audit, docs/audits/2026-11-26-scope-470-fonts.md
// §4): this proves font-IDENTITY -- that the app's actual root font-family
// stack (src/app.css:188, unchanged by this PR) resolves to the same
// physical font face Inter provides. It does NOT prove the result looks
// visually correct, legible, or well-kerned -- that judgment still needs
// a human looking at the published screenshots. It also cannot catch a
// case where Inter is installed and resolving, but renders with some
// other visual defect (clipping, wrong weight, etc.) -- measureText
// metrics would still match in that case. What it DOES catch, reliably:
// Inter not installed, the fontconfig alias not present/not picked up,
// or the root stack resolving to a different distro fallback face
// (DejaVu Sans, Liberation Sans, Ubuntu Sans, etc.) -- any of those
// produce different per-glyph advance widths, which this test asserts
// against directly.
test('Linux CI font stack resolves to Inter, not the distro fallback', async ({ tauriPage }) => {
  // tauri-plugin-playwright's evaluate() takes a raw JS expression string
  // sent over the socket, not a function reference (confirmed against
  // every other evaluate() call site in this suite, e.g.
  // e2e/tests/tier2/ai-tool-use.spec.ts and this journey's own
  // activeTabContrastExpr in create-an-agent.journey.ts) -- there is no
  // function-serialization step the way real Playwright's page.evaluate
  // has.
  const result = await tauriPage.evaluate<{
    interAvailable: boolean;
    interWidth: number;
    stackWidth: number;
  }>(`(() => {
    const canvas = document.createElement('canvas');
    const ctx = canvas.getContext('2d');
    if (!ctx) throw new Error('fonts.spec.ts: 2D canvas context unavailable');
    const probe = 'The quick brown fox jumps 0123456789';

    ctx.font = '16px Inter';
    const interWidth = ctx.measureText(probe).width;

    // The exact root stack src/app.css:188 uses, unchanged by this PR.
    // Must resolve to the SAME physical font as the explicit 'Inter'
    // probe above -- via the fontconfig alias (e2e/ci-fonts/local.conf),
    // not a direct CSS name match (this literal stack never names
    // 'Inter' anywhere in its list).
    ctx.font = "16px -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif";
    const stackWidth = ctx.measureText(probe).width;

    return {
      interAvailable: document.fonts.check('16px Inter'),
      interWidth: interWidth,
      stackWidth: stackWidth,
    };
  })()`);

  // document.fonts.check() alone only proves Inter is installed, not that
  // any given CSS stack actually resolves to it -- the measureText
  // comparison below is the load-bearing half of this check.
  expect(result.interAvailable).toBe(true);

  // Same physical font => canvas measureText returns identical glyph
  // metrics for the same probe string at the same size. A different
  // fallback face has different per-glyph advance widths, so this
  // genuinely fails if the fontconfig alias is missing, Inter isn't
  // installed, or fc-cache wasn't re-run after writing local.conf.
  expect(result.stackWidth).toBeCloseTo(result.interWidth, 1);
});

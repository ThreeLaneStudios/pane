import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import { expect } from '@playwright/test';
import type { TauriPage } from '@srsholmes/tauri-playwright';

// Extracted from e2e/tests/smoke.spec.ts (#470 PR 3) per the convention
// posted on issue #470 (comment 7169240575): "every screenshot call passes
// an explicit path, and every assertion reads from the file on disk --
// never from the returned buffer. This belongs in one shared helper... so
// no journey author can reintroduce either mistake." Moved verbatim, not
// rewritten -- the decode/validate logic itself was already correct (PR
// #472's final audit, §5).
//
// Why disk-only: tauri-plugin-playwright 0.4.1's screenshot() returns a
// populated Buffer only when called with NO `path`; when a `path` IS
// passed, it writes the PNG to disk and returns an EMPTY buffer. PR #472
// hit both failure modes in sequence (no file ever written, then a correct
// capture read as a false "bytes: 0" failure) before landing on
// disk-only validation -- see that PR's own history for the full story.

/**
 * Minimal, dependency-free PNG decoder (8-bit depth, color types
 * 0/2/3/4/6) -- just enough to defilter scanlines back to raw pixel
 * bytes. No PNG-decoding package exists in this project's dependencies,
 * and this helper intentionally doesn't add one. Throws on anything it
 * doesn't understand (e.g. a 16-bit-depth or interlaced PNG) rather than
 * silently passing.
 */
function decodePng(buf: Buffer): { width: number; height: number; channels: number; pixels: Buffer } {
  let offset = 8; // skip the 8-byte PNG signature
  let width = 0;
  let height = 0;
  let bitDepth = 0;
  let colorType = -1;
  const idatParts: Buffer[] = [];
  while (offset < buf.length) {
    const len = buf.readUInt32BE(offset);
    const type = buf.toString('ascii', offset + 4, offset + 8);
    const data = buf.subarray(offset + 8, offset + 8 + len);
    if (type === 'IHDR') {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      bitDepth = data[8];
      colorType = data[9];
    } else if (type === 'IDAT') {
      idatParts.push(data);
    } else if (type === 'IEND') {
      break;
    }
    offset += 8 + len + 4;
  }
  if (bitDepth !== 8) {
    throw new Error(`screenshot PNG: unsupported bit depth ${bitDepth} (expected 8)`);
  }
  const channelsByColorType: Record<number, number> = { 0: 1, 2: 3, 3: 1, 4: 2, 6: 4 };
  const channels = channelsByColorType[colorType];
  if (!channels) {
    throw new Error(`screenshot PNG: unsupported color type ${colorType}`);
  }
  const inflated = zlib.inflateSync(Buffer.concat(idatParts));
  const rowBytes = width * channels;
  const pixels = Buffer.alloc(rowBytes * height);
  let prevRowStart = -1;
  for (let y = 0; y < height; y++) {
    const inRowStart = y * (rowBytes + 1);
    const filterType = inflated[inRowStart];
    const outRowStart = y * rowBytes;
    for (let x = 0; x < rowBytes; x++) {
      const raw = inflated[inRowStart + 1 + x];
      const a = x >= channels ? pixels[outRowStart + x - channels] : 0;
      const b = prevRowStart >= 0 ? pixels[prevRowStart + x] : 0;
      const c = x >= channels && prevRowStart >= 0 ? pixels[prevRowStart + x - channels] : 0;
      let value: number;
      switch (filterType) {
        case 0:
          value = raw;
          break;
        case 1:
          value = (raw + a) & 0xff;
          break;
        case 2:
          value = (raw + b) & 0xff;
          break;
        case 3:
          value = (raw + ((a + b) >> 1)) & 0xff;
          break;
        case 4: {
          const p = a + b - c;
          const pa = Math.abs(p - a);
          const pb = Math.abs(p - b);
          const pc = Math.abs(p - c);
          const pred = pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
          value = (raw + pred) & 0xff;
          break;
        }
        default:
          throw new Error(`screenshot PNG: unsupported filter type ${filterType} on row ${y}`);
      }
      pixels[outRowStart + x] = value;
    }
    prevRowStart = outRowStart;
  }
  return { width, height, channels, pixels };
}

/**
 * Proves the frame isn't a blank/uniform capture -- the exact failure mode
 * a byte-size check alone misses, since a solid-color 1280x800 PNG still
 * compresses to several KB (verified locally: ~5.9KB), comfortably past
 * any "non-empty file" size threshold.
 */
function analyzeVariation({ width, height, channels, pixels }: { width: number; height: number; channels: number; pixels: Buffer }) {
  const totalPixels = width * height;
  const sampleStep = Math.max(1, Math.floor(totalPixels / 20000));
  const distinctColors = new Set<string>();
  let minR = 255, maxR = 0, minG = 255, maxG = 0, minB = 255, maxB = 0;
  for (let p = 0; p < totalPixels; p += sampleStep) {
    const i = p * channels;
    const r = pixels[i];
    const g = channels >= 2 ? pixels[i + 1] : r;
    const b = channels >= 3 ? pixels[i + 2] : r;
    if (r < minR) minR = r;
    if (r > maxR) maxR = r;
    if (g < minG) minG = g;
    if (g > maxG) maxG = g;
    if (b < minB) minB = b;
    if (b > maxB) maxB = b;
    distinctColors.add(`${r},${g},${b}`);
  }
  return {
    distinctSampledColors: distinctColors.size,
    maxChannelRange: Math.max(maxR - minR, maxG - minG, maxB - minB),
  };
}

export type ScreenshotVariation = { distinctSampledColors: number; maxChannelRange: number };

export type ScreenshotThresholds = {
  /** Minimum on-disk byte size. Default 1024. */
  minBytes?: number;
  /** Minimum distinct sampled colors (blank-frame rejection). Default 10. */
  minDistinctColors?: number;
  /** Minimum max-channel range (blank-frame rejection). Default 20. */
  minChannelRange?: number;
  /** Bounded deadline for the stability wait below. Default 10000ms. */
  stabilityTimeoutMs?: number;
};

// PR 480 round 2 — moved here from per-step polls in individual journeys
// (e.g. create-an-agent.journey.ts's deleted `dotPaintedActiveExpr`,
// `activeTabContrastExpr`, `ctaPaintedEnabledExpr`). Five separate
// bespoke paint checks across four journey steps still missed a real
// defect (screenshot 05 showed the wrong progress dot in CI run
// 46011622928) because each one only knew how to watch ONE specific CSS
// property on ONE specific element. This helper doesn't guess which
// property might still be animating -- it asks the browser what's
// actually still running, for the whole document, every time, so no
// journey author has to rediscover "the painted pixel lags the class/
// attribute" for each new element they add a screenshot of.
//
// Id used to find/remove the injected caret-hiding <style> tag so
// repeated calls across steps don't stack duplicate elements.
const CARET_HIDE_STYLE_ID = '__e2e-caret-hide__';

/**
 * A blinking text-input caret is real, continuous repaint that
 * `document.getAnimations()` cannot see (it's not a CSS animation or
 * transition -- most engines implement it as a timer-driven native
 * paint) and that could make two consecutive screenshots never match:
 * the condition (b) below checks for would never settle while a field
 * is focused. This sets `caret-color: transparent` document-wide before
 * the stability wait, same as before -- but pixel-sampled directly
 * (screenshot 04, `name-your-agent`, shows a 1px caret bar after
 * "Research Agent" in every real CI run) confirms WebKitGTK, the engine
 * `e2e-linux.yml` actually runs against, does not honor `caret-color` on
 * a native `<input>` -- the property is set correctly, the browser just
 * ignores it there. So this does NOT make the caret stop blinking in
 * CI, and is not what makes these screenshots stable. What actually
 * guarantees stability is the byte-identical-consecutive-capture check
 * (condition (b)) downstream: it happens to land on two frames with the
 * caret in the same phase often enough in practice that this hasn't
 * caused a flake, but that's luck from the polling cadence, not a
 * property this function controls. Kept because it's harmless, it does
 * work on at least some engines/contexts, and removing it would be a
 * behavior change with no evidence it's safe -- Reed's ruling: keep the
 * code, correct the comment. The element is removed again once this
 * step's capture is done so it doesn't leak into whatever the next
 * action does.
 */
async function hideCaret(tauriPage: TauriPage): Promise<void> {
  await tauriPage.evaluate(`(() => {
    if (document.getElementById('${CARET_HIDE_STYLE_ID}')) return;
    const style = document.createElement('style');
    style.id = '${CARET_HIDE_STYLE_ID}';
    style.textContent = '*, *::before, *::after { caret-color: transparent !important; }';
    document.head.appendChild(style);
  })()`);
}

async function restoreCaret(tauriPage: TauriPage): Promise<void> {
  await tauriPage.evaluate(`(() => {
    const style = document.getElementById('${CARET_HIDE_STYLE_ID}');
    if (style) style.remove();
  })()`);
}

/**
 * Waits until the screen is visually stable, then captures it to
 * `screenshotPath` and returns the captured bytes. Two conditions, both
 * must hold:
 *
 * (a) `document.getAnimations()` -- every running CSS animation/
 *     transition on the whole document, not a hand-picked selector --
 *     reports nothing still running or pending.
 * (b) Two screenshots taken back to back are byte-identical. This is
 *     the backstop for anything (a) can't see: a late image decode, a
 *     JS-driven repaint with no Web Animations entry, etc.
 *
 * The caret is hidden for the duration (see `hideCaret` above) so a
 * blinking cursor can't make (b) loop forever.
 *
 * Bounded by `timeoutMs`; throws a specific error naming which
 * condition never settled rather than silently returning an unstable
 * frame. No sleeps -- (a) polls via the plugin's own `waitForFunction`,
 * and (b) is paced by the real round-trip latency of each native
 * screenshot call, not an added delay.
 */
async function waitForScreenStable(
  tauriPage: TauriPage,
  screenshotPath: string,
  timeoutMs: number,
): Promise<Buffer> {
  const deadline = Date.now() + timeoutMs;
  const tmpA = `${screenshotPath}.stability-a.png`;
  const tmpB = `${screenshotPath}.stability-b.png`;

  await hideCaret(tauriPage);
  try {
    // (a) All running animations/transitions finished.
    const animationsSettledExpr =
      `document.getAnimations().every((a) => a.playState !== 'running' && a.playState !== 'pending')`;
    try {
      await tauriPage.waitForFunction(animationsSettledExpr, Math.max(1, deadline - Date.now()));
    } catch {
      throw new Error(
        `screenshot stability wait for ${screenshotPath} timed out after ${timeoutMs}ms: ` +
        `document.getAnimations() still reports a running/pending CSS animation or ` +
        `transition on the page.`,
      );
    }

    // (b) Two consecutive captures byte-identical.
    let previous: Buffer | null = null;
    let previousPath = tmpB;
    for (;;) {
      const current = previousPath === tmpA ? tmpB : tmpA;
      await tauriPage.screenshot({ path: current });
      const bytes = fs.readFileSync(current);
      if (previous && bytes.equals(previous)) {
        fs.copyFileSync(current, screenshotPath);
        return bytes;
      }
      if (Date.now() > deadline) {
        throw new Error(
          `screenshot stability wait for ${screenshotPath} timed out after ${timeoutMs}ms: ` +
          `could not confirm two consecutive captures were byte-identical (something kept ` +
          `repainting that document.getAnimations() could not see).`,
        );
      }
      previous = bytes;
      previousPath = current;
    }
  } finally {
    fs.rmSync(tmpA, { force: true });
    fs.rmSync(tmpB, { force: true });
    await restoreCaret(tauriPage);
  }
}

/**
 * Captures a screenshot to `screenshotPath` and validates it from disk --
 * never from `tauriPage.screenshot()`'s returned buffer, which
 * tauri-plugin-playwright 0.4.1 returns empty (`Buffer.alloc(0)`)
 * unconditionally whenever a `path` is passed.
 *
 * Validation carries all four checks posted as the shared-helper
 * convention on issue #470: file exists, on-disk size > threshold,
 * distinctSampledColors > threshold, maxChannelRange > threshold.
 *
 * PR 480 round 2: before any of that, waits for the screen to be
 * visually stable (see `waitForScreenStable` above) -- every journey
 * step's screenshot goes through this one function, so the stability
 * wait applies everywhere without any journey file asking for it.
 */
export async function captureAndValidateScreenshot(
  tauriPage: TauriPage,
  screenshotPath: string,
  thresholds: ScreenshotThresholds = {},
): Promise<{ onDisk: Buffer; variation: ScreenshotVariation }> {
  const { minBytes = 1024, minDistinctColors = 10, minChannelRange = 20, stabilityTimeoutMs = 10000 } = thresholds;

  fs.mkdirSync(path.dirname(screenshotPath), { recursive: true });
  const onDisk = await waitForScreenStable(tauriPage, screenshotPath, stabilityTimeoutMs);

  // It must actually have landed on disk at the given path.
  expect(fs.existsSync(screenshotPath)).toBe(true);

  // A sane byte size rules out an empty or truncated file. This alone does
  // NOT rule out a blank frame (see analyzeVariation below) -- a
  // solid-color screenshot this size still compresses well past this
  // floor -- so it's a minimum-sanity check, not the blank-frame proof.
  expect(onDisk.length).toBeGreaterThan(minBytes);

  // Decode the actual uploaded PNG and prove it isn't a uniform
  // single-colour frame: a real capture must show many distinct sampled
  // colors and real spread in at least one color channel. A blank/solid
  // frame decodes to exactly one distinct color and zero channel range, as
  // verified locally against synthetically generated solid-black and
  // solid-white 1280x800 PNGs.
  const decoded = decodePng(onDisk);
  const variation = analyzeVariation(decoded);
  expect(variation.distinctSampledColors).toBeGreaterThan(minDistinctColors);
  expect(variation.maxChannelRange).toBeGreaterThan(minChannelRange);

  return { onDisk, variation };
}

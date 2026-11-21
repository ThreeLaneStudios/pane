/**
 * Onboarding.venice-smart-features.test.ts — #2795 coverage.
 *
 * Venice as a 4th Smart Features provider on the §6.4.1 picker screen
 * (screen === 'smart-features-picker'). Mirrors the navigation pattern used
 * by Onboarding.test.ts's navigateToModelSelect() / Onboarding.synthesisOrdering.test.ts's
 * navigateToMainScreen() — drives the component from 'connect' through
 * 'model-select' to 'smart-features-picker' via real user interactions, then
 * exercises the Venice-specific picker card + sub-flow.
 *
 * Coverage:
 *  A. §6.4.1 picker renders a 4th Venice card (name + descriptor, #2792 convention)
 *  B. Clicking the Venice card routes to the 'smart-features-venice-setup' screen
 *  C. sfTestConnection's Venice branch calls validate_venice_key + save_venice_key
 *     (NOT set_ai_setting('internal_api_key', ...) — the shared OR/Anthropic slot)
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, fireEvent } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import Onboarding from './Onboarding.svelte';

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: invokeMock,
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

if (typeof (globalThis.window as any).addEventListener !== 'function') {
  (globalThis.window as any).addEventListener = () => {};
}
if (typeof (globalThis.window as any).removeEventListener !== 'function') {
  (globalThis.window as any).removeEventListener = () => {};
}
if (typeof (globalThis.window as any).dispatchEvent !== 'function') {
  (globalThis.window as any).dispatchEvent = () => true;
}

function defaultInvokeImpl(overrides: Record<string, (args: any) => any> = {}) {
  return async (cmd: string, args: any = {}) => {
    if (overrides[cmd]) return overrides[cmd](args);
    switch (cmd) {
      case 'check_fresh_install':
        return false;
      case 'ai_get_default_soul':
        return '';
      case 'validate_anthropic_key':
      case 'validate_openrouter_key':
        return null;
      case 'ai_configure_and_save':
        return null;
      case 'get_agents':
        return [];
      case 'ollama_probe_two_step':
        return { error_class: null, models: [] };
      case 'fetch_available_models':
        return [{ id: 'claude-opus-4-5', name: 'Claude Opus 4.5', provider: 'anthropic' }];
      case 'set_ai_setting':
        return null;
      default:
        return null;
    }
  };
}

/** Drives the component from 'connect' through 'model-select' to
 *  'smart-features-picker', reusing the same interaction pattern as
 *  Onboarding.test.ts's navigateToModelSelect(). */
async function navigateToSmartFeaturesPicker(container: HTMLElement) {
  await tick();
  await tick();

  const providerCard = Array.from(container.querySelectorAll('.provider-card')).find((el) =>
    el.textContent?.includes('Anthropic')
  ) as HTMLElement;
  await fireEvent.click(providerCard);
  await tick();

  const keyInput = container.querySelector('input[type="password"]') as HTMLInputElement;
  await fireEvent.input(keyInput, { target: { value: 'sk-test-key' } });
  await tick();

  const testBtn = Array.from(container.querySelectorAll('button')).find((b) =>
    b.textContent?.includes('Test Connection')
  ) as HTMLButtonElement;
  await fireEvent.click(testBtn);
  await tick();
  await tick();
  await new Promise((r) => setTimeout(r, 0));
  await tick();
  await tick();

  const continueBtn = Array.from(container.querySelectorAll('button')).find((b) =>
    b.textContent?.trim() === 'Continue' && b.classList.contains('connect-continue')
  ) as HTMLButtonElement;
  await fireEvent.click(continueBtn);
  await tick();
  await tick();
  await tick();
  await new Promise((r) => setTimeout(r, 0));
  await tick();
  await tick();

  // 'model-select' screen — pick a model, then Continue.
  const modelInput = container.querySelector('.model-input') as HTMLInputElement;
  await fireEvent.focus(modelInput);
  await tick();
  const item = Array.from(container.querySelectorAll('.dropdown-item'))[0] as HTMLElement;
  await fireEvent.mouseDown(item);
  await tick();

  const modelContinueBtn = Array.from(container.querySelectorAll('button')).find((b) =>
    b.textContent?.trim() === 'Continue'
  ) as HTMLButtonElement;
  await fireEvent.click(modelContinueBtn);
  await new Promise((r) => setTimeout(r, 0));
  await tick();
  await new Promise((r) => setTimeout(r, 0));
  await tick();
  await tick();

  // Now on 'smart-features-picker'.
}

beforeEach(() => {
  invokeMock.mockReset();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('A — §6.4.1 picker renders a 4th Venice card', () => {
  it('renders a Venice provider-card with name + descriptor, alongside OpenRouter/Anthropic/Ollama', async () => {
    invokeMock.mockImplementation(defaultInvokeImpl());
    const { container } = render(Onboarding, { props: { onComplete: vi.fn() } });
    await navigateToSmartFeaturesPicker(container);

    expect(container.textContent).toContain('Connect Smart Features');

    const veniceCard = Array.from(container.querySelectorAll('.provider-card')).find((el) =>
      el.textContent?.includes('Venice')
    ) as HTMLElement;
    expect(veniceCard).toBeTruthy();
    expect(veniceCard.querySelector('.provider-desc')?.textContent).toBe('Your Venice API models');

    // No regression: the other three cards are still present.
    const cardNames = Array.from(container.querySelectorAll('.provider-card .provider-name')).map(
      (el) => el.textContent
    );
    expect(cardNames).toContain('OpenRouter');
    expect(cardNames).toContain('Anthropic');
    expect(cardNames).toContain('Ollama');
    expect(cardNames).toContain('Venice');
  });
});

describe('B — clicking the Venice card routes to the Venice sub-flow screen', () => {
  it('shows "Smart Features — Venice" and a Venice-specific API key field after clicking the Venice card', async () => {
    invokeMock.mockImplementation(defaultInvokeImpl());
    const { container } = render(Onboarding, { props: { onComplete: vi.fn() } });
    await navigateToSmartFeaturesPicker(container);

    const veniceCard = Array.from(container.querySelectorAll('.provider-card')).find((el) =>
      el.textContent?.includes('Venice')
    ) as HTMLElement;
    await fireEvent.click(veniceCard);
    await tick();

    expect(container.textContent).toContain('Smart Features — Venice');
    expect(container.textContent).toContain('venice.ai/settings/api');
    const keyInput = container.querySelector('input[type="password"]') as HTMLInputElement;
    expect(keyInput).toBeTruthy();
    expect(keyInput.placeholder).toBe('VEN-...');
  });
});

describe('C — Venice sub-flow test-connection uses Venice-specific commands', () => {
  it('calls validate_venice_key + save_venice_key, NOT set_ai_setting internal_api_key (R3 CRITICAL)', async () => {
    const setAiSettingCalls: Array<{ key: string; value: string }> = [];
    invokeMock.mockImplementation(
      defaultInvokeImpl({
        set_ai_setting: async (args: any) => {
          setAiSettingCalls.push({ key: args.key, value: args.value });
          return null;
        },
        validate_venice_key: async () => null,
        save_venice_key: async () => null,
        fetch_internal_provider_models: async () => [
          { id: 'venice-model-1', name: 'Venice Model 1', provider: 'venice' },
        ],
      })
    );

    const { container } = render(Onboarding, { props: { onComplete: vi.fn() } });
    await navigateToSmartFeaturesPicker(container);

    const veniceCard = Array.from(container.querySelectorAll('.provider-card')).find((el) =>
      el.textContent?.includes('Venice')
    ) as HTMLElement;
    await fireEvent.click(veniceCard);
    await tick();

    const keyInput = container.querySelector('input[type="password"]') as HTMLInputElement;
    await fireEvent.input(keyInput, { target: { value: 'VEN-test-key-1234' } });
    await tick();

    const testBtn = Array.from(container.querySelectorAll('button')).find((b) =>
      b.textContent?.includes('Test Connection')
    ) as HTMLButtonElement;
    await fireEvent.click(testBtn);
    await tick();
    await tick();
    await new Promise((r) => setTimeout(r, 0));
    await tick();
    await tick();

    expect(invokeMock).toHaveBeenCalledWith('validate_venice_key', { apiKey: 'VEN-test-key-1234' });
    expect(invokeMock).toHaveBeenCalledWith('save_venice_key', { key: 'VEN-test-key-1234' });
    expect(invokeMock).toHaveBeenCalledWith('set_ai_setting', { key: 'internal_provider', value: 'venice' });

    // R3 CRITICAL regression guard: Venice's key must never be written via
    // set_ai_setting('internal_api_key', ...) — that's the shared OR/Anthropic
    // keychain-backed slot (read_internal_key), not Venice's dedicated
    // ai_venice_key account (read_venice_key).
    const internalApiKeyWrite = setAiSettingCalls.find((c) => c.key === 'internal_api_key');
    expect(internalApiKeyWrite).toBeUndefined();

    // Should have advanced to the models step.
    expect(container.textContent).toContain('Select models for Smart Features');
  });
});

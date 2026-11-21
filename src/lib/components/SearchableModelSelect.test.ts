/**
 * SearchableModelSelect.test.ts — Component tests for the searchable AI model
 * dropdown (#2874 item 6).
 *
 * SearchableModelSelect renders a text input that filters a provided model
 * list (by name/provider/id) and shows a fixed-position dropdown grouped by
 * provider. It also hosts a "Enter custom model name..." action that opens
 * PromptDialog (replacing the old native `window.prompt()` call) so a caller
 * can select an out-of-list model id.
 */

import { describe, it, expect, vi } from 'vitest';
import { render, fireEvent, screen } from '@testing-library/svelte/svelte5';
import SearchableModelSelect from './SearchableModelSelect.svelte';

// vitest-setup.ts replaces `globalThis.window` with a plain mock object (Tauri
// internals shim) that lacks a real addEventListener/removeEventListener pub-sub
// implementation. SearchableModelSelect registers `scroll`/`resize` listeners
// (and PromptDialog, rendered when the custom-model prompt is open, uses
// <svelte:window onkeydown={...}>) via onMount, so the DOM listener API needs
// to exist on that mock window for mount/unmount to succeed (same patch pattern
// as PromptDialog.test.ts / DefaultAgentPickerModal.test.ts / AgentCreateOverlay.test.ts).
(globalThis.window as any).addEventListener = () => {};
(globalThis.window as any).removeEventListener = () => {};

const models = [
  { id: 'anthropic/claude-sonnet-4-5', name: 'Claude Sonnet 4.5', provider: 'Anthropic' },
  { id: 'anthropic/claude-opus-4', name: 'Claude Opus 4', provider: 'Anthropic' },
  { id: 'openai/gpt-4o', name: 'GPT-4o', provider: 'OpenAI' },
  { id: 'venice/llama-3.3-70b', name: 'Llama 3.3 70B', provider: 'Venice' },
];

describe('SearchableModelSelect', () => {
  it('renders with placeholder text when no model is selected', () => {
    const { container } = render(SearchableModelSelect, {
      props: {
        models,
        selectedModel: '',
        placeholder: 'Search models...',
        onSelect: vi.fn(),
      },
    });

    const input = container.querySelector('.model-input') as HTMLInputElement;
    expect(input.placeholder).toBe('Search models...');
    expect(input.value).toBe('');
  });

  it('shows display text of the selected model when one is set', () => {
    const { container } = render(SearchableModelSelect, {
      props: {
        models,
        selectedModel: 'openai/gpt-4o',
        onSelect: vi.fn(),
      },
    });

    const input = container.querySelector('.model-input') as HTMLInputElement;
    expect(input.value).toBe('GPT-4o');
    expect(input.title).toBe('GPT-4o');
  });

  it('falls back to the raw id as display text for a custom (non-list) selected model', () => {
    const { container } = render(SearchableModelSelect, {
      props: {
        models,
        selectedModel: 'some/custom-model-id',
        onSelect: vi.fn(),
      },
    });

    const input = container.querySelector('.model-input') as HTMLInputElement;
    expect(input.value).toBe('some/custom-model-id');
  });

  it('opens the dropdown on focus and lists all models sorted by provider then name', async () => {
    const { container } = render(SearchableModelSelect, {
      props: {
        models,
        selectedModel: '',
        provider: 'openrouter',
        onSelect: vi.fn(),
      },
    });

    const input = container.querySelector('.model-input') as HTMLInputElement;
    await fireEvent.focus(input);

    const dropdown = container.querySelector('.dropdown-list');
    expect(dropdown).toBeTruthy();

    const items = Array.from(container.querySelectorAll('.dropdown-item:not(.custom)'));
    // 4 models, sorted by provider (Anthropic, Anthropic, OpenAI, Venice) then name.
    expect(items.length).toBe(4);
    expect(items[0].querySelector('.provider')?.textContent).toBe('Anthropic');
    expect(items[0].querySelector('.model-name')?.textContent).toBe('Claude Opus 4');
    expect(items[1].querySelector('.provider')?.textContent).toBe('Anthropic');
    expect(items[1].querySelector('.model-name')?.textContent).toBe('Claude Sonnet 4.5');
    expect(items[2].querySelector('.provider')?.textContent).toBe('OpenAI');
    expect(items[3].querySelector('.provider')?.textContent).toBe('Venice');

    // The always-present custom-model action is appended after the list.
    expect(container.querySelector('.dropdown-item.custom')?.textContent).toContain(
      'Enter custom model name...'
    );
  });

  it('filters models by search text matching name, provider, or id', async () => {
    const { container } = render(SearchableModelSelect, {
      props: {
        models,
        selectedModel: '',
        provider: 'openrouter',
        onSelect: vi.fn(),
      },
    });

    const input = container.querySelector('.model-input') as HTMLInputElement;
    await fireEvent.focus(input);

    // Match by name substring.
    await fireEvent.input(input, { target: { value: 'GPT' } });
    let items = Array.from(container.querySelectorAll('.dropdown-item:not(.custom)'));
    expect(items.length).toBe(1);
    expect(items[0].querySelector('.model-name')?.textContent).toBe('GPT-4o');

    // Match by provider substring (case-insensitive).
    await fireEvent.input(input, { target: { value: 'venice' } });
    items = Array.from(container.querySelectorAll('.dropdown-item:not(.custom)'));
    expect(items.length).toBe(1);
    expect(items[0].querySelector('.provider')?.textContent).toBe('Venice');

    // Match by id substring.
    await fireEvent.input(input, { target: { value: 'claude-opus' } });
    items = Array.from(container.querySelectorAll('.dropdown-item:not(.custom)'));
    expect(items.length).toBe(1);
    expect(items[0].querySelector('.model-name')?.textContent).toBe('Claude Opus 4');

    // No matches.
    await fireEvent.input(input, { target: { value: 'nonexistent-model-xyz' } });
    items = Array.from(container.querySelectorAll('.dropdown-item:not(.custom)'));
    expect(items.length).toBe(0);
    // The custom-model action is still present even with zero matches.
    expect(container.querySelector('.dropdown-item.custom')).toBeTruthy();
  });

  it('clicking a model in the dropdown calls onSelect with that model id', async () => {
    const onSelect = vi.fn();
    const { container } = render(SearchableModelSelect, {
      props: {
        models,
        selectedModel: '',
        onSelect,
      },
    });

    const input = container.querySelector('.model-input') as HTMLInputElement;
    await fireEvent.focus(input);

    const items = Array.from(container.querySelectorAll('.dropdown-item:not(.custom)'));
    const gptItem = items.find((el) => el.querySelector('.model-name')?.textContent === 'GPT-4o') as HTMLButtonElement;
    expect(gptItem).toBeTruthy();

    // The component's mousedown handler calls preventDefault() and selects
    // directly (it does not rely on the subsequent click), matching real
    // pointer-down interaction with the dropdown.
    await fireEvent.mouseDown(gptItem);

    expect(onSelect).toHaveBeenCalledWith('openai/gpt-4o');
  });

  it('"Enter custom model name..." button opens PromptDialog', async () => {
    const { container } = render(SearchableModelSelect, {
      props: {
        models,
        selectedModel: '',
        provider: 'openrouter',
        onSelect: vi.fn(),
      },
    });

    const input = container.querySelector('.model-input') as HTMLInputElement;
    await fireEvent.focus(input);

    expect(container.querySelector('.prompt-modal')).toBeNull();

    const customBtn = container.querySelector('.dropdown-item.custom') as HTMLButtonElement;
    await fireEvent.mouseDown(customBtn);

    const promptModal = container.querySelector('.prompt-modal');
    expect(promptModal).toBeTruthy();
    expect(screen.getByText('Custom Model')).toBeTruthy();
    const promptInput = container.querySelector('.prompt-input') as HTMLInputElement;
    expect(promptInput.placeholder).toBe('e.g. gpt-4o-custom');
  });

  it('submitting a custom model name via the prompt calls onSelect with the trimmed value', async () => {
    const onSelect = vi.fn();
    const { container } = render(SearchableModelSelect, {
      props: {
        models,
        selectedModel: '',
        provider: 'openrouter',
        onSelect,
      },
    });

    const input = container.querySelector('.model-input') as HTMLInputElement;
    await fireEvent.focus(input);

    const customBtn = container.querySelector('.dropdown-item.custom') as HTMLButtonElement;
    await fireEvent.mouseDown(customBtn);

    const promptInput = container.querySelector('.prompt-input') as HTMLInputElement;
    await fireEvent.input(promptInput, { target: { value: '  my-custom-model  ' } });

    const confirmBtn = container.querySelector('.action-btn') as HTMLButtonElement;
    await fireEvent.click(confirmBtn);

    expect(onSelect).toHaveBeenCalledWith('my-custom-model');
    // Prompt closes after submission.
    expect(container.querySelector('.prompt-modal')).toBeNull();
  });

  it('cancelling the custom model prompt does not call onSelect', async () => {
    const onSelect = vi.fn();
    const { container } = render(SearchableModelSelect, {
      props: {
        models,
        selectedModel: '',
        provider: 'openrouter',
        onSelect,
      },
    });

    const input = container.querySelector('.model-input') as HTMLInputElement;
    await fireEvent.focus(input);

    const customBtn = container.querySelector('.dropdown-item.custom') as HTMLButtonElement;
    await fireEvent.mouseDown(customBtn);

    expect(container.querySelector('.prompt-modal')).toBeTruthy();

    const promptInput = container.querySelector('.prompt-input') as HTMLInputElement;
    await fireEvent.input(promptInput, { target: { value: 'abandoned-model' } });

    const cancelBtn = container.querySelector('.cancel-btn') as HTMLButtonElement;
    await fireEvent.click(cancelBtn);

    expect(onSelect).not.toHaveBeenCalled();
    expect(container.querySelector('.prompt-modal')).toBeNull();
  });

  // #2988 PR2: Venice capability-tag rendering. Copy is Matt-ruled and
  // deliberately conservative ("TEE" / "E2EE not supported in Pane", no
  // "Private" badge) — these exact strings are pinned so a future copy edit
  // breaks a test rather than shipping a false privacy claim. The two tags
  // render from two independent {#if} blocks (not an else-chain), because a
  // model can legitimately have both flags true at once.
  describe('Venice capability tags (#2988)', () => {
    it('renders the "TEE" tag when supports_tee_attestation is true', async () => {
      const capModels = [
        { id: 'tee-model', name: 'TEE Model', provider: 'Venice', supports_tee_attestation: true },
      ];
      const { container } = render(SearchableModelSelect, {
        props: { models: capModels, selectedModel: '', onSelect: vi.fn() },
      });
      await fireEvent.focus(container.querySelector('.model-input') as HTMLInputElement);

      const tags = Array.from(container.querySelectorAll('.model-tier-tag')).map((el) => el.textContent);
      expect(tags).toContain('TEE');
      expect(tags).not.toContain('E2EE not supported in Pane');
    });

    it('renders the "E2EE not supported in Pane" tag when supports_e2ee is true, and never a "Private" badge', async () => {
      const capModels = [
        { id: 'e2ee-model', name: 'E2EE Model', provider: 'Venice', supports_e2ee: true },
      ];
      const { container } = render(SearchableModelSelect, {
        props: { models: capModels, selectedModel: '', onSelect: vi.fn() },
      });
      await fireEvent.focus(container.querySelector('.model-input') as HTMLInputElement);

      const tags = Array.from(container.querySelectorAll('.model-tier-tag')).map((el) => el.textContent);
      expect(tags).toContain('E2EE not supported in Pane');
      expect(tags.some((t) => t === 'Private')).toBe(false);
      expect(container.textContent).not.toContain('Private');
    });

    it('renders BOTH tags for a model with both flags true — independent {#if} blocks, not an else-chain', async () => {
      const capModels = [
        {
          id: 'e2ee-glm-5-3-p',
          name: 'GLM 5.3P',
          provider: 'Venice',
          supports_tee_attestation: true,
          supports_e2ee: true,
        },
      ];
      const { container } = render(SearchableModelSelect, {
        props: { models: capModels, selectedModel: '', onSelect: vi.fn() },
      });
      await fireEvent.focus(container.querySelector('.model-input') as HTMLInputElement);

      const tags = Array.from(container.querySelectorAll('.model-tier-tag')).map((el) => el.textContent);
      expect(tags).toContain('TEE');
      expect(tags).toContain('E2EE not supported in Pane');
      expect(tags.length).toBe(2);
    });

    it('renders neither tag when both flags are absent/undefined — absent must never render as supported', async () => {
      const capModels = [
        { id: 'plain-model', name: 'Plain Model', provider: 'OpenRouter' },
      ];
      const { container } = render(SearchableModelSelect, {
        props: { models: capModels, selectedModel: '', onSelect: vi.fn() },
      });
      await fireEvent.focus(container.querySelector('.model-input') as HTMLInputElement);

      const tags = Array.from(container.querySelectorAll('.model-tier-tag')).map((el) => el.textContent);
      expect(tags).not.toContain('TEE');
      expect(tags).not.toContain('E2EE not supported in Pane');
    });

    it('renders neither tag when both flags are explicitly false', async () => {
      const capModels = [
        {
          id: 'false-flags-model',
          name: 'False Flags Model',
          provider: 'Venice',
          supports_tee_attestation: false,
          supports_e2ee: false,
        },
      ];
      const { container } = render(SearchableModelSelect, {
        props: { models: capModels, selectedModel: '', onSelect: vi.fn() },
      });
      await fireEvent.focus(container.querySelector('.model-input') as HTMLInputElement);

      const tags = Array.from(container.querySelectorAll('.model-tier-tag')).map((el) => el.textContent);
      expect(tags).not.toContain('TEE');
      expect(tags).not.toContain('E2EE not supported in Pane');
    });
  });

  // #3083: tag-aware search. The filter previously matched only name/
  // provider/id (SearchableModelSelect.svelte's `filteredModels`); it now
  // also matches `tier_label` and the two rendered capability-tag strings
  // ("TEE" / "E2EE not supported in Pane") via `capabilityLabels()`, so a
  // model carrying a flag surfaces under that search term even when its id
  // and name don't contain the substring.
  describe('tag-aware search (#3083)', () => {
    const tagModels = [
      // Deliberately named so the match can ONLY come from the flag/tier_label,
      // never from id/name/provider containing the search substring — this is
      // the exact distinction Reed's #3083 audit (§1) found the pre-existing
      // filter could not make.
      { id: 'vendor/model-alpha', name: 'Model Alpha', provider: 'Venice', supports_tee_attestation: true },
      { id: 'vendor/model-bravo', name: 'Model Bravo', provider: 'Venice', supports_e2ee: true },
      { id: 'vendor/model-charlie', name: 'Model Charlie', provider: 'Venice', tier_label: 'private' },
      { id: 'vendor/model-delta', name: 'Model Delta', provider: 'Venice' }, // no capability data at all
    ];

    it('matches a model by its TEE capability flag, not by id/name substring', async () => {
      const { container } = render(SearchableModelSelect, {
        props: { models: tagModels, selectedModel: '', provider: 'venice', onSelect: vi.fn() },
      });
      await fireEvent.focus(container.querySelector('.model-input') as HTMLInputElement);
      await fireEvent.input(container.querySelector('.model-input') as HTMLInputElement, { target: { value: 'TEE' } });

      const items = Array.from(container.querySelectorAll('.dropdown-item:not(.custom)'));
      expect(items.length).toBe(1);
      expect(items[0].querySelector('.model-name')?.textContent).toBe('Model Alpha');
    });

    it('matches a model by its E2EE capability flag via the rendered label text', async () => {
      const { container } = render(SearchableModelSelect, {
        props: { models: tagModels, selectedModel: '', provider: 'venice', onSelect: vi.fn() },
      });
      await fireEvent.focus(container.querySelector('.model-input') as HTMLInputElement);
      await fireEvent.input(container.querySelector('.model-input') as HTMLInputElement, { target: { value: 'e2ee' } });

      const items = Array.from(container.querySelectorAll('.dropdown-item:not(.custom)'));
      expect(items.length).toBe(1);
      expect(items[0].querySelector('.model-name')?.textContent).toBe('Model Bravo');
    });

    it('matches a model by its raw tier_label value', async () => {
      const { container } = render(SearchableModelSelect, {
        props: { models: tagModels, selectedModel: '', provider: 'venice', onSelect: vi.fn() },
      });
      await fireEvent.focus(container.querySelector('.model-input') as HTMLInputElement);
      await fireEvent.input(container.querySelector('.model-input') as HTMLInputElement, { target: { value: 'private' } });

      const items = Array.from(container.querySelectorAll('.dropdown-item:not(.custom)'));
      expect(items.length).toBe(1);
      expect(items[0].querySelector('.model-name')?.textContent).toBe('Model Charlie');
    });

    it('#3075 absent-flag guarantee: a model with no capability data never matches a "TEE" search', async () => {
      const { container } = render(SearchableModelSelect, {
        props: { models: tagModels, selectedModel: '', provider: 'venice', onSelect: vi.fn() },
      });
      await fireEvent.focus(container.querySelector('.model-input') as HTMLInputElement);
      await fireEvent.input(container.querySelector('.model-input') as HTMLInputElement, { target: { value: 'TEE' } });

      const names = Array.from(container.querySelectorAll('.dropdown-item:not(.custom) .model-name')).map((el) => el.textContent);
      expect(names).not.toContain('Model Delta');
    });

    it('still matches on name/provider/id exactly as before — tag search is additive, not a replacement', async () => {
      const { container } = render(SearchableModelSelect, {
        props: { models, selectedModel: '', provider: 'openrouter', onSelect: vi.fn() },
      });
      await fireEvent.focus(container.querySelector('.model-input') as HTMLInputElement);
      await fireEvent.input(container.querySelector('.model-input') as HTMLInputElement, { target: { value: 'GPT' } });

      const items = Array.from(container.querySelectorAll('.dropdown-item:not(.custom)'));
      expect(items.length).toBe(1);
      expect(items[0].querySelector('.model-name')?.textContent).toBe('GPT-4o');
    });

    // Reed's #3083 scope-audit pinning test (1 of 2): a model whose name/id does
    // NOT contain the typed substring, and whose capability flag/tier_label is
    // NOT set, must never appear — pins the search-scope rule itself (tags are
    // now in-scope, but only the real tags — never a false positive).
    it('search scope pin: a model with neither a matching substring nor a matching capability never appears', async () => {
      const { container } = render(SearchableModelSelect, {
        props: { models: tagModels, selectedModel: '', provider: 'venice', onSelect: vi.fn() },
      });
      await fireEvent.focus(container.querySelector('.model-input') as HTMLInputElement);
      await fireEvent.input(container.querySelector('.model-input') as HTMLInputElement, { target: { value: 'TEE' } });

      const names = Array.from(container.querySelectorAll('.dropdown-item:not(.custom) .model-name')).map((el) => el.textContent);
      expect(names).toEqual(['Model Alpha']);
    });

    // Reed's #3083 scope-audit pinning test (2 of 2): a custom (typed, non-list)
    // entry can never carry a capability tag — structurally guaranteed today
    // because no code path attaches tags to a hand-typed string (displayText()
    // falls back to the raw string with no Model object at all). Pinning this
    // before any future change adds a lookup path that could violate it.
    it('a custom (non-list) selection never shows a capability tag — structural, not assumed', () => {
      const { container } = render(SearchableModelSelect, {
        props: { models: tagModels, selectedModel: 'totally-made-up-model-id', provider: 'venice', onSelect: vi.fn() },
      });

      // displayText() falls back to the raw typed id — no Model object, so no
      // tags render for the selected value anywhere (the dropdown itself is
      // closed here; this just asserts the closed-state display carries
      // nothing tag-like).
      const input = container.querySelector('.model-input') as HTMLInputElement;
      expect(input.value).toBe('totally-made-up-model-id');
      expect(container.querySelector('.model-tier-tag')).toBeNull();
    });
  });

  // #3083: the custom "Enter custom model name..." entry box is gated by
  // `provider` — shown only where the provider's own catalog is itself
  // incomplete by design (OpenRouter: an open, third-party-aggregated
  // namespace). Hidden for Venice/Anthropic/Ollama, whose fetched model list
  // is each provider's own complete, authoritative live list — a typed name
  // that isn't in it is almost certainly a typo, not a real unlisted model.
  describe('custom-model box gated by namespace (#3083)', () => {
    it('shows the custom-entry box for OpenRouter (open namespace)', async () => {
      const { container } = render(SearchableModelSelect, {
        props: { models, selectedModel: '', provider: 'openrouter', onSelect: vi.fn() },
      });
      await fireEvent.focus(container.querySelector('.model-input') as HTMLInputElement);

      expect(container.querySelector('.dropdown-item.custom')).toBeTruthy();
    });

    it('hides the custom-entry box for Venice (live/authoritative list)', async () => {
      const { container } = render(SearchableModelSelect, {
        props: { models, selectedModel: '', provider: 'venice', onSelect: vi.fn() },
      });
      await fireEvent.focus(container.querySelector('.model-input') as HTMLInputElement);

      expect(container.querySelector('.dropdown-item.custom')).toBeNull();
    });

    it('hides the custom-entry box for Anthropic (live/authoritative list)', async () => {
      const { container } = render(SearchableModelSelect, {
        props: { models, selectedModel: '', provider: 'anthropic', onSelect: vi.fn() },
      });
      await fireEvent.focus(container.querySelector('.model-input') as HTMLInputElement);

      expect(container.querySelector('.dropdown-item.custom')).toBeNull();
    });

    it('hides the custom-entry box for Ollama (live/authoritative list)', async () => {
      const { container } = render(SearchableModelSelect, {
        props: { models, selectedModel: '', provider: 'ollama', onSelect: vi.fn() },
      });
      await fireEvent.focus(container.querySelector('.model-input') as HTMLInputElement);

      expect(container.querySelector('.dropdown-item.custom')).toBeNull();
    });

    it('defaults to hidden when `provider` is omitted — fail safe, not fail open', async () => {
      const { container } = render(SearchableModelSelect, {
        props: { models, selectedModel: '', onSelect: vi.fn() },
      });
      await fireEvent.focus(container.querySelector('.model-input') as HTMLInputElement);

      expect(container.querySelector('.dropdown-item.custom')).toBeNull();
    });
  });
});

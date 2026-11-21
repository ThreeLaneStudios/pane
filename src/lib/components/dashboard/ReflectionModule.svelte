<script lang="ts">
  import { onMount } from 'svelte';
  import * as api from '$lib/api';
  import type { DashboardModule } from '$lib/types/dashboard';
  
  interface Props {
    module: DashboardModule;
  }
  
  let { module }: Props = $props();
  
  interface ReflectionEntry {
    id: string;
    question_id: string;
    entry_date: string;
    value_numeric: number | null;
    value_bool: boolean | null;
    value_text: string | null;
  }
  
  const todayStr = new Date().toISOString().split('T')[0];
  
  let answers: Record<string, any> = $state({});
  let loading = $state(true);
  
  onMount(async () => {
    await loadEntries();
  });
  
  async function loadEntries() {
    try {
      const entries: ReflectionEntry[] = await api.getReflectionEntries(todayStr);
      // Populate answers from saved entries
      for (const entry of entries) {
        const item = module.items.find(i => i.id === entry.question_id);
        if (!item) continue;
        
        const cfg = getQuestionConfig(item);
        if (cfg.type === 'scale') {
          answers[entry.question_id] = entry.value_numeric;
        } else if (cfg.type === 'yesno') {
          answers[entry.question_id] = entry.value_bool ? 'yes' : 'no';
        } else if (cfg.type === 'multiple_choice' || cfg.type === 'long_form') {
          answers[entry.question_id] = entry.value_text;
        }
      }
      loading = false;
    } catch (e) {
      console.error('Failed to load reflection entries:', e);
      loading = false;
    }
  }
  
  function getQuestionConfig(item: any): { type: string; options: string[] } {
    try {
      const cfg = JSON.parse(item.config_json || '{}');
      return { type: cfg.answer_type || 'long_form', options: cfg.options || [] };
    } catch { return { type: 'long_form', options: [] }; }
  }
  
  async function saveAnswer(itemId: string, value: any, type: string) {
    let valueNumeric: number | undefined;
    let valueBool: boolean | undefined;
    let valueText: string | undefined;
    
    if (type === 'scale') {
      valueNumeric = Number(value);
    } else if (type === 'yesno') {
      valueBool = value === 'yes';
    } else {
      valueText = String(value);
    }
    
    try {
      await api.saveReflectionEntry(itemId, todayStr, valueNumeric ?? null, valueBool ?? null, valueText ?? null);
    } catch (e) {
      console.error('Failed to save reflection entry:', e);
    }
  }
  
  function handleScaleClick(itemId: string, value: number) {
    answers[itemId] = value;
    answers = { ...answers }; // Trigger reactivity
    saveAnswer(itemId, value, 'scale');
  }
  
  function handleYesNoClick(itemId: string, value: string) {
    answers[itemId] = value;
    answers = { ...answers }; // Trigger reactivity
    saveAnswer(itemId, value, 'yesno');
  }
  
  function handleMCClick(itemId: string, value: string) {
    answers[itemId] = value;
    answers = { ...answers }; // Trigger reactivity
    saveAnswer(itemId, value, 'multiple_choice');
  }
  
  function handleTextChange(itemId: string, value: string) {
    answers[itemId] = value;
    // Debounce save on text input
    clearTimeout(textSaveTimers[itemId]);
    textSaveTimers[itemId] = setTimeout(() => {
      saveAnswer(itemId, value, 'long_form');
    }, 1000);
  }
  
  let textSaveTimers: Record<string, any> = {};
</script>

<div class="card">
  <div class="card-label">EVENING REFLECTION</div>
  
  {#if loading}
    <div class="loading">Loading...</div>
  {:else if module.items.length === 0}
    <div class="empty">No reflection questions configured.</div>
  {:else}
    <div class="reflection-list">
      {#each module.items as item (item.id)}
        {@const qcfg = getQuestionConfig(item)}
        <div class="reflection-item">
          <div class="reflection-label">{item.label}</div>
          
          {#if qcfg.type === 'scale'}
            <div class="scale-row">
              <div class="scale-buttons">
                {#each Array(10) as _, n}
                  <button
                    class="scale-btn"
                    class:selected={answers[item.id] === n + 1}
                    onclick={() => handleScaleClick(item.id, n + 1)}
                  >{n + 1}</button>
                {/each}
              </div>
            </div>
          {:else if qcfg.type === 'yesno'}
            <div class="toggle-row">
              <button
                class="toggle-btn"
                class:selected={answers[item.id] === 'yes'}
                onclick={() => handleYesNoClick(item.id, 'yes')}
              >Yes</button>
              <button
                class="toggle-btn"
                class:selected={answers[item.id] === 'no'}
                onclick={() => handleYesNoClick(item.id, 'no')}
              >No</button>
            </div>
          {:else if qcfg.type === 'multiple_choice'}
            <div class="mc-group">
              {#each qcfg.options as opt}
                <button
                  class="mc-btn"
                  class:selected={answers[item.id] === opt}
                  onclick={() => handleMCClick(item.id, opt)}
                >{opt}</button>
              {/each}
            </div>
          {:else}
            <textarea
              class="text-input"
              placeholder="Write here..."
              rows={3}
              value={answers[item.id] || ''}
              oninput={(e) => handleTextChange(item.id, e.currentTarget.value)}
            ></textarea>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .card {
    background: rgba(255,255,255,0.05);
    border: 1px solid rgba(255,255,255,0.06);
    border-radius: 4px;
    padding: 22px 24px;
    position: relative;
  }
  
  .card-label {
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    font-size: 14px;
    font-weight: 700;
    color: #666666;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    margin-bottom: 16px;
  }
  
  .reflection-list {
    display: flex;
    flex-direction: column;
    gap: 18px;
  }
  
  .reflection-item {
    padding-bottom: 16px;
    border-bottom: 1px solid rgba(255,255,255,0.04);
  }
  .reflection-item:last-child { border-bottom: none; padding-bottom: 0; }
  
  .reflection-label {
    font-size: 13px;
    color: #888888;
    margin-bottom: 10px;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }
  
  .scale-row {
    display: flex;
    align-items: center;
    gap: 14px;
  }
  
  .scale-buttons {
    display: flex;
    gap: 4px;
  }
  
  .scale-btn {
    width: 32px;
    height: 32px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: #0a0a0f;
    border: 1px solid rgba(255,255,255,0.1);
    border-radius: 4px;
    color: #888888;
    font-size: 12px;
    cursor: pointer;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    transition: all 0.15s;
  }
  .scale-btn:hover { border-color: #00E4E4; color: #00E4E4; }
  .scale-btn.selected { background: rgba(0,228,228,0.15); border-color: #00E4E4; color: #00E4E4; }
  
  .toggle-row { display: flex; gap: 10px; }
  .toggle-btn {
    padding: 8px 24px;
    border-radius: 4px;
    background: #0a0a0f;
    border: 1px solid rgba(255,255,255,0.1);
    color: #888888;
    font-size: 13px;
    cursor: pointer;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    transition: all 0.15s;
  }
  .toggle-btn:hover { border-color: #00E4E4; color: #00E4E4; }
  .toggle-btn.selected { background: rgba(0,228,228,0.15); border-color: #00E4E4; color: #00E4E4; }
  
  .mc-group {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  
  .mc-btn {
    padding: 10px 16px;
    border-radius: 4px;
    background: #0a0a0f;
    border: 1px solid rgba(255,255,255,0.1);
    color: #888888;
    font-size: 13px;
    text-align: left;
    cursor: pointer;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    transition: all 0.15s;
  }
  .mc-btn:hover { border-color: #00E4E4; color: #00E4E4; }
  .mc-btn.selected { background: rgba(0,228,228,0.15); border-color: #00E4E4; color: #00E4E4; }
  
  .text-input {
    width: 100%;
    padding: 10px 14px;
    border-radius: 4px;
    border: 1px solid rgba(255,255,255,0.08);
    background: #0a0a0f;
    color: #e0e0e0;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    font-size: 13px;
    line-height: 1.5;
    outline: none;
    resize: vertical;
    transition: border-color 0.2s;
  }
  .text-input:focus { border-color: rgba(0,228,228,0.4); }
  .text-input::placeholder { color: #666666; }
  
  .empty, .loading {
    padding: 16px;
    text-align: center;
    color: #666666;
    font-size: 13px;
  }
</style>

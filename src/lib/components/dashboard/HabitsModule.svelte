<script lang="ts">
  import { onMount } from 'svelte';
  import type { DashboardModule } from '$lib/types/dashboard';
  import * as api from '$lib/api';
  
  interface Props {
    module: DashboardModule;
  }
  
  let { module }: Props = $props();
  
  interface HabitStatus {
    item_id: string;
    label: string;
    emoji: string | null;
    completed: boolean;
    streak: number;
  }
  
  let habits: HabitStatus[] = $state([]);
  let loading = $state(true);
  
  const todayStr = new Date().toISOString().split('T')[0];
  
  onMount(async () => {
    await loadHabits();
  });
  
  async function loadHabits() {
    try {
      habits = await api.getHabitsToday();
      loading = false;
    } catch (e) {
      console.error('Failed to load habits:', e);
      loading = false;
    }
  }
  
  async function toggleHabit(habit: HabitStatus) {
    const newCompleted = !habit.completed;
    try {
      const result = await api.toggleHabit(habit.item_id, todayStr, newCompleted);
      // Update local state with new streak data
      habit.completed = newCompleted;
      habit.streak = result.current_streak;
      habits = [...habits]; // Trigger reactivity
    } catch (e) {
      console.error('Failed to toggle habit:', e);
    }
  }
</script>

<div class="card">
  <div class="card-label">DAILY HABITS</div>
  
  {#if loading}
    <div class="loading">Loading habits...</div>
  {:else if habits.length === 0}
    <div class="empty">No habits configured. Add them in Settings.</div>
  {:else}
    <div class="habits-list">
      {#each habits as habit (habit.item_id)}
        <div class="habit-row">
          <label class="habit-label">
            <input
              type="checkbox"
              class="habit-checkbox"
              checked={habit.completed}
              onchange={() => toggleHabit(habit)}
            />
            <span class="habit-name">{habit.label}</span>
          </label>
          <div class="streak">Day {habit.streak}</div>
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .card {
    background: rgba(255,255,255,0.05);
    border: 1px solid rgba(255, 255, 255, 0.06);
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
  
  .habits-list {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  
  .habit-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  
  .habit-label {
    display: flex;
    align-items: center;
    gap: 10px;
    cursor: pointer;
    flex: 1;
  }
  
  .habit-checkbox {
    width: 18px;
    height: 18px;
    accent-color: #00E4E4;
    cursor: pointer;
  }
  
  .habit-name {
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    font-size: 13px;
    color: #e0e0e0;
    font-weight: 500;
  }
  
  .streak {
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    font-size: 12px;
    color: #666666;
    font-weight: 500;
  }
  
  .loading, .empty {
    padding: 16px;
    text-align: center;
    color: #888888;
    font-size: 13px;
  }
</style>

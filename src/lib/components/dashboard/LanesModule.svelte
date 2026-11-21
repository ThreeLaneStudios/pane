<script lang="ts">
  import type { DashboardModule } from '$lib/types/dashboard';
  
  interface Props {
    module: DashboardModule;
  }
  
  let { module }: Props = $props();
</script>

<div class="card">
  <div class="card-label">FOCUS LANES</div>
  <div class="lanes">
    {#each module.items as lane (lane.id)}
      <div class="lane">
        <div class="lane-name">{lane.label}</div>
        {#if lane.description}
          <div class="lane-desc">{lane.description}</div>
        {/if}
      </div>
    {/each}
  </div>
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
    margin-bottom: 12px;
  }
  
  .lanes {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  
  .lane {
    padding: 14px 16px;
    border-radius: 4px;
    background: rgba(0, 228, 228, 0.04);
    border: 1px solid rgba(0, 228, 228, 0.08);
  }
  
  .lane-name {
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    font-size: 16px;
    color: #e0e0e0;
    font-weight: 600;
    margin-bottom: 4px;
  }
  
  .lane-desc {
    font-size: 13px;
    color: #888888;
    line-height: 1.5;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }
</style>

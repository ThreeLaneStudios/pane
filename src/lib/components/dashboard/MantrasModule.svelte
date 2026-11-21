<script lang="ts">
  import type { DashboardModule } from '$lib/types/dashboard';
  
  interface Props {
    module: DashboardModule;
  }
  
  let { module }: Props = $props();
</script>

<div class="card">
  <div class="card-label">DAILY MANTRAS</div>
  <div class="mantras">
    {#each module.items as mantra (mantra.id)}
      <div class="mantra">{mantra.label}</div>
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
  
  .mantras {
    display: flex;
    gap: 12px;
    flex-wrap: wrap;
  }
  
  .mantra {
    flex: 1;
    min-width: 200px;
    padding: 14px;
    text-align: center;
    border-radius: 4px;
    background: rgba(0,228,228,0.04);
    border: 1px solid rgba(0,228,228,0.08);
    font-size: 14px;
    font-weight: 600;
    color: #e0e0e0;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }
</style>

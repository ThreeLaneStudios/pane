<script lang="ts">
  import type { DashboardModule } from '$lib/types/dashboard';
  
  interface Props {
    module: DashboardModule;
  }
  
  let { module }: Props = $props();
</script>

<div class="card">
  <div class="card-label">DAILY AFFIRMATION</div>
  <div class="affirmation-body">
    {module.static_content || 'No affirmation set yet.'}
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
  
  .affirmation-body {
    font-size: 14px;
    line-height: 1.8;
    color: #e0e0e0;
    white-space: pre-wrap;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }
</style>

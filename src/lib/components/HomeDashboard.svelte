<script lang="ts">
  // Home view — ChatSidebar now rendered in +page.svelte with layoutMode="centered"
  // This component can be extended with other home modules (tasks, calendar, etc.)
</script>

<div class="home-container">
  <div class="home-content">
    <!-- Future: Add home modules here (TasksModule, CalendarWidget, etc.) -->
    <!-- ChatSidebar is now always rendered in +page.svelte -->
  </div>
</div>

<style>
  .home-container {
    display: flex;
    justify-content: center;
    height: 100%;
    width: 100%;
    background: #0a0a0f;
    overflow: hidden;
  }

  .home-content {
    width: 100%;
    height: 100%;
    display: flex;
    flex-direction: column;
  }
</style>

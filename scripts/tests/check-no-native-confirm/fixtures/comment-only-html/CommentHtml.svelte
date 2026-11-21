<script lang="ts">
  let showConfirm = false;
</script>
<!-- replaces the native confirm() that never renders in Tauri's
     WebView (silently resolves false). -->
{#if showConfirm}
  <ConfirmDialog title="Delete?" onConfirm={() => {}} onCancel={() => {}} />
{/if}

<script lang="ts">
  let showConfirm = false;
</script>
<span>Enter the code from Pane Gateway's terminal on the OC machine.</span>
<!-- replaces the native confirm() that never renders in Tauri's
     WebView (silently resolves false). -->
{#if showConfirm}
  <ConfirmDialog title="Delete?" onConfirm={() => {}} onCancel={() => {}} />
{/if}

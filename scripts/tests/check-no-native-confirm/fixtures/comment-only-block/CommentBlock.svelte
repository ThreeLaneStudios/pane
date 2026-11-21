<script lang="ts">
  /* native confirm() never renders in this WebView — replaced below */
  let showConfirm = false;
</script>
{#if showConfirm}
  <ConfirmDialog title="Delete?" onConfirm={() => {}} onCancel={() => {}} />
{/if}

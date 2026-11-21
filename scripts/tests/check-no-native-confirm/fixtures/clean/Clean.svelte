<script lang="ts">
  let showConfirm = false;
  function deleteItem() {
    showConfirm = true;
  }
</script>
{#if showConfirm}
  <ConfirmDialog title="Delete?" onConfirm={() => {}} onCancel={() => {}} />
{/if}

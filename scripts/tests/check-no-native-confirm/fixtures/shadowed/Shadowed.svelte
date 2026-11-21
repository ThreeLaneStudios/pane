<script lang="ts">
  let resolvePromise: ((v: boolean) => void) | null = null;
  function confirm() {
    resolvePromise?.(true);
  }
  function onSelect() {
    confirm();
  }
</script>

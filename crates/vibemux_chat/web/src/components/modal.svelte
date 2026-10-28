<script>
  import { onMount } from "svelte";
  let { title, on_close, class_name = "", children } = $props();
  let node;
  onMount(() => {
    const opener = document.activeElement;
    node.showModal();
    return () => {
      node.close();
      if (opener?.isConnected) opener.focus();
    };
  });
</script>

<dialog
  bind:this={node}
  class={`dialog ${class_name}`}
  aria-label={title}
  oncancel={(e) => {
    e.preventDefault();
    on_close();
  }}
  onclick={(e) => {
    if (e.target === node) on_close();
  }}
  onkeydown={(e) => e.stopPropagation()}
>
  {@render children()}
</dialog>

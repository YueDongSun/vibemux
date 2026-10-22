<script lang="ts">
  import { console_views, type console_view_id } from '../console_policy';

  let {
    active,
    on_select,
  }: { active: console_view_id; on_select: (view: console_view_id) => void } = $props();
</script>

<nav class="view_tabs" aria-label="Monitor views">
  {#each console_views as view, index (view.id)}
    <button
      type="button"
      class="view_tab"
      class:active={active === view.id}
      aria-current={active === view.id ? 'page' : undefined}
      onclick={() => on_select(view.id)}
    >
      <span class="view_key" aria-hidden="true">{index + 1}</span>
      {view.label}
    </button>
  {/each}
</nav>

<script lang="ts">
  import { untrack } from 'svelte';
  import type { WorkspaceCleanupState, WorkspaceCleanupSnapshot, WorkspaceCleanupTarget } from '../lib/workspace_cleanup_state';

  let { controller, target }: { controller: WorkspaceCleanupState; target: WorkspaceCleanupTarget | null } = $props();
  let view = $state<WorkspaceCleanupSnapshot>(untrack(() => controller.snapshot()));
  let target_key = $derived(target ? JSON.stringify({ registration_key: target.registration_key, task_id: target.task_id, run_id: target.run_id }) : '');
  let scope_matches = $derived(target !== null && view.target !== null && target.registration_key === view.target.registration_key
    && target.task_id.toLowerCase() === view.target.task_id && target.run_id.toLowerCase() === view.target.run_id);
  $effect(() => {
    const current = controller;
    const selected = target_key ? JSON.parse(target_key) as WorkspaceCleanupTarget : null;
    const unsubscribe = current.subscribe(next => { view = next; });
    void current.bind(selected);
    current.start_polling();
    return () => { unsubscribe(); current.stop_polling(); void current.bind(null); };
  });
</script>

<div class="workspace_cleanup" role="group" aria-label="Worktree removal" aria-busy={view.loading || view.busy}>
  {#if !scope_matches}<p class="removal_status" role="status">Checking removal…</p>
  {:else}
  {#if view.review_open && view.cleanup?.phase === 'planned'}
    <div class="removal_review">
      <p class="review_title">Review removal</p>
      <p class="branch"><code>{view.cleanup.branch}</code></p>
      <p>The branch and conversation history will be kept.</p>
      <div class="removal_actions">
        <button type="button" onclick={() => controller.dismiss_preview()}>Keep worktree</button>
        <button type="button" class="remove_button" disabled={!view.can_confirm} onclick={() => void controller.confirm()}>Remove worktree</button>
      </div>
    </div>
  {:else if view.cleanup?.phase === 'removed'}
    <p class="removal_status" role="status">Worktree removed.</p>
    <p class="retention">The branch and conversation history were kept.</p>
  {:else}
    {#if view.cleanup?.phase === 'refused'}<p class="removal_status" role="status">Worktree kept.</p>{/if}
    {#if view.busy || view.cleanup?.phase === 'claimed' || view.cleanup?.phase === 'armed' || view.cleanup?.phase === 'quiescent'}
      <p class="removal_status" role="status">Checking removal…</p>
    {/if}
    <div class="removal_actions">
      <button type="button" disabled={!view.can_plan} onclick={() => void controller.plan()}>Review removal</button>
      <button type="button" disabled={!view.can_refresh} onclick={() => void controller.refresh_status()}>{view.loading ? 'Checking…' : 'Refresh status'}</button>
      {#if view.cleanup && ['claimed', 'armed', 'unknown', 'quiescent'].includes(view.cleanup.phase)}
        <button type="button" disabled={!view.can_reconcile} onclick={() => void controller.reconcile()}>Check outcome</button>
      {/if}
    </div>
  {/if}
  {#if view.error}<p class="removal_status" role="alert">{view.error}</p>
  {:else if view.uncertain}<p class="removal_status" role="status">Removal is unconfirmed. Refresh status to check the outcome.</p>{/if}
  {/if}
</div>

<style>
  .workspace_cleanup{min-width:0;font-size:12px}.removal_review{border:1px solid var(--border);border-radius:8px;padding:12px}.review_title{font-weight:600;margin:0 0 8px}.branch{overflow-wrap:anywhere}.removal_review p{line-height:1.5}.removal_actions{display:flex;flex-wrap:wrap;gap:8px}.removal_actions button{font:inherit;white-space:nowrap}.remove_button{color:var(--danger,#b42318)}.removal_status,.retention{margin:9px 0;color:var(--muted_text);font-size:11px;line-height:1.5;overflow-wrap:anywhere}
</style>

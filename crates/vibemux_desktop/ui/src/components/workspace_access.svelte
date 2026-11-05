<script lang="ts">
  import { onMount } from 'svelte';
  import { load_backend } from '../lib/backend';
  import { command_code } from '../lib/native_state';
  import { WorkspaceAccessState, type WorkspaceAccessView } from '../lib/workspace_access_state';

  let { project_key, project_id, task_id, run_id, branch, has_worktree }: {
    project_key: string; project_id: string; task_id: string; run_id: string; branch: string | null; has_worktree: boolean;
  } = $props();
  let model = $state<WorkspaceAccessState | null>(null);
  let view = $state<WorkspaceAccessView | null>(null);
  let startup_error = $state('');
  let fixture_mode = $state(false);

  $effect(() => { model?.set_availability({ has_worktree, has_branch: Boolean(branch) }); });

  onMount(() => {
    // The parent keys this component by its immutable project/task/run scope.
    const context = { project_key, project_id, task_id, run_id };
    let disposed = false;
    void (async () => {
      try {
        const backend = await load_backend();
        if (disposed) return;
        fixture_mode = backend.mode === 'fixture';
        model = new WorkspaceAccessState(context, { has_worktree, has_branch: Boolean(branch) },
          (request) => backend.invoke('open_link', { request }),
          (next) => { if (!disposed) view = next; }, fixture_mode);
        view = model.snapshot();
      } catch (error: unknown) {
        if (!disposed) startup_error = `Worktree actions unavailable (${command_code(error)}).`;
      }
    })();
    return () => { disposed = true; model?.dispose(); };
  });
</script>

<div class="workspace_access" role="group" aria-label="Worktree actions" aria-busy={Boolean(view?.pending)}>
  <div class="workspace_actions">
    <button disabled={!view?.can_open} onclick={() => void model?.open_folder()}>{view?.pending === 'open' ? 'Opening…' : 'Open worktree folder'}</button>
    <button disabled={!view?.can_copy_path} onclick={() => void model?.copy_path()}>{view?.pending === 'copy_path' ? 'Copying…' : 'Copy worktree path'}</button>
    <button disabled={!view?.can_copy_branch} onclick={() => void model?.copy_branch()}>{view?.pending === 'copy_branch' ? 'Copying…' : 'Copy branch'}</button>
  </div>
  {#if fixture_mode}<p class="workspace_access_status" role="status">DEMO · Worktree actions are unavailable.</p>{/if}
  {#if startup_error}<p class="workspace_access_status" role="alert">{startup_error}</p>
  {:else if !view}<p class="workspace_access_status" role="status">Loading worktree actions…</p>
  {:else if view.error}<p class="workspace_access_status" role="alert">{view.error}</p>
  {:else if view.notice}<p class="workspace_access_status" role="status">{view.notice}</p>{/if}
</div>

<style>
  .workspace_access{min-width:0;font-size:12px}.workspace_actions{display:flex;flex-wrap:wrap;gap:8px}.workspace_actions button{font:inherit;white-space:nowrap}.workspace_access_status{margin:9px 0 0;color:var(--muted_text);font-size:11px;line-height:1.5;overflow-wrap:anywhere}
</style>

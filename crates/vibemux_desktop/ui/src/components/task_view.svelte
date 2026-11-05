<script lang="ts">
  import { MAX_LOADED_TASKS, filter_tasks, task_status_label, type ReadViewState, type TaskFilter } from '../lib/read_state';
  import TaskDetail from './task_detail.svelte';
  let { view, on_refresh, on_more, on_select, on_filter, on_close_detail, on_open_window, opening_window = false }: {
    view: ReadViewState; on_refresh: () => void; on_more: () => void;
    on_select: (task_id: string) => void; on_filter: (filter: TaskFilter) => void; on_close_detail: () => void;
    on_open_window?: (task_id: string) => void; opening_window?: boolean;
  } = $props();
  const filters: { value: TaskFilter; label: string }[] = [{ value: 'all', label: 'All' }, { value: 'running', label: 'Running' }, { value: 'blocked', label: 'Blocked' }, { value: 'done', label: 'Done' }];
  let shown = $derived(filter_tasks(view.tasks, view.task_filter));
</script>

<section class="task_workspace" aria-labelledby="tasks_heading">
  <header class="read_header">
    <div><h1 id="tasks_heading">Tasks</h1><p>Canonical task state and read-only run metadata.</p></div>
    <button disabled={!view.project_key || view.tasks_loading} onclick={on_refresh}>{view.tasks_loading ? 'Reading…' : 'Refresh'}</button>
  </header>
  {#if !view.project_key}
    <div class="empty_read"><h2>Choose a project</h2><p>Select a registered project to read its tasks.</p></div>
  {:else}
    <div class="task_toolbar"><div class="filter_group" aria-label="Filter loaded tasks">{#each filters as filter}<button class:active={view.task_filter === filter.value} aria-pressed={view.task_filter === filter.value} onclick={() => on_filter(filter.value)}>{filter.label}</button>{/each}</div><span>{shown.length} shown · {view.tasks.length} loaded</span></div>
    {#if view.tasks_error}<p class="read_notice" role="alert">{view.tasks_error}</p>{/if}
    <p class="freshness">{view.tasks_observed_at ? `Observed ${view.tasks_observed_at}` : 'No task observation yet.'} Filters apply to loaded pages.</p>
    <div class="task_layout">
      <div class="task_list" aria-busy={view.tasks_loading}>
        {#if !view.tasks_loaded && view.tasks_loading}<p class="empty_read">Reading task metadata…</p>
        {:else if !shown.length}<p class="empty_read">{view.tasks_loaded ? 'No loaded tasks match this filter.' : 'Refresh to read the project’s tasks.'}</p>
        {/if}
        <div class="task_grid">{#each shown as task (task.task_id)}
          <button class="task_card" class:selected={view.selected_task === task.task_id} aria-pressed={view.selected_task === task.task_id} onclick={() => on_select(task.task_id)}>
            <span class="task_status" data-status={task.status}>{task_status_label(task.status)}</span>
            <strong>{task.title || 'Untitled task'}</strong>
            <code>{task.task_id}</code>
            <small>Event #{task.latest_sequence} · {task.detail_revision ? `revision ${task.detail_revision}` : 'revision not provided'}</small>
            {#if task.title_truncated}<small>Title shortened in the source snapshot</small>{/if}
          </button>
        {/each}</div>
        {#if view.next_task_cursor}
          <div class="pagination"><button disabled={view.tasks_loading || view.tasks.length >= MAX_LOADED_TASKS} onclick={on_more}>Load next page</button><span>{view.tasks.length >= MAX_LOADED_TASKS ? `The view retains at most ${MAX_LOADED_TASKS} tasks. Refresh starts from the first page.` : 'More tasks are available.'}</span></div>
        {/if}
      </div>
      {#if view.selected_task}
        <aside class="task_detail" aria-label="Task details" aria-busy={view.detail_loading}>
          <TaskDetail project_key={view.project_key ?? undefined} task_id={view.selected_task} detail={view.detail} loading={view.detail_loading} error={view.detail_error} on_refresh={() => on_select(view.selected_task ?? '')} on_close={on_close_detail} {on_open_window} {opening_window} />
        </aside>
      {/if}
    </div>
  {/if}
</section>

<style>
  .task_workspace{display:flex;flex:1;flex-direction:column;min-width:0;min-height:0;background:var(--bg);color:var(--text_primary)}
  .read_header{display:flex;align-items:center;justify-content:space-between;gap:18px;padding:25px 28px 16px}.read_header h1{font:400 30px/1.2 var(--font_display);margin:0}.read_header p,.freshness{color:var(--muted_text);font-size:12px}.read_header p{margin:7px 0 0}.task_toolbar{display:flex;align-items:center;justify-content:space-between;gap:16px;flex-wrap:wrap;padding:0 28px}.task_toolbar>span{font-size:12px;color:var(--muted_text)}.filter_group{display:flex;gap:6px}.filter_group button{border-color:transparent;background:transparent;font-size:12px}.filter_group .active{background:var(--accent_tint);color:var(--accent_text);border-color:var(--accent_edge)}.freshness{margin:12px 28px 16px}.read_notice{margin:12px 28px;padding:12px 14px;border:1px solid var(--border);border-radius:9px;background:var(--surface);font-size:12px}.task_layout{display:flex;flex:1;min-height:0;min-width:0;border-top:1px solid var(--border)}.task_list{flex:1;min-width:0;overflow:auto;padding:22px 28px}.task_grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(min(240px,100%),1fr));gap:12px}.task_card{display:flex;flex-direction:column;gap:10px;align-items:flex-start;text-align:left;padding:17px;border:1px solid var(--border);border-radius:12px;background:var(--surface_alt);min-width:0}.task_card.selected{border-color:var(--accent_edge);box-shadow:inset 0 0 0 1px var(--accent_edge)}.task_card strong{font-weight:600;font-size:14px;line-height:1.45;overflow-wrap:anywhere}.task_card code{font-size:10px;overflow-wrap:anywhere;color:var(--muted_text)}.task_card small{font-size:10px;color:var(--muted_text)}.task_status{font-size:10px;border:1px solid var(--border);border-radius:20px;padding:3px 7px;color:var(--muted_text);background:var(--surface)}.task_status[data-status=in_progress]{color:var(--accent_text);background:var(--accent_tint)}.task_status[data-status=done]{color:var(--success)}.empty_read{padding:42px 28px;color:var(--muted_text);font-size:13px}.empty_read h2{font:24px var(--font_display);color:var(--text_primary)}.pagination{display:flex;align-items:center;gap:12px;padding:22px 0;font-size:11px;color:var(--muted_text)}.task_detail{width:420px;min-width:300px;max-width:65%;flex-shrink:0;resize:horizontal;overflow:auto;padding:20px;border-left:1px solid var(--border);background:var(--surface_alt)}
  @media(max-width:900px){.task_layout{flex-direction:column;overflow:auto}.task_list{overflow:visible;flex:initial}.task_detail{width:100%;min-width:0;max-width:100%;resize:none;border-left:0;border-top:1px solid var(--border)}.read_header,.task_toolbar{padding-left:18px;padding-right:18px}.freshness{margin-left:18px;margin-right:18px}.task_list{padding:18px}}
</style>

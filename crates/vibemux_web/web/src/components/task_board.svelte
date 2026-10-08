<script lang="ts">
  import type { freshness, task_status } from '../api_client';
  import { display_task_title } from '../presentation_policy';

  let {
    tasks,
    selected_id,
    freshness: data_freshness,
    on_select,
  }: {
    tasks: task_status[];
    selected_id: string | null;
    freshness: freshness | null;
    on_select: (task_id: string) => void;
  } = $props();

</script>

<section class="panel task_panel" aria-labelledby="task_list_heading">
  <div class="panel_heading">
    <div>
      <p class="eyebrow">Workspace</p>
      <h2 id="task_list_heading">Tasks</h2>
    </div>
    <span class="count_badge" aria-label={`${tasks.length} tasks`}>{tasks.length}</span>
  </div>

  {#if tasks.length === 0}
    <p class="empty_state">No tasks are available from the local daemon.</p>
  {:else}
    <ul class="task_list">
      {#each tasks as task (task.task_id)}
        <li>
          <button
            class:selected={selected_id === task.task_id}
            class="task_row"
            type="button"
            onclick={() => on_select(task.task_id)}
            aria-current={selected_id === task.task_id ? 'true' : undefined}
          >
            <span class="task_row_top">
              <span class="task_title">{display_task_title(task.task_id, task.title, data_freshness?.stale ?? false)}</span>
              <span class="state_pill" data-state={task.state}>{task.state}</span>
            </span>
            <span class="task_row_meta">{task.kind} · {task.runs.length} run{task.runs.length === 1 ? '' : 's'}</span>
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</section>

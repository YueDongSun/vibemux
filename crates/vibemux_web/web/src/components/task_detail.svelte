<script lang="ts">
  import type { freshness, task_status } from '../api_client';
  import { display_task_title } from '../presentation_policy';

  let {
    task,
    freshness: data_freshness,
  }: { task: task_status | null; freshness: freshness | null } = $props();

</script>

<section class="panel detail_panel" aria-labelledby="detail_heading" aria-live="polite">
  <div class="panel_heading detail_heading">
    <div>
      <p class="eyebrow">Task detail</p>
      <h2 id="detail_heading">{task ? display_task_title(task.task_id, task.title, data_freshness?.stale ?? false) : 'Select a task'}</h2>
    </div>
    {#if task}<span class="state_pill" data-state={task.state}>{task.state}</span>{/if}
  </div>

  {#if !task}
    <p class="empty_state">Choose a task to inspect its current run records.</p>
  {:else}
    <dl class="detail_grid">
      <div><dt>Task ID</dt><dd class="mono">{task.task_id}</dd></div>
      <div><dt>Type</dt><dd>{task.kind}</dd></div>
      <div><dt>Latest sequence</dt><dd>{task.latest_sequence ?? '—'}</dd></div>
    </dl>

    <div class="linked_status_grid" aria-label="Linked status">
      <article class="linked_status_card">
        <h3>Dispatch</h3>
        {#if task.dispatch}
          <span class="state_pill" data-state={task.dispatch.state ?? 'unknown'}>{task.dispatch.state ?? 'Unknown'}</span>
          <p class="mono">{task.dispatch.id ?? 'ID unavailable'} · seq {task.dispatch.sequence ?? '—'}</p>
        {:else}<p>No linked dispatch</p>{/if}
      </article>
      <article class="linked_status_card">
        <h3>Workflow</h3>
        {#if task.workflow}
          <span class="state_pill" data-state={task.workflow.state ?? 'unknown'}>{task.workflow.state ?? 'Unknown'}</span>
          <p class="mono">{task.workflow.id ?? 'ID unavailable'} · seq {task.workflow.sequence ?? '—'}</p>
        {:else}<p>No linked workflow</p>{/if}
      </article>
    </div>

    <h3 class="subheading">Runs</h3>
    {#if task.runs.length === 0}
      <p class="empty_state compact">No run records are attached to this task.</p>
    {:else}
      <ul class="run_list">
        {#each task.runs as run (run.run_id)}
          <li class="run_card">
            <div class="run_card_heading">
              <span class="mono">{run.run_id}</span>
              <span class="state_pill" data-state={run.state}>{run.state}</span>
            </div>
            <p>{run.harness ?? 'Unknown harness'}{run.role ? ` · ${run.role}` : ''} · seq {run.latest_sequence ?? '—'}</p>
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
</section>

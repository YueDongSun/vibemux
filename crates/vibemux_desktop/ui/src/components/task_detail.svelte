<script lang="ts">
  import { task_status_label } from '../lib/read_state';
  import type { TaskDetail as TaskDetailModel } from '../lib/read_state';
  import TaskTerminal from './task_terminal.svelte';
  import WorkspaceAccess from './workspace_access.svelte';

  let { task_id, detail, loading, error, on_refresh, on_close, on_open_window, opening_window = false, project_key }: {
    task_id: string;
    detail: TaskDetailModel | null;
    loading: boolean;
    error: string;
    on_refresh: () => void;
    on_close?: () => void;
    on_open_window?: (task_id: string) => void;
    opening_window?: boolean;
    project_key?: string;
  } = $props();

  const tabs = ['overview', 'activity', 'artifacts', 'terminal'] as const;
  let active_tab = $state<(typeof tabs)[number]>('overview');
  let copy_notice = $state('');

  async function copy_id(value: string) {
    try { await navigator.clipboard.writeText(value); copy_notice = 'ID copied.'; }
    catch { copy_notice = 'Copy is unavailable in this window.'; }
  }
</script>

<div class="task_detail_body" aria-busy={loading}>
  <header><h2>{detail?.task.title || 'Task details'}</h2>{#if on_close}<button class="close_detail" aria-label="Close task details" onclick={() => on_close?.()}>×</button>{/if}</header>
  {#if detail?.task.title_truncated}<p class="muted_read">Title shortened in the source snapshot</p>{/if}
  <div class="id_row"><code>{task_id}</code><button aria-label="Copy task ID" onclick={() => copy_id(task_id)}>Copy ID</button></div>
  <div class="detail_actions">
    <button disabled={loading} onclick={() => on_refresh()}>{loading ? 'Reading…' : 'Refresh task'}</button>
    {#if on_open_window}<button disabled={opening_window} onclick={() => on_open_window?.(task_id)}>{opening_window ? 'Opening…' : 'Open in window'}</button>{/if}
    <span role="status">{copy_notice}</span>
  </div>
  {#if error}<p class="read_notice" role="alert">{error}</p>{/if}
  {#if detail}
    <nav class="detail_tabs" aria-label="Task detail sections">{#each tabs as tab}<button class:active={active_tab === tab} aria-current={active_tab === tab ? 'page' : undefined} onclick={() => active_tab = tab}>{tab}</button>{/each}</nav>
    {#if active_tab === 'overview'}
      <dl class="task_facts"><dt>Status</dt><dd>{task_status_label(detail.task.status)}</dd><dt>Project ID</dt><dd><code>{detail.task.project_id}</code></dd><dt>Detail revision</dt><dd>{detail.task.detail_revision || 'Not provided'}</dd></dl>
      <h3>Runs</h3>
      {#if !detail.runs.length}<p class="muted_read">No run metadata was returned.</p>{/if}
      {#each detail.runs as run (run.run_id)}<article class="run_card"><div class="id_row"><code>{run.run_id}</code><button aria-label="Copy run ID" onclick={() => copy_id(run.run_id)}>Copy</button></div><dl><dt>Status</dt><dd>{task_status_label(run.status)}</dd><dt>Harness</dt><dd>{run.harness}</dd><dt>Role</dt><dd>{run.role}</dd><dt>Branch</dt><dd><code>{run.branch ?? 'Not recorded'}</code></dd><dt>Worktree</dt><dd>{run.has_worktree ? 'Recorded' : 'Not recorded'}</dd></dl>
        {#if project_key}{#key `${project_key}:${detail.task.task_id}:${run.run_id}`}
          <WorkspaceAccess {project_key} project_id={detail.task.project_id} task_id={detail.task.task_id} run_id={run.run_id} branch={run.branch} has_worktree={run.has_worktree} />
        {/key}{/if}
      </article>{/each}
      {#if detail.has_more_runs}<p class="muted_read">The daemon returned a bounded run summary; more runs exist.</p>{/if}
    {:else if active_tab === 'activity'}
      <h3>Recorded activity</h3>
      {#if !detail.events.length}<p class="muted_read">No recent events were returned.</p>{/if}
      <ol class="activity_list">{#each detail.events as event (event.sequence)}<li><span class="event_sequence">#{event.sequence}</span><div><strong>{event.event_type}</strong><time datetime={event.at}>{event.at}</time>{#if event.run_id}<code>{event.run_id}</code>{/if}</div></li>{/each}</ol>
      {#if detail.has_more_events}<p class="muted_read">This is a bounded activity window, not the complete event history.</p>{/if}
    {:else if active_tab === 'artifacts'}
      <h3>Artifact metadata</h3>
      {#if !detail.artifacts.length}<p class="muted_read">No artifact metadata was returned.</p>{/if}
      {#each detail.artifacts as artifact (artifact.artifact_id)}<article class="run_card"><div class="id_row"><code>{artifact.artifact_id}</code><button aria-label="Copy artifact ID" onclick={() => copy_id(artifact.artifact_id)}>Copy</button></div><p>{artifact.media_type} · {artifact.size_bytes.toLocaleString()} bytes</p><small>Run <code>{artifact.run_id}</code></small></article>{/each}
      {#if detail.has_more_artifacts}<p class="muted_read">More artifact metadata exists than this bounded snapshot includes.</p>{/if}
    {:else if project_key}
      {#key `${project_key}:${detail.task.task_id}`}
        <TaskTerminal {project_key} project_id={detail.task.project_id} task_id={detail.task.task_id} runs={detail.runs} />
      {/key}
    {:else}<p class="muted_read">Choose a registered project to inspect its terminal.</p>
    {/if}
  {:else if loading}<p class="muted_read" role="status">Reading task metadata…</p>
  {:else if !error}<p class="muted_read">Select a task to read its details.</p>{/if}
</div>

<style>
  .task_detail_body>header{display:flex;align-items:flex-start;justify-content:space-between;gap:12px}.task_detail_body h2{font:23px/1.35 var(--font_display);margin:0 0 14px;overflow-wrap:anywhere}.task_detail_body h3{font-size:12px;margin:22px 0 12px}.close_detail{border:0;background:transparent;font-size:22px;padding:0 5px}.id_row{display:flex;gap:10px;align-items:center;justify-content:space-between;min-width:0}.id_row code{font-size:10px;overflow-wrap:anywhere}.id_row button{font-size:10px;white-space:nowrap;padding:4px 7px}.detail_actions{display:flex;flex-wrap:wrap;gap:10px;align-items:center;margin:14px 0;font-size:11px;color:var(--muted_text)}.detail_actions button{font-size:11px}.read_notice{margin:12px 0;padding:12px 14px;border:1px solid var(--border);border-radius:9px;background:var(--surface);font-size:12px}.detail_tabs{display:flex;gap:5px;border-bottom:1px solid var(--border);padding:8px 0}.detail_tabs button{border-color:transparent;background:transparent;font-size:12px;text-transform:capitalize}.detail_tabs .active{background:var(--accent_tint);color:var(--accent_text);border-color:var(--accent_edge)}.task_facts,.run_card dl{display:grid;grid-template-columns:85px minmax(0,1fr);gap:9px;font-size:12px}.task_facts dt,.run_card dt{color:var(--muted_text)}.task_facts dd,.run_card dd{margin:0;overflow-wrap:anywhere}.task_facts code,.run_card code{font-size:10px}.run_card{padding:13px 0;border-bottom:1px solid var(--border)}.run_card p,.run_card small{font-size:11px;color:var(--muted_text)}.activity_list{list-style:none;padding:0;margin:0}.activity_list li{display:flex;gap:12px;padding:12px 0;border-bottom:1px solid var(--border)}.event_sequence{color:var(--muted_text);font:10px var(--font_code);padding-top:3px}.activity_list li>div{display:grid;gap:5px;min-width:0;overflow-wrap:anywhere}.activity_list strong{font-size:12px}.activity_list time,.activity_list code{font-size:10px;color:var(--muted_text)}.muted_read{color:var(--muted_text);font-size:12px}
</style>

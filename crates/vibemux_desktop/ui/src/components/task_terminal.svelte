<script lang="ts">
  import { onMount } from 'svelte';
  import { load_backend } from '../lib/backend';
  import { command_code } from '../lib/native_state';
  import type { RunFact } from '../lib/read_state';
  import { TaskTerminalState, type TerminalStateView } from '../lib/terminal_state';

  let { project_key, project_id, task_id, runs }: { project_key: string; project_id: string; task_id: string; runs: RunFact[] } = $props();
  let model = $state<TaskTerminalState | null>(null);
  let view = $state<TerminalStateView | null>(null);
  let startup_error = $state('');
  let fixture_mode = $state(false);

  $effect(() => { model?.set_runs(runs.map((run) => run.run_id)); });

  onMount(() => {
    // The parent keys this component by its immutable project/task scope.
    const context = { project_key, project_id, task_id };
    let disposed = false;
    void (async () => {
      try {
        const backend = await load_backend();
        if (disposed) return;
        fixture_mode = backend.mode === 'fixture';
        model = new TaskTerminalState(context, runs.map((run) => run.run_id), {
          perform: (key, task, run, request) => backend.invoke('supervisor_action', { project_key: key, task_id: task, run_id: run, request }),
        }, (next) => { if (!disposed) view = next; });
        view = model.snapshot();
      } catch (error: unknown) {
        if (!disposed) startup_error = `Terminal view unavailable (${command_code(error)}).`;
      }
    })();
    return () => { disposed = true; model?.dispose(); };
  });
</script>

<section class="terminal_panel" aria-labelledby="terminal_heading" aria-busy={view?.pending !== null && view !== null}>
  <header><h3 id="terminal_heading">Terminal</h3><p>Terminal contents stay in WezTerm.</p></header>
  {#if fixture_mode}<p class="terminal_notice" role="status">DEMO · Terminal actions are unavailable.</p>{/if}
  {#if startup_error}<p class="terminal_notice" role="alert">{startup_error}</p>
  {:else if !view}<p class="terminal_muted" role="status">Loading terminal view…</p>
  {:else}
    <div class="terminal_toolbar">
      <label>Run
        <select value={view.run_id ?? ''} disabled={Boolean(view.pending) || !runs.length || fixture_mode} onchange={(event) => model?.select_run(event.currentTarget.value)}>
          {#if !runs.length}<option value="">No run available</option>{/if}
          {#each runs as run (run.run_id)}<option value={run.run_id}>{run.harness} · {run.run_id}</option>{/each}
        </select>
      </label>
      <button disabled={!view.can_find || fixture_mode} onclick={() => void model?.find_terminals()}>{view.pending === 'inspect' ? 'Finding…' : 'Find terminals'}</button>
    </div>
    {#if !runs.length}<p class="terminal_muted">No run metadata is available for this task.</p>{/if}
    {#if view.pending}<p class="terminal_muted" role="status">{({ inspect: 'Finding terminals…', link: 'Linking terminal…', focus: 'Focusing terminal…', unlink: 'Unlinking observation…' } as const)[view.pending]}</p>{/if}
    {#if view.error}<p class="terminal_notice" role="alert">{view.error}</p>{/if}
    {#if view.notice}<p class="terminal_notice" role="status">{view.notice}</p>{/if}
    {#if view.terminal}
      <p class="terminal_status">{view.terminal.status}</p>
      {#if view.terminal.checked_at}<p class="terminal_muted">Checked {view.terminal.checked_at}</p>{/if}
      {#if view.terminal.binding_id}
        <div class="terminal_linked"><span>Pane <code>{view.terminal.pane_id}</code></span>
          <button disabled={!view.can_focus || fixture_mode} onclick={() => void model?.focus()}>Focus</button>
          <button disabled={!view.can_unlink || fixture_mode} onclick={() => void model?.unlink()}>Unlink</button>
        </div>
        {#if view.can_unlink && !view.can_focus}<p class="terminal_muted">Unlink targets only the previously reviewed observation.</p>{/if}
      {/if}
      <div class="terminal_candidates">
        {#each view.terminal.candidates as candidate (`${candidate.plugin_id}:${candidate.instance_id}:${candidate.pane_id}`)}
          <article><strong>{candidate.display_name}</strong><button disabled={!view.can_link || fixture_mode} onclick={() => void model?.link_candidate(candidate.pane_id, candidate.instance_id)}>Link terminal</button></article>
        {/each}
      </div>
    {:else if runs.length && !view.error}<p class="terminal_muted">Find terminals to inspect this Run’s configured observer.</p>{/if}
  {/if}
</section>

<style>
  .terminal_panel{min-width:0;color:var(--text_primary);font-size:12px}.terminal_panel header h3{font-size:13px;margin:0 0 7px}.terminal_panel header p,.terminal_muted{color:var(--muted_text);font-size:12px;line-height:1.5}.terminal_panel header p{margin:0 0 16px}.terminal_toolbar{display:flex;gap:10px;align-items:flex-end;flex-wrap:wrap}.terminal_toolbar label{display:grid;gap:6px;min-width:0;flex:1}.terminal_toolbar select{min-width:0;max-width:100%;padding:8px;border:1px solid var(--border);border-radius:8px;background:var(--surface);color:var(--text_primary);font:inherit}.terminal_toolbar button,.terminal_linked button,.terminal_candidates button{font-size:12px;white-space:nowrap}.terminal_notice{padding:10px 12px;border:1px solid var(--border);border-radius:8px;background:var(--surface);line-height:1.5;overflow-wrap:anywhere}.terminal_status{line-height:1.5;overflow-wrap:anywhere}.terminal_linked{display:flex;flex-wrap:wrap;align-items:center;gap:10px;padding:10px 0}.terminal_linked>span{flex:1;overflow-wrap:anywhere}.terminal_linked code{font-size:11px}.terminal_candidates{display:grid;gap:10px;margin-top:12px}.terminal_candidates article{display:flex;align-items:center;gap:12px;flex-wrap:wrap;padding:12px;border:1px solid var(--border);border-radius:9px;background:var(--surface_alt)}.terminal_candidates strong{flex:1;min-width:0;overflow-wrap:anywhere;font-weight:500}.terminal_toolbar select:focus-visible{outline:2px solid var(--accent_edge);outline-offset:2px}
</style>

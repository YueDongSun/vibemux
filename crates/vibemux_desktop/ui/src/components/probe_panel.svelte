<script lang="ts">
  import { untrack } from 'svelte';
  import type { ProbeState, ProbeStep, ProbeView } from '../lib/probe_state';

  let { controller, project_key }: { controller: ProbeState; project_key: string | null } = $props();
  let view = $state<ProbeView>(untrack(() => controller.snapshot()));
  const names: Record<string, string> = { claude: 'Claude', codex: 'Codex', open_code: 'OpenCode', copilot: 'Copilot', grok: 'Grok', qwen: 'Qwen', iflow: 'iFlow', trae: 'TRAE', codebuddy: 'CodeBuddy', kimi: 'Kimi', dsh: 'DSH' };
  const steps: Record<ProbeStep, string> = { verified: 'Verified', failed: 'Failed', unavailable: 'Unavailable', not_run: 'Not checked' };
  $effect(() => {
    const current = controller;
    const unsubscribe = current.subscribe(next => { view = next; });
    void current.bind(project_key);
    current.start_polling();
    return () => { unsubscribe(); current.stop_polling(); void current.bind(null); };
  });
</script>

<section class="probe_panel" aria-label="Local checks" aria-busy={view.loading || view.busy}>
  <header><div><h3>Local checks</h3><p>Local tool versions and connection health.</p></div></header>
  {#if !project_key}<p class="probe_status">Choose a project to check local tools.</p>
  {:else if view.project_key !== project_key}<p class="probe_status" role="status">Checking status…</p>
  {:else}
    <div class="probe_actions">
      <button type="button" disabled={!view.can_start} onclick={() => void controller.start()}>Run local checks</button>
      <button type="button" disabled={!view.can_cancel} onclick={() => void controller.cancel()}>{view.cancel_requested ? 'Cancellation requested' : 'Cancel'}</button>
      <button type="button" disabled={!view.can_refresh} onclick={() => void controller.refresh_status()}>{view.loading ? 'Checking…' : 'Check status'}</button>
    </div>
    {#if view.error}<p class="probe_status" role="alert">{view.error}</p>
    {:else if view.uncertain}<p class="probe_status" role="status">The check is unconfirmed. Check status before starting another.</p>
    {:else if view.cancel_requested && view.job?.phase === 'running'}<p class="probe_status" role="status">Cancellation requested. Waiting for the check to stop.</p>
    {:else if view.notice}<p class="probe_status" role="status">{view.notice}</p>{/if}
    {#if view.result}
      <p class="probe_time">Last checked <time datetime={new Date(view.result.report.observed_at_epoch_seconds * 1000).toISOString()}>{new Date(view.result.report.observed_at_epoch_seconds * 1000).toISOString()}</time></p>
      <div class="probe_table"><table><caption>Last completed local checks</caption><thead><tr><th scope="col">Tool</th><th scope="col">Launcher</th><th scope="col">Version</th><th scope="col">Authentication</th><th scope="col">Model response</th></tr></thead>
        <tbody>{#each view.result.report.agents as agent (agent.agent)}<tr><th scope="row">{names[agent.agent]}</th><td>{steps[agent.launcher_state]}</td><td>{agent.version ?? 'Not available'}</td><td>Not checked</td><td>Not checked</td></tr>{/each}</tbody>
      </table></div>
      <p class="probe_summary">Gateway: {steps[view.result.report.gateway.state]} · A2A: {steps[view.result.report.a2a.state]}</p>
    {:else}<p class="probe_status">No completed local checks yet.</p>{/if}
  {/if}
</section>

<style>
  .probe_panel{min-width:0;border:1px solid var(--border);border-radius:12px;background:var(--surface_alt);padding:18px;margin-bottom:16px}.probe_panel h3{font-size:14px;margin:0}.probe_panel header p,.probe_status,.probe_time,.probe_summary{color:var(--muted_text);font-size:12px;line-height:1.5}.probe_actions{display:flex;flex-wrap:wrap;gap:8px;margin:14px 0}.probe_actions button{font:inherit;white-space:nowrap}.probe_table{max-width:100%;overflow:auto}.probe_table table{border-collapse:collapse;min-width:100%;font-size:12px}.probe_table caption{text-align:left;margin:8px 0;color:var(--muted_text)}.probe_table th,.probe_table td{text-align:left;vertical-align:top;border-bottom:1px solid var(--border);padding:9px 10px;white-space:nowrap}.probe_status{overflow-wrap:anywhere}.probe_time time{font-variant-numeric:tabular-nums}
</style>

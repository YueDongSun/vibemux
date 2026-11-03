<script lang="ts">
  import type { AskStatus, ProjectRow } from '../lib/ui_contract';
  import { harness_label, phase_label } from '../lib/native_state';
  let { project, status, on_close }: { project: ProjectRow | undefined; status: AskStatus | undefined; on_close: () => void } = $props();
</script>

<aside class="native_context" aria-label="Ask context and provenance">
  <header><h2>Context</h2><button class="icon_button" aria-label="Close context" onclick={on_close}>×</button></header>
  <section><h3>Project</h3><p>{project?.name ?? 'No project selected'}</p><p class="muted">Daemon: {project?.daemon ?? 'unknown'}</p></section>
  <section><h3>Execution</h3><p>{status ? harness_label(status.harness) : 'Select a route in the composer'}</p><p class="muted">{status ? phase_label(status) : 'Each Send starts one independent turn.'}</p><p class="muted">The visual presentation does not choose a harness.</p></section>
  <section><h3>Canonical identity</h3>
    {#if status?.canonical}<dl><dt>Request</dt><dd>{status.canonical.request_id}</dd><dt>Task</dt><dd>{status.canonical.task_id}</dd><dt>Run</dt><dd>{status.canonical.run_id}</dd><dt>Protocol</dt><dd>{status.canonical.protocol}</dd><dt>Phase</dt><dd>{status.canonical.phase}</dd></dl>
    {:else}<p class="muted">No confirmed daemon admission yet.</p>{/if}
  </section>
  <section><h3>Output</h3><p class="muted">{status?.output_state ?? 'pending'} · {status?.delivery_complete ? 'delivery complete' : 'delivery incomplete'}</p></section>
  <section><h3>Context limits</h3><p class="muted">Claude Code has no tools. Codex uses its configured read-only profile. Prior Asks are not sent as shared vendor memory.</p></section>
  <section><h3>Retention</h3><p class="muted">Available Asks survive a webview reload while this desktop process stays open. Prompts and replies are not saved in browser storage. App exit loses this display history; daemon restart loses captured output.</p></section>
</aside>

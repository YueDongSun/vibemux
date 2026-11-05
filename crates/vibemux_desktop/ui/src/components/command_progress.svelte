<script lang="ts">
  import type { CommandProgressEntry } from '../lib/command_progress';
  let { entry, active = false, final_context = '' }: { entry: CommandProgressEntry; active?: boolean; final_context?: string } = $props();
  const labels = { in_progress: 'in progress', finalizing: 'finalizing', recorded: 'recorded', unconfirmed: 'unconfirmed' };
  let preview = $derived(Array.from(entry.content.command).slice(0, 120).join(''));
</script>

<details class="command_progress" data-command-record={entry.record_id}>
  <summary><span class="summary_label">Command · {active ? 'reported' : 'last observed · reported'} {labels[entry.phase]}</span><span class="command_preview" dir="auto">{preview || 'Empty command text'}</span></summary>
  <div class="command_body">
    <p class="command_note">Reported activity; execution and permission are not confirmed by this observation.</p>
    {#if final_context}<p class="command_note">{final_context}</p>{/if}
    <p class="field_label">Command</p><pre dir="auto">{entry.content.command}</pre>
    {#if entry.content.command_truncated}<p class="command_note">Command text truncated · {entry.content.command_original_bytes} original bytes.</p>{/if}
    <p class="field_label">Working directory</p><pre dir="auto">{entry.content.working_directory}</pre>
    {#if entry.content.directory_truncated}<p class="command_note">Directory text truncated · {entry.content.directory_original_bytes} original bytes.</p>{/if}
    <p class="field_label">Output tail</p>
    {#if entry.content.output_tail === null}<p class="command_note">Output unavailable.</p>
    {:else if entry.content.output_tail === ''}<p class="command_note">{entry.content.output_omitted_bytes ? 'No retained output.' : 'No output observed.'}</p>
    {:else}<pre dir="auto">{entry.content.output_tail}</pre>{/if}
    {#if entry.content.output_omitted_bytes}<p class="command_note">{entry.content.output_omitted_bytes} output bytes omitted · {entry.content.output_original_bytes} observed bytes.</p>{/if}
  </div>
</details>

<style>
  .command_progress{min-width:0;max-width:100%;border:1px solid var(--border);border-radius:8px;margin:8px 0;font-size:12px;background:var(--surface_alt)}summary{padding:10px 12px;cursor:pointer;overflow-wrap:anywhere}.summary_label{font-weight:600}.command_preview{display:block;white-space:pre-wrap;overflow-wrap:anywhere;font-family:var(--font_code);font-size:11px;color:var(--muted_text);unicode-bidi:plaintext}.command_body{padding:0 12px 12px;min-width:0}.field_label{margin:12px 0 5px;font-weight:600;font-size:11px}.command_progress pre{margin:0;max-width:100%;white-space:pre-wrap;overflow-wrap:anywhere;word-break:break-word;font:11px/1.5 var(--font_code);unicode-bidi:plaintext}.command_note{font-size:11px;line-height:1.5;color:var(--muted_text);margin:6px 0;overflow-wrap:anywhere}
</style>

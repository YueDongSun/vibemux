<script lang="ts">
  import CommandProgress from './command_progress.svelte';
  import type { CommandProgressEntry } from '../lib/command_progress';
  import { read_tool_record } from '../lib/tool_records';
  let { text, complete, truncated, progress = null, progress_active = false, content_error = null }: { text: string; complete: boolean; truncated: boolean; progress?: CommandProgressEntry | null; progress_active?: boolean; content_error?: string | null } = $props();
  let parsed = $derived(read_tool_record(text, complete, truncated));
  let command_preview = $derived(parsed.kind === 'command' ? Array.from(parsed.record.command).slice(0, 120).join('') : '');
</script>

{#if progress && (content_error !== null || parsed.kind === 'text')}
  <CommandProgress entry={progress} active={progress_active} final_context={content_error ? 'Final tool content could not be read. Showing the last command observation.' : !complete ? 'Final tool content is loading. Showing the last command observation.' : 'Final tool content is unavailable for structured display. Showing the last command observation.'} />
{:else if content_error}<p class="tool_note">Record content is unavailable ({content_error}).</p>
{:else}
<details class="tool_record">
  <summary>
    {#if parsed.kind === 'command'}
      <span class="summary_label">Command · reported {parsed.record.status}{#if parsed.record.exit_code !== null} · exit {parsed.record.exit_code}{/if}</span>
      <span class="command_preview" dir="auto">{command_preview || 'Empty command text'}</span>
    {:else if parsed.kind === 'file_change'}
      <span class="summary_label">File changes · reported {parsed.record.status} · {parsed.record.original_change_count} {parsed.record.original_change_count === 1 ? 'change' : 'changes'}</span>
    {:else}<span class="summary_label">Tool event{parsed.incomplete ? ' · partial' : ''}</span>{/if}
  </summary>
  <div class="tool_body">
    {#if parsed.kind === 'command'}
      <p class="field_label">Command</p>
      {#if parsed.record.command === ''}<p class="tool_note">{parsed.record.command_truncated ? 'No retained command text.' : 'Empty command text.'}</p>
      {:else}<pre dir="auto">{parsed.record.command}</pre>{/if}
      {#if parsed.record.command_truncated}<p class="tool_note">Command text truncated · {parsed.record.command_original_bytes} original bytes.</p>{/if}
      <p class="field_label">Working directory</p><pre class="working_directory" dir="auto">{parsed.record.working_directory}</pre>
      <p class="field_label">Output</p>
      {#if parsed.record.output === null}<p class="tool_note">Output unavailable.</p>
      {:else if parsed.record.output === ''}<p class="tool_note">{parsed.record.output_truncated ? 'No retained output.' : 'No output.'}</p>
      {:else}<pre dir="auto">{parsed.record.output}</pre>{/if}
      {#if parsed.record.output_truncated}<p class="tool_note">Output truncated · {parsed.record.output_original_bytes} original bytes.</p>{/if}
      <p class="tool_note">Duration: {parsed.record.duration_ms === null ? 'Not reported' : `${parsed.record.duration_ms} ms`}</p>
    {:else if parsed.kind === 'file_change'}
      {#if parsed.record.changes_truncated}<p class="tool_note">Showing {parsed.record.changes.length} of {parsed.record.original_change_count} reported changes.</p>{/if}
      {#if parsed.record.changes.length === 0}
        <p class="tool_note">{parsed.record.original_change_count === 0 ? 'No file changes reported.' : 'No retained file details.'}</p>
      {:else}
        {#each parsed.record.changes as change, index (index)}
          <section class="file_change" aria-label="Reported file change">
            <p class="field_label">File {index + 1} · reported {change.kind}</p>
            <p class="field_label">Path</p><pre dir="auto">{change.path}</pre>
            {#if change.move_path !== null}<p class="field_label">Move path</p><pre dir="auto">{change.move_path}</pre>{/if}
            <p class="field_label">Diff text</p>
            {#if change.diff === ''}<p class="tool_note">{change.diff_truncated ? 'No retained diff text.' : 'No diff text reported.'}</p>
            {:else}<pre dir="auto">{change.diff}</pre>{/if}
            {#if change.diff_truncated}<p class="tool_note">Diff text truncated · {change.diff_original_bytes} original bytes.</p>{/if}
          </section>
        {/each}
      {/if}
    {:else}
      {#if parsed.incomplete}<p class="tool_note">Partial text.</p>{/if}
      <pre dir="auto">{parsed.text}</pre>
    {/if}
  </div>
</details>
{/if}

<style>
  .tool_record{min-width:0;max-width:100%;border:1px solid var(--border);border-radius:8px;margin:8px 0;font-size:12px;background:var(--surface_alt)}summary{padding:10px 12px;cursor:pointer;overflow-wrap:anywhere}.summary_label{font-weight:600}.command_preview{display:block;white-space:pre-wrap;overflow-wrap:anywhere;font-family:var(--font_code);font-size:11px;color:var(--muted_text);unicode-bidi:plaintext}.tool_body{padding:0 12px 12px;min-width:0}.field_label{margin:12px 0 5px;font-weight:600;font-size:11px}.tool_record pre{margin:0;max-width:100%;white-space:pre-wrap;overflow-wrap:anywhere;word-break:break-word;font:11px/1.5 var(--font_code);unicode-bidi:plaintext}.file_change{border-top:1px solid var(--border);margin-top:10px;padding-top:2px}.tool_note{font-size:11px;line-height:1.5;color:var(--muted_text);margin:6px 0;overflow-wrap:anywhere}
</style>

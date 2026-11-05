<script lang="ts">
  import type { ExportFormat } from '../lib/chat_export_contract';
  import type { ExportView } from '../lib/chat_export_state';

  let { view, on_format, on_export }: {
    view: ExportView; on_format: (format: ExportFormat) => void; on_export: () => void;
  } = $props();
</script>

<div class="chat_export" aria-label="Conversation export">
  <div class="export_actions">
    <label>Export format
      <select value={view.format} disabled={view.busy || !view.can_export}
        onchange={(event) => on_format(event.currentTarget.value as ExportFormat)}>
        <option value="text">Text</option>
        <option value="json">JSON</option>
      </select>
    </label>
    <button type="button" disabled={view.busy || !view.can_export} onclick={on_export}>Export</button>
  </div>
  <p class="export_hint">Private conversation content will be saved to your chosen file.</p>
  {#if view.busy}
    <p role="status">Export in progress…</p>
  {:else if view.error}
    <p class="export_error" role="alert">{view.error}</p>
  {:else if view.status === 'saved' && view.saved}
    <p role="status">Saved {view.saved.format === 'text' ? 'Text' : 'JSON'} export · {view.saved.record_count.toLocaleString()} records · {view.saved.byte_count.toLocaleString()} bytes.</p>
    {#if view.saved.partial}<p class="export_hint">Some content was unavailable. The file contains a partial history.</p>{/if}
  {:else if view.status === 'cancelled'}
    <p role="status">Export cancelled.</p>
  {/if}
</div>

<style>
  .chat_export { display: grid; gap: 5px; min-width: 0; }
  .export_actions { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
  .export_actions label { display: flex; align-items: center; gap: 7px; font-size: 12px; }
  .export_actions select, .export_actions button { padding: 5px 9px; font-size: 12px; }
  .chat_export p { margin: 0; font-size: 11px; }
  .export_hint { color: var(--muted_text); }
  .export_error { color: var(--danger, var(--muted_text)); }
</style>

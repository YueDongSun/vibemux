<script lang="ts">
  /** Purely presentational text-attachment list for the chat composer. File
   * content is rendered as literal text only — never interpreted HTML, script,
   * or tool instructions. Picking, state, and removal logic live in the
   * controller that owns the draft. */
  import { ATTACHMENT_LIMITS, type TextAttachment } from '../lib/text_attachments';

  let { files, busy = false, on_pick, on_remove }: {
    files: readonly TextAttachment[];
    busy?: boolean;
    on_pick: () => void;
    on_remove: (attachment_id: string) => void;
  } = $props();

  const total_bytes = $derived(files.reduce((total, file) => total + file.byte_count, 0));
</script>

<div class="text_attachments" aria-label="Text file attachments">
  <div class="attachment_toolbar">
    <button type="button" class="attachment_pick" disabled={busy || files.length >= ATTACHMENT_LIMITS.files} title="Attach UTF-8 text or code files" onclick={on_pick}>＋ Attach text</button>
    <span class="attachment_caption">{files.length} / {ATTACHMENT_LIMITS.files} files · {total_bytes.toLocaleString()} / {ATTACHMENT_LIMITS.total_bytes.toLocaleString()} bytes</span>
  </div>
  {#if files.length}
    <ul class="attachment_list">
      {#each files as file (file.attachment_id)}
        <li class="attachment_row">
          <div class="attachment_meta">
            <span class="attachment_name" title={file.name}>{file.name}</span>
            <span class="attachment_bytes">{file.byte_count.toLocaleString()} bytes</span>
            <button type="button" class="attachment_remove" aria-label={'Remove ' + file.name} disabled={busy} onclick={() => on_remove(file.attachment_id)}>×</button>
          </div>
          <details class="attachment_preview">
            <summary>Preview (literal text)</summary>
            <pre>{file.content}</pre>
          </details>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="attachment_empty">Up to {ATTACHMENT_LIMITS.files} UTF-8 text or code files · {(ATTACHMENT_LIMITS.file_bytes / 1024).toLocaleString()} KiB each · text only, no binary formats.</p>
  {/if}
</div>

<style>
  .text_attachments { display: grid; gap: 8px; margin-bottom: 10px; }
  .attachment_toolbar { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; }
  .attachment_pick { font-size: 12px; padding: 5px 9px; }
  .attachment_caption, .attachment_empty { font-size: 11px; color: var(--muted_text); margin: 0; }
  .attachment_list { list-style: none; margin: 0; padding: 0; display: grid; gap: 6px; }
  .attachment_row { border: 1px solid var(--border); border-radius: 9px; background: var(--surface); padding: 8px 10px; }
  .attachment_meta { display: flex; align-items: center; gap: 9px; min-width: 0; }
  .attachment_name { font-size: 12px; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .attachment_bytes { font-size: 11px; color: var(--muted_text); flex-shrink: 0; }
  .attachment_remove { background: transparent; border-color: transparent; min-width: 24px; padding: 2px 6px; font-size: 14px; line-height: 1; }
  .attachment_preview summary { font-size: 11px; color: var(--muted_text); cursor: pointer; margin-top: 5px; }
  .attachment_preview pre { max-height: 180px; overflow: auto; margin: 6px 0 0; padding: 9px; border: 1px solid var(--border); border-radius: 7px; background: var(--surface_alt); font-size: 11px; line-height: 1.55; white-space: pre-wrap; overflow-wrap: anywhere; }
</style>

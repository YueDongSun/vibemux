<script lang="ts">
  import BrandMark from '../../../../vibemux_chat/web/src/components/brand_mark.svelte';
  import MarkdownView from './native_markdown.svelte';
  import { harness_label, phase_label } from '../lib/native_state';
  import type { AskReply, AskStatus } from '../lib/ui_contract';
  import TranscriptFind from './transcript_find.svelte';
  export interface CodeArtifact { title: string; language: string; content: string }
  let { status, reply, ask_id, notify, on_cancel, on_copy_draft, on_artifact, find_open = false, on_find_close = () => {} }: {
    status: AskStatus | undefined; reply: AskReply | undefined; ask_id: string;
    notify: (message: string) => void; on_cancel: () => void; on_copy_draft: (text: string) => void;
    on_artifact: (artifact: CodeArtifact) => void;
    find_open?: boolean; on_find_close?: () => void;
  } = $props();
  let transcript_root = $state<HTMLElement>();
  async function copy(text: string) {
    try { await navigator.clipboard.writeText(text); notify('Copied.'); }
    catch { notify('Clipboard unavailable. Select the text to copy it.'); }
  }
</script>

<section class="native_transcript" aria-label="Independent Ask transcript" bind:this={transcript_root}>
  {#if find_open}<TranscriptFind root={transcript_root} revision={reply?.revision ?? 0} on_close={on_find_close} />{/if}
  <div class="ask_heading"><span>Independent Ask</span><code title={ask_id}>{ask_id.slice(0, 12)}</code></div>
  <article class="user_message"><div class="message_role">You</div><div class="user_prompt" data-transcript-body="prompt">{reply?.prompt ?? 'Loading the accepted prompt…'}</div>
    <div class="message_actions"><button onclick={() => void copy(reply?.prompt ?? '')} disabled={!reply}>Copy</button><button onclick={() => on_copy_draft(reply?.prompt ?? '')} disabled={!reply}>Use as a new draft</button></div>
  </article>
  <article class="assistant_message">
    <div class="message_role"><BrandMark /><strong>{status ? harness_label(status.harness) : 'Harness'}</strong><span class="status_badge">{status ? phase_label(status) : 'Locally accepted · awaiting admission'}</span></div>
    {#if reply?.reply}<div data-transcript-body="reply"><MarkdownView content={reply.reply} {notify} {on_artifact} /></div>
    {:else}<p class="muted">{status?.output_state === 'unavailable' ? 'Reply output is unavailable.' : status?.terminal_observed ? 'No assistant text is available for this attempt.' : 'Waiting for assistant text…'}</p>{/if}
    {#if status && !status.terminal_observed && status.phase !== 'rejected'}
      <button class="cancel_button" disabled={status.cancel_requested} onclick={on_cancel}>{status.cancel_requested ? 'Cancelling…' : 'Cancel this Ask'}</button>
    {/if}
    {#if status?.phase === 'uncertain' || status?.observer_state === 'failed'}<div class="notice warning">{status.terminal_observed ? 'Display observation is incomplete. The confirmed daemon outcome is retained.' : 'Observation is incomplete. The last daemon state is retained; execution may still be active.'} <code>{status.observer_error}</code></div>{/if}
    {#if status?.canonical?.error_code}<p class="notice warning">Daemon result: <code>{status.canonical.error_code}</code></p>{/if}
    {#if reply?.truncated}<p class="notice warning">The displayed reply was truncated to the native display limit.</p>{/if}
    {#if status?.terminal_observed && !status.delivery_complete}<p class="notice warning">The daemon outcome is confirmed, but display output is incomplete or unavailable.</p>{/if}
    <div class="message_actions"><button disabled={!reply?.reply} onclick={() => void copy(reply?.reply ?? '')}>Copy reply</button></div>
  </article>
</section>

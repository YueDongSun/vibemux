<script lang="ts">
  import { untrack } from 'svelte';
  import BrandMark from '../../../../vibemux_chat/web/src/components/brand_mark.svelte';
  import MarkdownView from './native_markdown.svelte';
  import TranscriptFind from './transcript_find.svelte';
  import TextAttachments from './text_attachments.svelte';
  import ToolRecord from './tool_record.svelte';
  import CommandProgress from './command_progress.svelte';
  import { command_progress_entry } from '../lib/command_progress';
  import { format_attachment_prompt, parse_attachment_prompt } from '../lib/text_attachments';
  import type { CodeArtifact } from './native_transcript.svelte';
  import { harness_label } from '../lib/native_state';
  import { CHAT_LIMITS, type ChatRecord, type ChatTurn } from '../lib/chat_contract';
  import { ChatState, chat_can_manage, chat_phase_label, chat_prompt_error, chat_title, type ChatView } from '../lib/chat_state';
  import { should_send } from '../lib/native_config';
  import type { Harness } from '../lib/ui_contract';

  let { view, controller, disabled = false, harness = 'claude', harness_available = true, route_note = '', route_reason = '', notify, on_artifact, on_consent, on_purge, on_new_chat, find_open = false, on_find_close = () => {} }: {
    view: ChatView; controller: ChatState; disabled?: boolean; harness?: Harness; harness_available?: boolean; route_note?: string; route_reason?: string;
    notify: (message: string) => void; on_artifact: (artifact: CodeArtifact) => void;
    on_consent: () => void; on_purge: () => void; on_new_chat: () => void;
    find_open?: boolean; on_find_close?: () => void;
  } = $props();
  let transcript_root = $state<HTMLElement>();
  let renaming = $state(false);
  let rename_value = $state('');
  let composer_open = $state(false);
  let composer_scope = '';
  const session = $derived(view.session);
  const composed_prompt = $derived(format_attachment_prompt(view.draft, view.attachments));
  const bytes = $derived(new TextEncoder().encode(composed_prompt).byteLength);
  const prompt_fault = $derived(chat_prompt_error(composed_prompt));
  const sending = $derived(view.pending?.state === 'sending');
  const uncertain = $derived(view.pending?.state === 'uncertain');
  const busy_turn = $derived(Boolean(session?.active_request_id));
  const send_disabled = $derived(disabled || sending || uncertain || view.picking_files || busy_turn || !view.confirmed || !harness_available || Boolean(prompt_fault));
  const records_by_turn = $derived.by(() => {
    const groups = new Map<string, typeof view.records>();
    const loose: typeof view.records = [];
    const turn_ids = new Set(view.turns.map((turn) => turn.turn_id));
    for (const row of view.records) {
      if (row.record.turn_id && turn_ids.has(row.record.turn_id)) {
        const group = groups.get(row.record.turn_id) ?? [];
        group.push(row); groups.set(row.record.turn_id, group);
      } else loose.push(row);
    }
    return { groups, loose };
  });
  const live_turn = $derived(view.live ? view.turns.find((turn) => turn.request_id === view.live?.request_id) ?? null : null);
  const profile = $derived(route_note || (harness === 'claude' ? 'Claude Code: tool-free, one turn at a time.' : 'Codex: configured read-only; approvals are denied.'));
  const retained_draft = $derived(Boolean(session && (session.state === 'closed' || session.state === 'archived' || session.content_purged)));
  $effect(() => {
    const scope = JSON.stringify([view.project_key, view.selected_chat, retained_draft]);
    if (scope !== composer_scope) {
      composer_scope = scope;
      composer_open = untrack(() => Boolean(view.draft || view.attachments.length));
    }
  });

  function turn_label(turn: ChatTurn): string {
    return ({ admitted: 'Admitted', running: 'Running', cancel_requested: 'Cancel requested', completed: 'Completed', failed: 'Failed', cancelled: 'Cancelled', interrupted: 'Interrupted', recovery_required: 'Recovery required' } as Record<string, string>)[turn.phase] ?? turn.phase;
  }
  function kind_label(record: ChatRecord): string {
    return record.kind === 'user' ? 'You' : record.kind === 'assistant' ? harness_label(session?.harness ?? harness) : record.kind === 'tool' ? 'Tool event' : record.kind === 'approval' ? 'Approval' : 'Notice';
  }
  function record_text(record: ChatRecord): string {
    return record.kind === 'user' || record.kind === 'tool' || record.kind === 'approval' || record.kind === 'notice' ? 'plain' : 'markdown';
  }
  async function copy(text: string) {
    try { await navigator.clipboard.writeText(text); notify('Copied.'); }
    catch { notify('Clipboard unavailable. Select the text to copy it.'); }
  }
  function transcript_text(): string {
    return view.records.map((row) => kind_label(row.record) + ':\n' + row.text).join('\n\n') + (view.live ? '\n\n' + harness_label(session?.harness ?? harness) + ' (streaming):\n' + view.live.text : '');
  }
  function start_rename() { renaming = true; rename_value = session ? chat_title(session, view.titles) : ''; }
  async function save_rename() {
    if (!session) return;
    if (await controller.manage('rename', rename_value)) { renaming = false; notify('Chat renamed.'); }
  }
</script>

{#snippet user_message(text: string)}
  {@const parsed = parse_attachment_prompt(text)}
  {#if parsed.kind === 'envelope'}
    {#if parsed.message}<div class="user_prompt">{parsed.message}</div>{/if}
    {#each parsed.files as file, index (index)}
      <details class="saved_attachment"><summary>{file.name}</summary><pre class="user_prompt">{file.content}</pre></details>
    {/each}
  {:else}<div class="user_prompt">{parsed.text}</div>{/if}
{/snippet}

<section class="persistent_chat" aria-label="Persistent chat">
  <div class="button_row" aria-label="Saved chat discovery">
    <button disabled={disabled || view.busy || view.loading} aria-pressed={view.archived} onclick={() => void controller.set_archived(!view.archived)}>{view.archived ? 'Hide archived chats' : 'Show archived chats'}</button>
    {#if view.more_sessions}<button disabled={disabled || view.busy || view.loading} onclick={() => void controller.load_more_sessions()}>Load more chats</button>{/if}
  </div>
  {#if find_open && session}<TranscriptFind root={transcript_root} revision={view.revision} section_labels={{ user: 'You', assistant: 'Reply', tool: 'Tool event', approval: 'Approval', notice: 'Notice', prompt: 'You' }} on_close={on_find_close} />{/if}

  {#if view.pending && uncertain}
    <div class="notice warning" role="status">
      <strong>Chat action not yet confirmed</strong>
      <p>The exact payload and action IDs are retained and nothing is re-sent automatically. Reconcile read-only, or explicitly retry the same action with the same IDs.{#if view.pending.prompt}<br />Your draft is preserved.{/if}</p>
      {#if view.pending.code}<p><code>{view.pending.code}</code></p>{/if}
      <div class="button_row">
        <button disabled={view.busy} onclick={() => void controller.check_pending()}>Reconcile (read-only)</button>
        <button disabled={view.busy} onclick={() => void controller.retry_pending()}>Retry same action</button>
      </div>
    </div>
  {/if}

  {#if view.error}<div class="notice warning" role="alert">{view.error} <button onclick={() => void controller.refresh()}>Refresh</button></div>{/if}
  {#if view.notice && !view.error}<div class="notice" role="status">{view.notice}</div>{/if}

  {#if session}
    <header class="chat_header">
      <div class="chat_heading">
        {#if renaming}
          <form class="chat_rename" onsubmit={(event) => { event.preventDefault(); void save_rename(); }}>
            <label><span class="sr_only">Chat title</span><input maxlength="256" bind:value={rename_value} /></label>
            <button class="primary" type="submit" disabled={view.busy || disabled}>Save</button>
            <button type="button" disabled={view.busy} onclick={() => (renaming = false)}>Cancel</button>
          </form>
        {:else}
          <h2 title={session.chat_id}>{chat_title(session, view.titles)}</h2>
        {/if}
        <span class="status_badge">Preview</span><span class="status_badge">{chat_phase_label(session.state)}</span>
        <span class="status_badge">{harness_label(session.harness)}</span>
        <span class="status_badge">generation {session.generation}</span>
      </div>
      <div class="button_row chat_actions">
        {#if !harness_available}<span class="small chat_route_reason">{route_reason || 'The selected harness route is unavailable.'}</span>{/if}
        {#if !renaming}<button disabled={view.busy || disabled || !chat_can_manage(session, 'rename')} onclick={start_rename}>Rename</button>{/if}
        <button disabled={view.busy || disabled || !chat_can_manage(session, 'close')} title="Stop this chat; it stays saved and can resume." onclick={() => void controller.manage('close')}>Close</button>
        <button disabled={view.busy || disabled || !chat_can_manage(session, 'archive')} title="Move this chat out of the active list." onclick={() => void controller.manage('archive')}>Archive</button>
        <button disabled={view.busy || disabled || !chat_can_manage(session, 'resume')} title="Resume a closed or suspended chat." onclick={() => void controller.manage('resume')}>Resume</button>
        <button class="cancel_button" disabled={view.busy || disabled || !chat_can_manage(session, 'purge')} title="Remove this chat's stored private text; canonical record metadata remains." onclick={on_purge}>Purge…</button>
        <button disabled={view.busy || disabled || busy_turn} onclick={on_new_chat}>New chat</button>
        <button disabled={!view.records.length && !view.live} onclick={() => void copy(transcript_text())}>Copy chat</button>
      </div>
    </header>
  {:else}
    <div class="chat_header"><div class="chat_heading"><h2>New chat</h2><span class="status_badge">Preview</span></div>
      <div class="button_row chat_actions">
        {#if !harness_available}<span class="small chat_route_reason">{route_reason || 'The selected harness route is unavailable.'}</span>{/if}
        <label class="route_picker"><span class="sr_only">Harness route</span>
        <select value={harness} disabled={view.busy || disabled} onchange={(event) => controller.set_harness(event.currentTarget.value as Harness)}><option value="claude">Claude Code</option><option value="codex">Codex</option></select>
      </label></div>
    </div>
  {/if}

  <div class="transcript_scroll" data-transcript-scroll bind:this={transcript_root}>
    {#if !view.confirmed}
      <div class="availability_card"><strong>Persistent chat consent required</strong>
        <p>Saved chats keep messages and reported tool activity in this project's private storage. Sending starts the selected installed harness and may use your configured provider account. Claude Code runs tool-free; Codex runs configured read-only with approvals denied.</p>
        <p class="small">Persistent chat is a preview capability. No live-run certification is claimed.</p>
        <button class="primary" disabled={disabled} onclick={on_consent}>Review and accept</button>
      </div>
    {/if}
    {#if !session}
      <section class="welcome"><BrandMark /><p class="eyebrow">PERSISTENT CHAT · PREVIEW</p><h1>Start a saved conversation.</h1>
        <p>Chats are kept by this project's daemon and survive app restarts.<br />Your draft stays local until you press Send.</p></section>
    {:else if session.content_purged}
      <div class="notice warning">Stored private text was purged and can no longer be shown. Canonical record metadata remains daemon-owned.</div>
    {:else}
      {#if !view.available && !view.loading}<div class="notice warning">Chat records are unavailable right now; the daemon keeps the canonical copy.</div>{/if}
      {#if view.loading}<p class="muted chat_state_line">Loading chat records…</p>{/if}
      {#each records_by_turn.loose as row (row.record.record_id)}
        <article class="chat_record chat_record_{row.record.kind}" data-transcript-body={row.record.kind} data-search-record={row.record.record_id}>
          <div class="message_role">{kind_label(row.record)}</div>
          {#if row.record.kind === 'tool'}<ToolRecord text={row.text} complete={row.complete} truncated={row.record.truncated} progress={command_progress_entry(view.command_progress, row.record.record_id, row.record.turn_id)} progress_active={view.command_progress?.active ?? false} content_error={row.error} />
            {:else if row.error}<p class="notice warning">Record content is unavailable ({row.error}).</p>{:else if row.text}{#if row.record.kind === 'user'}{@render user_message(row.text)}{:else}<div class="user_prompt">{row.text}</div>{/if}{/if}
          {#if row.record.truncated}<p class="small">Shown text was truncated to the record limit.</p>{/if}
        </article>
      {/each}
      {#each view.turns as turn (turn.request_id)}
        <div class="turn_marker"><span>{turn_label(turn)}</span>{#if turn.outcome && !turn.output_complete}<span class="status_badge">outcome {turn.outcome} · display output incomplete</span>{/if}{#if turn.error_code}<code>{turn.error_code}</code>{/if}</div>
        {#each records_by_turn.groups.get(turn.turn_id) ?? [] as row (row.record.record_id)}
          {@const is_plain = record_text(row.record) === 'plain'}
          <article class="chat_record chat_record_{row.record.kind}" data-transcript-body={row.record.kind} data-search-record={row.record.record_id}>
            <div class="message_role">{#if row.record.kind === 'assistant'}<BrandMark />{/if}{kind_label(row.record)}</div>
            {#if row.record.kind === 'tool'}<ToolRecord text={row.text} complete={row.complete} truncated={row.record.truncated} progress={command_progress_entry(view.command_progress, row.record.record_id, row.record.turn_id)} progress_active={view.command_progress?.active ?? false} content_error={row.error} />
            {:else if row.error}<p class="notice warning">Record content is unavailable ({row.error}).</p>
            {:else if row.text}{#if row.record.kind === 'user'}{@render user_message(row.text)}{:else if is_plain}<div class="user_prompt">{row.text}</div>{:else}<div><MarkdownView content={row.text} {notify} {on_artifact} /></div>{/if}{/if}
            {#if row.record.truncated}<p class="small">Shown text was truncated to the record limit ({row.record.retained_byte_count ?? row.record.original_byte_count} of {row.record.original_byte_count} bytes kept).</p>{/if}
            <div class="message_actions">{#if row.text && !row.error}<button onclick={() => void copy(row.text)}>Copy</button>{/if}</div>
          </article>
        {/each}
        {#if view.live && live_turn?.turn_id === turn.turn_id}
          <article class="chat_record chat_record_assistant chat_record_live" data-transcript-body="assistant">
            <div class="message_role"><BrandMark />{harness_label(session.harness)}<span class="status_badge">{view.live.complete ? 'Finalizing…' : 'Streaming…'}</span></div>
            {#if view.live.text}<div class="user_prompt">{view.live.text}</div>{:else}<p class="muted">Waiting for assistant text…</p>{/if}
            {#if view.live.truncated}<p class="small">Live text is bounded; the final record keeps the retained bytes.</p>{/if}
          </article>
        {/if}
      {/each}
      {#if view.command_progress}
        <section class="command_activity" aria-label="Command observations">
          {#each view.command_progress.snapshot.entries.filter((entry) => !view.records.some((row) => row.record.kind === 'tool' && row.record.record_id.toLowerCase() === entry.record_id && row.record.turn_id.toLowerCase() === view.command_progress?.snapshot.canonical_id)) as entry (entry.record_id)}
            <CommandProgress {entry} active={view.command_progress.active} />
          {/each}
          {#if view.command_progress.snapshot.capture_limited}<p class="small">Command capture is limited.{#if view.command_progress.snapshot.omitted_entries} {view.command_progress.snapshot.omitted_entries} earlier observations omitted.{/if}</p>{/if}
        </section>
      {/if}
      {#if view.progress_error}<p class="notice warning" role="status">{view.progress_error}</p>{/if}
      {#if !view.records.length && !view.turns.length && !view.loading}<p class="muted chat_state_line">No records loaded yet.{session.record_count ? ' Load or refresh to read the saved history.' : ''}</p>{/if}
      {#if view.content_more || view.history_capped}<div class="button_row chat_history_more"><button disabled={view.busy || view.loading} onclick={() => void controller.load_more_history()}>Load more messages{view.history_capped ? ' (bounded view)' : ''}</button></div>{/if}
      {#if view.turns_more}<div class="button_row chat_history_more"><button disabled={view.busy || view.loading} onclick={() => void controller.load_more_turns()}>Load more turns</button></div>{/if}
    {/if}
  </div>

  {#snippet composer()}
  <div class="native_composer_wrap">
    <form class="native_composer" onsubmit={(event) => { event.preventDefault(); if (!send_disabled) void controller.send(); }}>
      <TextAttachments files={view.attachments} busy={disabled || view.picking_files || sending} on_pick={() => void controller.pick_files()} on_remove={(id) => controller.remove_file(id)} />
      <textarea
        id="message_composer"
        aria-label="Text prompt for this persistent chat"
        aria-describedby="chat_composer_profile chat_composer_limit"
        placeholder={session ? 'Continue this chat…' : 'Start a new saved chat…'}
        rows="3"
        value={view.draft}
        disabled={disabled}
        oninput={(event) => controller.set_draft(event.currentTarget.value)}
        onkeydown={(event) => { if (should_send(event)) { event.preventDefault(); if (!send_disabled) void controller.send(); } }}
      ></textarea>
      <div class="composer_toolbar">
        <div class="button_row">{#if busy_turn}<button type="button" class="cancel_button" disabled={view.busy || disabled} onclick={() => void controller.cancel()}>Cancel this turn</button>{:else if session}<button type="button" disabled={view.busy || disabled} onclick={on_new_chat}>New chat</button>{/if}</div>
        <div class="button_row">
          <button class="primary send_button" type="submit" disabled={send_disabled}>{sending ? 'Sending…' : 'Send ↑'}</button>
        </div>
      </div>
    </form>
    <div class="composer_caption"><span id="chat_composer_profile">{profile}</span><span id="chat_composer_limit">{bytes.toLocaleString()} / {CHAT_LIMITS.prompt_bytes.toLocaleString()} bytes</span></div>
    {#if bytes > CHAT_LIMITS.prompt_bytes}<p class="notice warning">The message and attached text exceed 32 KiB. Remove a file or shorten the message.</p>{/if}
    <p class="composer_note">Saved chat · preview · draft stays local until Send. Enter to send · Shift + Enter for a new line.</p>
  </div>
  {/snippet}
  {#if retained_draft}
    <details class="retained_composer" bind:open={composer_open}><summary>Draft message{#if view.draft || view.attachments.length} · retained locally{/if}</summary>{@render composer()}</details>
  {:else}
    {@render composer()}
  {/if}
</section>

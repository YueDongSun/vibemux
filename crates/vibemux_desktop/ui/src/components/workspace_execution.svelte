<script lang="ts">
  import { untrack, type Snippet } from 'svelte';
  import MarkdownView from './native_markdown.svelte';
  import TextAttachments from './text_attachments.svelte';
  import ToolRecord from './tool_record.svelte';
  import CommandProgress from './command_progress.svelte';
  import { command_progress_entry } from '../lib/command_progress';
  import TranscriptFind from './transcript_find.svelte';
  import WorkspaceApproval from './workspace_approval.svelte';
  import type { WorkspaceApprovalState, WorkspaceApprovalView } from '../lib/workspace_approval_state';
  import type { CodeArtifact } from './native_transcript.svelte';
  import { modal_focus } from '../lib/keyboard_shortcuts';
  import { harness_label } from '../lib/native_state';
  import { CHAT_LIMITS, type ChatRecordKind } from '../lib/chat_contract';
  import { format_attachment_prompt, parse_attachment_prompt } from '../lib/text_attachments';
  import { WorkspaceExecutionState, workspace_attempt_label, workspace_prompt_fault, type WorkspaceExecutionView } from '../lib/workspace_execution_state';
  import { workspace_phase_label, workspace_run_label } from '../lib/workspace_chat_state';
  import type { Harness } from '../lib/ui_contract';

  let { view, controller, approval_view = null, approval_controller = null, presentation = 'claude', notify, on_artifact, find_open = false, on_find_close = () => {}, workspace_context }: {
    view: WorkspaceExecutionView; controller: WorkspaceExecutionState; presentation?: Harness;
    approval_view?: WorkspaceApprovalView | null; approval_controller?: WorkspaceApprovalState | null;
    notify: (message: string) => void; on_artifact: (artifact: CodeArtifact) => void;
    find_open?: boolean; on_find_close?: () => void; workspace_context?: Snippet;
  } = $props();

  let start_open = $state(false);
  let start_target = $state<{ project_key: string | null; chat_id: string } | null>(null);
  let execution_consent = $state(false);
  let transcript_root = $state<HTMLElement>();
  let composer_open = $state(false);
  let composer_scope = '';

  const owner = $derived(view.owner);
  const session = $derived(view.session);
  const harness = $derived(owner?.harness ?? presentation);
  const chat_label = $derived(owner ? 'Workspace ' + owner.chat_id.slice(0, 12) : 'Workspace');
  const uncertain = $derived(view.pending?.state === 'uncertain');
  const busy_turn = $derived(session?.active_request_id !== null && session?.active_request_id !== undefined);
  const prompt_fault = $derived(workspace_prompt_fault(view.draft, view.files));
  const send_disabled = $derived(view.busy || uncertain || !view.can_send || Boolean(prompt_fault));
  const prompt_bytes = $derived(new TextEncoder().encode(view.files.length ? format_attachment_prompt(view.draft, view.files) : view.draft).length);
  const resume = $derived(session?.state === 'closed' || session?.state === 'suspended');
  const grouped = $derived.by(() => {
    const groups = new Map<string, typeof view.records>();
    const loose: typeof view.records = [];
    const ids = new Set(view.attempts.map((row) => row.attempt_id));
    for (const row of view.records) {
      if (ids.has(row.record.attempt_id)) {
        const group = groups.get(row.record.attempt_id) ?? [];
        group.push(row); groups.set(row.record.attempt_id, group);
      } else loose.push(row);
    }
    return { groups, loose };
  });
  const live_attempt = $derived(view.live ? view.attempts.find((row) => row.request_id === view.live?.request_id) ?? null : null);
  const retained_draft = $derived(Boolean(!view.process_claimed && session));
  $effect(() => {
    const scope = JSON.stringify([view.project_key, view.chat_id, retained_draft]);
    if (scope !== composer_scope) {
      composer_scope = scope;
      composer_open = untrack(() => Boolean(view.draft || view.files.length));
    }
  });

  // The consent dialog is pinned to the project and chat it was opened for; a
  // selection change invalidates the consent instead of carrying it over.
  $effect(() => {
    if (start_target !== null && (view.project_key !== start_target.project_key || view.chat_id !== start_target.chat_id)) close_start();
  });

  function open_start() {
    if (view.chat_id === null) return;
    execution_consent = false;
    start_target = { project_key: view.project_key, chat_id: view.chat_id };
    start_open = true;
  }
  function close_start() { start_open = false; start_target = null; }
  function submit_start(event: SubmitEvent) {
    event.preventDefault();
    if (!execution_consent) return;
    if (start_target === null || view.project_key !== start_target.project_key || view.chat_id !== start_target.chat_id) { close_start(); return; }
    close_start();
    void controller.start(true);
  }
  function composer_keydown(event: KeyboardEvent) {
    if (start_open) return;
    if ((event.ctrlKey || event.metaKey) && event.key === 'Enter' && !event.isComposing && event.keyCode !== 229 && !event.shiftKey && !event.altKey) {
      event.preventDefault();
      if (!send_disabled) void controller.send();
    }
  }
  function kind_label(kind: ChatRecordKind): string {
    return kind === 'user' ? 'You' : kind === 'assistant' ? harness_label(harness) : kind === 'tool' ? 'Tool event' : kind === 'approval' ? 'Approval' : 'Notice';
  }
</script>

<section class="workspace_execution" aria-label="Workspace execution">
  {#if view.project_key === null || view.chat_id === null}
    <div class="welcome"><h1>Workspace conversations</h1><p>Select a workspace to start an owner-backed conversation.</p></div>
  {:else}
    {#if uncertain && view.pending}
      <div class="notice warning" role="status">
        <strong>Workspace action not yet confirmed</strong>
        <p>The exact payload and action IDs are retained and nothing is re-sent automatically. Refresh re-checks read-only; Retry replays the same action and IDs.{#if view.pending.prompt}<br />Your draft is preserved.{/if}</p>
        {#if view.pending.code}<p><code>{view.pending.code}</code></p>{/if}
        <div class="button_row">
          <button disabled={view.busy} onclick={() => void controller.refresh()}>Refresh</button>
          <button disabled={view.busy} onclick={() => void controller.retry_pending()}>Retry same action</button>
        </div>
      </div>
    {/if}
    {#if approval_view && approval_controller}<WorkspaceApproval view={approval_view} controller={approval_controller} />{/if}
    {#if view.error}<div class="notice warning" role="alert">{view.error} <button disabled={view.loading} onclick={() => void controller.refresh()}>Refresh</button></div>{/if}
    {#if view.notice && !view.error}<div class="notice" role="status">{view.notice}</div>{/if}

    <header class="chat_header">
      <div class="chat_heading">
        <h2 title={view.chat_id}>{chat_label}</h2>
        {#if owner}<span class="status_badge">{workspace_phase_label(owner.phase)}</span><span class="status_badge">{workspace_run_label(owner.run_status)}</span>{/if}
        <span class="status_badge">{harness_label(harness)}</span>
        <span class="status_badge">Read-only</span>
        {#if session}<span class="status_badge">generation {session.generation}</span>{/if}
      </div>
      <div class="button_row chat_actions">
        <button disabled={view.loading} onclick={() => void controller.refresh()}>Refresh</button>
        {#if view.can_close}<button class="cancel_button" disabled={view.busy} title="End this session's process; the workspace itself stays open." onclick={() => void controller.close()}>Close session</button>{/if}
        {#if view.can_start}<button class="primary" disabled={view.busy} onclick={open_start}>{resume ? 'Resume…' : 'Start…'}</button>{/if}
      </div>
    </header>

    {#key JSON.stringify([view.project_key, view.chat_id])}
      {#if find_open && session && !session.content_purged}<TranscriptFind root={transcript_root} revision={view.revision} section_labels={{ user: 'You', assistant: 'Reply', tool: 'Tool event', approval: 'Approval', notice: 'Notice' }} on_close={on_find_close} />{/if}
    {/key}
    <div class="transcript_scroll" data-transcript-scroll bind:this={transcript_root}>
      {@render workspace_context?.()}
      {#if session?.content_purged}
        <div class="notice warning">Stored private text was purged and can no longer be shown. Canonical record metadata remains daemon-owned.</div>
      {:else}
        {#if !view.runtime_known && !view.loading}<div class="notice">Loading workspace execution state…</div>{/if}
        {#if !view.process_claimed && view.runtime_known && !view.loading && !view.pending}
          <div class="workspace_execution_start_card">
            <p>{resume ? 'This session is closed. Resume it to continue the conversation.' : 'Execution has not started for this workspace.'}</p>
            <button class="primary" disabled={view.busy} onclick={open_start}>{resume ? 'Resume…' : 'Start…'}</button>
          </div>
        {/if}
        {#if view.loading}<p class="muted chat_state_line">Loading…</p>{/if}
        {#each grouped.loose as row (row.record.record_id)}
          <article class="chat_record chat_record_{row.record.kind}" data-transcript-body={row.record.kind} data-search-record={row.record.record_id}>
            <div class="message_role">{kind_label(row.record.kind)}</div>
            {#if row.record.kind === 'tool'}<ToolRecord text={row.text} complete={row.complete} truncated={row.record.truncated} progress={command_progress_entry(view.command_progress, row.record.record_id, row.record.attempt_id)} progress_active={view.command_progress?.active ?? false} content_error={row.error} />
            {:else if row.error}<p class="notice warning">Record content is unavailable ({row.error}).</p>
            {:else if row.text}{#if row.record.kind === 'assistant'}<div><MarkdownView content={row.text} {notify} {on_artifact} /></div>{:else}<div class="user_prompt">{row.text}</div>{/if}{/if}
            {#if row.record.truncated}<p class="small">Shown text was truncated to the record limit.</p>{/if}
          </article>
        {/each}
        {#each view.attempts as attempt (attempt.attempt_id)}
          <div class="turn_marker">
            <span>{workspace_attempt_label(attempt.phase)}</span>
            {#if attempt.outcome && !attempt.output_complete}<span class="status_badge">outcome {attempt.outcome} · display output incomplete</span>{/if}
            {#if attempt.error_code}<code>{attempt.error_code}</code>{/if}
          </div>
          {#each grouped.groups.get(attempt.attempt_id) ?? [] as row (row.record.record_id)}
            {@const envelope = row.record.kind === 'user' ? parse_attachment_prompt(row.text) : null}
            <article class="chat_record chat_record_{row.record.kind}" data-transcript-body={row.record.kind} data-search-record={row.record.record_id}>
              <div class="message_role">{kind_label(row.record.kind)}</div>
              {#if row.record.kind === 'tool'}<ToolRecord text={row.text} complete={row.complete} truncated={row.record.truncated} progress={command_progress_entry(view.command_progress, row.record.record_id, row.record.attempt_id)} progress_active={view.command_progress?.active ?? false} content_error={row.error} />
            {:else if row.error}<p class="notice warning">Record content is unavailable ({row.error}).</p>
              {:else if row.text && envelope?.kind === 'envelope'}
                <div class="user_prompt">{envelope.message}</div>
                <p class="small workspace_execution_files">Attached: {envelope.files.map((file) => file.name).join(', ')}</p>
              {:else if row.text}{#if row.record.kind === 'assistant'}<div><MarkdownView content={row.text} {notify} {on_artifact} /></div>{:else}<div class="user_prompt">{row.text}</div>{/if}{/if}
              {#if row.record.truncated}<p class="small">Shown text was truncated to the record limit ({row.record.retained_byte_count ?? row.record.original_byte_count} of {row.record.original_byte_count} bytes kept).</p>{/if}
            </article>
          {/each}
          {#if view.live && live_attempt?.attempt_id === attempt.attempt_id}
            <article class="chat_record chat_record_assistant chat_record_live" data-transcript-body="assistant">
              <div class="message_role">{harness_label(harness)}<span class="status_badge">{view.live.complete ? 'Finalizing…' : 'Streaming…'}</span></div>
              {#if view.live.text}<div class="user_prompt">{view.live.text}</div>{:else}<p class="muted">Waiting for assistant text…</p>{/if}
              {#if view.live.truncated}<p class="small">Live text is bounded; the final record keeps the retained bytes.</p>{/if}
            </article>
          {/if}
        {/each}
      {#if view.command_progress}
        <section class="command_activity" aria-label="Command observations">
          {#each view.command_progress.snapshot.entries.filter((entry) => !view.records.some((row) => row.record.kind === 'tool' && row.record.record_id.toLowerCase() === entry.record_id && row.record.attempt_id.toLowerCase() === view.command_progress?.snapshot.canonical_id)) as entry (entry.record_id)}
            <CommandProgress {entry} active={view.command_progress.active} />
          {/each}
          {#if view.command_progress.snapshot.capture_limited}<p class="small">Command capture is limited.{#if view.command_progress.snapshot.omitted_entries} {view.command_progress.snapshot.omitted_entries} earlier observations omitted.{/if}</p>{/if}
        </section>
      {/if}
      {#if view.progress_error}<p class="notice warning" role="status">{view.progress_error}</p>{/if}
        {#if !view.records.length && !view.attempts.length && !view.loading && !view.live}<p class="muted chat_state_line">No turns yet.{view.process_claimed ? ' Send the first message below.' : ''}</p>{/if}
        {#if view.content_more || view.attempts_more || view.history_capped}
          <div class="button_row chat_history_more"><button disabled={view.busy || view.loading} onclick={() => void controller.load_more()}>Load more{view.history_capped ? ' (bounded view)' : ''}</button></div>
        {/if}
      {/if}
    </div>

    {#snippet composer()}
    <div class="native_composer_wrap">
      <form class="native_composer" onsubmit={(event) => { event.preventDefault(); if (!send_disabled) void controller.send(); }}>
        <TextAttachments files={view.files} busy={view.busy} on_pick={() => void controller.pick_files()} on_remove={(attachment_id) => controller.remove_file(attachment_id)} />
        <textarea
          aria-label="Text prompt for this workspace conversation"
          aria-describedby="workspace_execution_limit"
          placeholder={view.can_send ? 'Message this workspace…' : 'Start the workspace process to send messages.'}
          rows="3"
          value={view.draft}
          disabled={view.busy && !uncertain}
          oninput={(event) => controller.set_draft(event.currentTarget.value)}
          onkeydown={composer_keydown}
        ></textarea>
        <div class="composer_toolbar">
          <div class="button_row">
            {#if busy_turn && view.can_cancel}<button type="button" class="cancel_button" disabled={view.busy} onclick={() => void controller.cancel()}>Cancel this turn</button>{/if}
          </div>
          <div class="button_row">
            <button class="primary send_button" type="submit" disabled={send_disabled}>{view.pending?.state === 'sending' ? 'Sending…' : 'Send'}</button>
          </div>
        </div>
      </form>
      <div class="composer_caption"><span>Read-only profile · one turn at a time · nothing is sent automatically</span><span id="workspace_execution_limit">{prompt_bytes.toLocaleString()} / {CHAT_LIMITS.prompt_bytes.toLocaleString()} bytes</span></div>
      <p class="composer_note">Workspace conversation · preview · draft stays local until Send. Ctrl / ⌘ + Enter to send · Shift + Enter for a new line.</p>
    </div>
    {/snippet}
    {#if retained_draft}
      <details class="retained_composer" bind:open={composer_open}><summary>Draft message{#if view.draft || view.files.length} · retained locally{/if}</summary>{@render composer()}</details>
    {:else}
      {@render composer()}
    {/if}
  {/if}

  {#if start_open && start_target}
    <dialog class="shell_dialog workspace_dialog workspace_execution_dialog" aria-labelledby="workspace_start_title" use:modal_focus oncancel={(event) => { if (!event.defaultPrevented) close_start(); }}>
      <header class="dialog_header">
        <div><h2 id="workspace_start_title">{resume ? 'Resume workspace execution' : 'Start workspace execution'}</h2><p class="dialog_project_name">{chat_label}</p></div>
        <button class="icon_button" aria-label="Close execution consent" onclick={close_start}>×</button>
      </header>
      <form onsubmit={submit_start}>
        <div class="settings_content">
          <p>{resume ? 'Resuming restarts the execution process for this workspace session.' : 'Starting execution launches the workspace\'s coding harness process for this session.'} It may use your configured provider account. The profile stays read-only; each turn is sent only by your explicit action.</p>
          <p>Messages and reported tool activity are retained in this project's private conversation history.</p>
          <label class="workspace_consent_check"><input type="checkbox" bind:checked={execution_consent} data-initial-focus /><span>I consent to {resume ? 'resuming' : 'starting'} execution for this workspace.</span></label>
        </div>
        <footer class="dialog_footer">
          <span>Consent is required before anything starts.</span>
          <div class="button_row">
            <button type="button" onclick={close_start}>Cancel</button>
            <button class="primary" type="submit" disabled={!execution_consent}>{resume ? 'Resume' : 'Start'}</button>
          </div>
        </footer>
      </form>
    </dialog>
  {/if}
</section>

<style>
  .workspace_execution { display: flex; flex-direction: column; flex: 1; min-height: 0; }
  .workspace_execution_start_card { margin: 18px 22px; padding: 14px 16px; border: 1px dashed var(--border); border-radius: 9px; display: grid; gap: 10px; justify-items: start; }
  .workspace_execution_start_card p { margin: 0; color: var(--muted_text); font-size: .867rem; }
  .workspace_execution_files { margin: 4px 0 0; }
  .workspace_execution_dialog .settings_content p { font-size: .933rem; }
  .workspace_consent_check { display: flex; gap: 10px; align-items: flex-start; font-size: .8rem; line-height: 1.55; }
  .workspace_consent_check input { width: 18px; height: 18px; margin-top: 2px; accent-color: var(--accent_text); flex-shrink: 0; }
</style>

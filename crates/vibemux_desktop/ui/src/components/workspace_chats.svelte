<script lang="ts">
  import { modal_focus } from '../lib/keyboard_shortcuts';
  import { harness_label } from '../lib/native_state';
  import type { Harness } from '../lib/ui_contract';
  import type { CodeArtifact } from './native_transcript.svelte';
  import WorkspaceExecution from './workspace_execution.svelte';
  import WorkspaceAccess from './workspace_access.svelte';
  import type { WorkspaceExecutionState, WorkspaceExecutionView } from '../lib/workspace_execution_state';
  import type { WorkspaceApprovalState, WorkspaceApprovalView } from '../lib/workspace_approval_state';
  import { workspace_can_reconcile, workspace_can_stop, workspace_effect_label, workspace_owner_label, workspace_phase_label, workspace_run_label, type WorkspaceChatView } from '../lib/workspace_chat_state';

  let { view, project_name, execution_view = null, execution_controller = null, approval_view = null, approval_controller = null, presentation = 'claude', notify = () => {}, on_artifact = () => {}, find_open = false, on_find_close = () => {}, on_refresh, on_load_more, on_select, on_create, on_reconcile, on_stop, on_retry }: {
    view: WorkspaceChatView; project_name: string;
    execution_view?: WorkspaceExecutionView | null; execution_controller?: WorkspaceExecutionState | null; presentation?: Harness;
    approval_view?: WorkspaceApprovalView | null; approval_controller?: WorkspaceApprovalState | null;
    notify?: (message: string) => void; on_artifact?: (artifact: CodeArtifact) => void;
    find_open?: boolean; on_find_close?: () => void;
    on_refresh: () => void; on_load_more: () => void; on_select: (chat_id: string) => void;
    on_create: (harness: Harness, title: string | null) => void; on_reconcile: () => void; on_stop: () => void; on_retry: () => void;
  } = $props();

  let create_open = $state(false);
  let create_target = $state<string | null>(null);
  let create_harness = $state<Harness>('claude');
  let create_title = $state('');
  let storage_consent = $state(false);
  let workspace_consent = $state(false);
  let stop_open = $state(false);
  let stop_target = $state<{ project_key: string | null; chat_id: string } | null>(null);

  const owner = $derived(view.owner);
  const mutation_locked = $derived(view.pending !== null);
  const execution_selected = $derived(execution_view?.project_key === view.project_key && execution_view?.chat_id === owner?.chat_id);
  const execution_locked = $derived(!execution_selected || !execution_view?.runtime_known || execution_view.process_claimed || execution_view.pending !== null);
  const unavailable = $derived(view.project_key !== null && view.error !== null && view.owners.length === 0 && !view.loading);
  const create_ready = $derived(storage_consent && workspace_consent);

  // A delayed project or selection update must never apply already-checked
  // consent or a Stop confirmation to a different target: the dialogs pin the
  // identity they were opened with, close when it changes, and re-check it at
  // submit before any callback runs.
  $effect(() => {
    if (create_target !== null && view.project_key !== create_target) close_create();
    if (stop_target !== null && (view.project_key !== stop_target.project_key || view.owner?.chat_id !== stop_target.chat_id)) close_stop();
  });

  function open_create() {
    create_harness = 'claude'; create_title = ''; storage_consent = false; workspace_consent = false;
    create_target = view.project_key;
    create_open = true;
  }
  function close_create() { create_open = false; create_target = null; }
  function submit_create(event: SubmitEvent) {
    event.preventDefault();
    if (!create_ready) return;
    if (create_target === null || view.project_key !== create_target) { close_create(); return; }
    const title = create_title.trim();
    close_create();
    on_create(create_harness, title === '' ? null : title);
  }
  function open_stop() {
    if (!owner) return;
    stop_target = { project_key: view.project_key, chat_id: owner.chat_id };
    stop_open = true;
  }
  function close_stop() { stop_open = false; stop_target = null; }
  function submit_stop(event: SubmitEvent) {
    event.preventDefault();
    if (mutation_locked || execution_locked) return;
    if (stop_target === null || view.project_key !== stop_target.project_key || view.owner?.chat_id !== stop_target.chat_id) { close_stop(); return; }
    close_stop();
    on_stop();
  }
</script>

<section class="workspace_chats" aria-label="Workspace management">
  {#if view.project_key === null}
    <div class="welcome">
      <h1>Manage chat workspaces</h1>
      <p>Select a project to see its lifetime workspaces, branches, and retained files.</p>
    </div>
  {:else}
    {#if view.pending}
      <div class="notice warning" role="status">
        <strong>Workspace action in progress or awaiting confirmation</strong>
        <p>The exact request and action ID are retained and nothing is re-sent automatically. Refresh re-checks read-only; Retry replays the same request and ID.</p>
        <div class="button_row">
          <button disabled={view.loading} onclick={on_refresh}>Refresh</button>
          <button onclick={on_retry}>Retry same action</button>
        </div>
      </div>
    {/if}
    {#if view.error && !unavailable}<div class="notice warning" role="alert">{view.error} <button onclick={on_refresh}>Refresh</button></div>{/if}

    <header class="chat_header">
      <div class="chat_heading">
        <h2>Workspaces</h2>
        <span class="status_badge">Preview</span>
        <span class="small">{project_name}</span>
      </div>
      <div class="button_row chat_actions">
        <button disabled={view.loading} onclick={on_refresh}>Refresh</button>
        <button class="primary" disabled={mutation_locked} onclick={open_create}>Create workspace…</button>
      </div>
    </header>

    {#if unavailable}
      <div class="availability_card"><strong>Workspace owners are unavailable right now</strong>
        <p>The daemon keeps the canonical lifetime records. The last observation is retained; nothing was re-sent.</p>
        <button onclick={on_refresh}>Try again</button>
      </div>
    {:else if !view.owners.length && !view.loading}
      <p class="muted workspace_state_line">No workspaces yet for this project.</p>
    {:else}
      <div class="workspace_layout">
        <nav class="workspace_list" aria-label="Workspaces for {project_name}">
          {#each view.owners as row (row.chat_id)}
            <button class:active={view.selected_owner === row.chat_id} onclick={() => on_select(row.chat_id)}>
              <span>{harness_label(row.harness)} · {workspace_phase_label(row.phase)}</span>
              <small>{workspace_run_label(row.run_status)}{row.workspace_branch ? ' · ' + row.workspace_branch : ''}</small>
            </button>
          {/each}
          {#if view.loading}<p class="small workspace_state_line">Loading workspaces…</p>{/if}
          {#if view.more}<button class="workspace_more" disabled={view.loading} onclick={on_load_more}>Load more workspaces</button>{/if}
        </nav>
        <div class="workspace_detail">
          {#if owner}
            {#snippet workspace_context()}
            <header class="chat_header workspace_detail_header">
              <div class="chat_heading">
                <h2 title={owner.chat_id}>{workspace_owner_label(owner, view.title)}</h2>
                <span class="status_badge">{workspace_phase_label(owner.phase)}</span>
                <span class="status_badge">{workspace_run_label(owner.run_status)}</span>
                <span class="status_badge">{harness_label(owner.harness)}</span>
                <span class="status_badge">version {owner.version}</span>
              </div>
              <div class="button_row chat_actions">
                <button disabled={mutation_locked || execution_locked || !workspace_can_reconcile(owner)} title="Close any execution session before reconciling this workspace." onclick={on_reconcile}>Reconcile</button>
                <button class="cancel_button" disabled={mutation_locked || execution_locked || !workspace_can_stop(owner)} title="Close any execution session before finalizing this workspace; files stay on disk." onclick={open_stop}>Stop workspace…</button>
              </div>
            </header>
            <div class="workspace_detail_body">
              {#if view.project_key}{#key `${view.project_key}:${owner.task_id}:${owner.run_id}`}
                <WorkspaceAccess project_key={view.project_key} project_id={owner.project_id} task_id={owner.task_id} run_id={owner.run_id} branch={owner.workspace_branch}
                  has_worktree={owner.workspace_planned && owner.workspace_effect === 'settled' && owner.workspace_verified_at !== null && (owner.phase === 'open' || owner.phase === 'finalized')} />
              {/key}{/if}
              {#if owner.phase === 'provisioning'}
                <p class="workspace_prep">Preparing the independent Git worktree and branch…</p>
              {/if}
              {#if owner.phase === 'recovery_required' && owner.error_code === 'workspace_cleanup_unverified'}
                <div class="notice warning"><strong>Cleanup could not be verified</strong>
                  <p>Operator investigation is required for this workspace. Fresh reconciliation is disabled until it is resolved; retrying an unconfirmed action stays available.</p>
                </div>
              {/if}
              {#if owner.error_code && owner.error_code !== 'workspace_cleanup_unverified'}<p class="small workspace_state_line">Error: <code>{owner.error_code}</code></p>{/if}
              {#if owner.phase !== 'finalized' && owner.error_code !== 'workspace_cleanup_unverified' && (!workspace_can_reconcile(owner) || !workspace_can_stop(owner))}
                <p class="small workspace_state_line">Reconciliation is required before this workspace can change state.</p>
              {/if}
              <details class="workspace_metadata"><summary>Workspace details</summary><dl class="about_list workspace_facts">
                <dt>Branch</dt><dd><code>{owner.workspace_branch ?? 'not created yet'}</code></dd>
                <dt>Workspace</dt><dd>{owner.workspace_planned ? 'planned' : 'unplanned'} · {workspace_effect_label(owner.workspace_effect)}</dd>
                <dt>Verified</dt><dd>{owner.workspace_verified_at ?? 'not verified yet'}</dd>
                <dt>Task / run</dt><dd><code>{owner.task_id} · {owner.run_id}</code></dd>
                {#if owner.admitted_tools.length}<dt>Admitted tools</dt><dd>{owner.admitted_tools.join(', ')}</dd>{/if}
              </dl>
              <p class="small workspace_retained">Files live in this workspace's own Git worktree and branch. Stopping or finalizing keeps the files on disk; the daemon never deletes them automatically.</p>
              </details>
            </div>
            {/snippet}
            {#if execution_selected && execution_view && execution_controller}
              <WorkspaceExecution view={execution_view} controller={execution_controller} {approval_view} {approval_controller} {presentation} {notify} {on_artifact} {find_open} {on_find_close} {workspace_context} />
            {:else}
              <div class="workspace_context_scroll">{@render workspace_context()}</div>
            {/if}
          {:else}
            <p class="muted workspace_state_line">Select a workspace to see its branch, run status, and retained files.</p>
          {/if}
        </div>
      </div>
    {/if}
  {/if}

  {#if create_open}
    <dialog class="shell_dialog workspace_dialog" aria-labelledby="workspace_create_title" use:modal_focus oncancel={(event) => { if (!event.defaultPrevented) close_create(); }}>
      <header class="dialog_header">
        <div><h2 id="workspace_create_title">Create workspace</h2><p class="dialog_project_name">{project_name}</p></div>
        <button class="icon_button" aria-label="Close workspace creation" onclick={close_create}>×</button>
      </header>
      <form onsubmit={submit_create}>
        <div class="settings_content workspace_form">
          <label class="settings_field"><span><strong>Harness</strong><small>The installed coding harness that will own this workspace.</small></span>
            <select data-initial-focus value={create_harness} onchange={(event) => (create_harness = event.currentTarget.value as Harness)}>
              <option value="claude">Claude Code</option><option value="codex">Codex</option>
            </select>
          </label>
          <label class="settings_field"><span><strong>Title</strong><small>Optional private name, stored by this project's daemon.</small></span>
            <input maxlength="256" bind:value={create_title} placeholder="Optional workspace title" />
          </label>
          <div class="workspace_consent">
            <p>Creating this workspace opens an independent Git worktree and branch for one chat, and stores the private title you provide in this project's daemon storage.</p>
            <label class="workspace_consent_check"><input type="checkbox" bind:checked={storage_consent} /><span>I consent to storing the private workspace title in this project's daemon storage.</span></label>
            <label class="workspace_consent_check"><input type="checkbox" bind:checked={workspace_consent} /><span>I consent to creating an independent Git worktree and branch for this workspace.</span></label>
          </div>
        </div>
        <footer class="dialog_footer">
          <span>Both consents are required. Start the execution session separately when the workspace is ready.</span>
          <div class="button_row">
            <button type="button" onclick={close_create}>Cancel</button>
            <button class="primary" type="submit" disabled={!create_ready}>Create workspace</button>
          </div>
        </footer>
      </form>
    </dialog>
  {/if}

  {#if stop_open && owner && stop_target}
    <dialog class="shell_dialog workspace_dialog workspace_stop_dialog" aria-labelledby="workspace_stop_title" use:modal_focus oncancel={(event) => { if (!event.defaultPrevented) close_stop(); }}>
      <header class="dialog_header">
        <div><h2 id="workspace_stop_title">Stop workspace</h2><p class="dialog_project_name">{workspace_owner_label(owner, view.title)}</p></div>
        <button class="icon_button" aria-label="Close stop confirmation" onclick={close_stop}>×</button>
      </header>
      <form onsubmit={submit_stop}>
        <div class="settings_content">
          <p>Stop {workspace_owner_label(owner, view.title)} ({harness_label(owner.harness)}){owner.workspace_branch ? ' on ' + owner.workspace_branch : ''}?</p>
          <p class="settings_note">The workspace becomes finalized. Its files are retained on disk in the independent worktree and branch; nothing is deleted. The daemon keeps the canonical lifetime record.</p>
          {#if owner.run_status === 'failed'}<p class="settings_note">The last run failed; stopping records the failure, not a success.</p>{/if}
        </div>
        <footer class="dialog_footer">
          <span>Stopping keeps the workspace files on disk.</span>
          <div class="button_row">
            <button type="button" onclick={close_stop}>Cancel</button>
            <button class="cancel_button" type="submit" data-initial-focus disabled={mutation_locked || execution_locked}>Stop workspace</button>
          </div>
        </footer>
      </form>
    </dialog>
  {/if}
</section>

<style>
  .workspace_chats { display: flex; flex-direction: column; flex: 1; min-height: 0; }
  .workspace_layout { display: grid; grid-template-columns: 280px minmax(0, 1fr); flex: 1; min-height: 0; }
  .workspace_list { border-right: 1px solid var(--border); overflow: auto; padding: 10px; display: grid; gap: 2px; align-content: start; }
  .workspace_list > button { display: grid; gap: 5px; text-align: left; border-color: transparent; background: transparent; padding: 10px 8px; }
  .workspace_list > button span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: .867rem; }
  .workspace_list > button small { font-size: .733rem; color: var(--muted_text); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .workspace_list > button.active { background: var(--accent_tint); }
  .workspace_more { text-align: left; background: transparent; border-color: transparent; font-size: .733rem; }
  .workspace_detail { display: flex; flex-direction: column; overflow: hidden; min-width: 0; min-height: 0; }
  .workspace_context_scroll { overflow: auto; min-height: 0; }
  .workspace_detail_header { border-bottom: 1px solid var(--border); }
  .workspace_detail_body { padding: 16px 22px 26px; }
  .workspace_state_line { padding: 14px 22px; }
  .workspace_detail_body .workspace_state_line { padding: 0 0 10px; }
  .workspace_prep { margin: 0 0 14px; padding: 10px 14px; border: 1px dashed var(--border); border-radius: 9px; color: var(--muted_text); font-size: .8rem; }
  .workspace_facts { margin: 4px 0 0; }
  .workspace_retained { margin: 16px 0 0; }
  .workspace_form { padding: 6px 25px 10px; }
  .workspace_consent { margin: 6px 0 12px; display: grid; gap: 10px; }
  .workspace_consent p { margin: 0; color: var(--muted_text); font-size: .8rem; line-height: 1.6; }
  .workspace_consent_check { display: flex; gap: 10px; align-items: flex-start; font-size: .8rem; line-height: 1.55; }
  .workspace_consent_check input { width: 18px; height: 18px; margin-top: 2px; accent-color: var(--accent_text); flex-shrink: 0; }
  .workspace_dialog { width: min(560px, calc(100vw - 48px)); }
  .workspace_stop_dialog .settings_content p { font-size: .933rem; }
  @media (max-width: 850px) {
    .workspace_layout { grid-template-columns: 1fr; grid-template-rows: auto minmax(0, 1fr); }
    .workspace_list { border-right: 0; border-bottom: 1px solid var(--border); max-height: 34vh; }
  }
</style>

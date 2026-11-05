<script lang="ts">
  import { modal_focus } from '../lib/keyboard_shortcuts';
  import type { WorkspaceApprovalState, WorkspaceApprovalView } from '../lib/workspace_approval_state';
  import type { WorkspaceApproval } from '../lib/workspace_approval_contract';

  let { view, controller }: { view: WorkspaceApprovalView; controller: WorkspaceApprovalState } = $props();
  const labels: Record<WorkspaceApproval['tool'], string> = {
    read_files: 'Read files', search_files: 'Search files', edit_files: 'Edit files',
    write_files: 'Write files', run_dev_tests: 'Run development tests',
  };
  function delivery_label(delivery: WorkspaceApproval['delivery']): string {
    return delivery === 'written' ? 'Response written' : delivery === 'unknown' ? 'Delivery unknown'
      : delivery === 'claimed' ? 'Delivery claimed' : 'No response delivery recorded';
  }
</script>

{#if view.chat_id && (view.approval || view.pending || view.error)}
  <section class="workspace_approval notice" aria-label="Tool approval">
    <div class="approval_heading">
      <strong>{view.approval ? labels[view.approval.tool] : 'Tool approval'}</strong>
      {#if view.approval}<span class="status_badge">{view.expired ? 'Expired' : view.approval.state}</span>{/if}
    </div>
    {#if view.uncertain}
      <p role="status">Decision receipt unconfirmed. The original choice and operation are retained; nothing is sent again.</p>
      <button type="button" disabled={view.loading} onclick={() => void controller.recover()}>Recover receipt</button>
    {:else if view.busy}
      <p role="status">Recording your decision…</p>
    {:else if view.approval?.decision}
      <p>Decision recorded: {view.approval.decision === 'allow_once' ? 'Allow once' : 'Deny'} · {delivery_label(view.approval.delivery)}.</p>
      <p class="small">Delivery describes the response pipe. It does not establish tool execution or turn success.</p>
    {:else if view.time_unverified}
      <p>The approval deadline needs a refresh because the local clock changed.</p>
    {:else if view.expired}
      <p>Approval expired. Decisions are disabled.</p>
    {:else if view.approval?.state === 'invalidated'}
      <p>This approval is no longer available for a decision.</p>
    {:else if view.approval}
      <p>Review the private JSON input before allowing this tool once.</p>
    {/if}
    {#if view.error}<p class="approval_error" role="alert">{view.error}</p>{/if}
    {#if view.notice && !view.error}<p class="small" role="status">{view.notice}</p>{/if}
    <div class="button_row">
      <button type="button" disabled={!view.can_review || view.content_loading} onclick={() => void controller.open_details()}>Review input</button>
      <button type="button" disabled={view.loading || view.busy} onclick={() => void controller.refresh(true)}>Refresh</button>
      {#if !view.details_open && !view.approval?.decision}
        <button type="button" class="cancel_button" disabled={!view.can_deny} onclick={() => void controller.decide('deny')}>Deny</button>
      {/if}
    </div>
  </section>
{/if}

{#if view.details_open && view.approval}
  <dialog class="shell_dialog approval_dialog" aria-labelledby="workspace_approval_title"
    use:modal_focus oncancel={(event) => { if (!event.defaultPrevented) controller.close_details(); }}>
    <header class="dialog_header">
      <div><h2 id="workspace_approval_title">{labels[view.approval.tool]}</h2><p class="small">Private tool input</p></div>
      <button type="button" class="icon_button" aria-label="Close approval review" onclick={() => controller.close_details()}>×</button>
    </header>
    <div class="settings_content">
      {#if view.content_loading}<p role="status">Loading the bounded JSON input…</p>
      {:else if view.content_error}<p role="alert">The private input is unavailable ({view.content_error}).</p>
      {:else if view.content_complete}<textarea class="approval_json" readonly rows="14" spellcheck="false" aria-label="Private JSON tool input" value={view.content}></textarea>
      {:else}<p role="status">The input has not been verified. Close and review it again after refreshing.</p>{/if}
      {#if view.error}<p role="alert">{view.error}</p>{/if}
      {#if view.expired}<p>Approval expired. Decisions are disabled.</p>{/if}
      {#if view.approval.decision}<p>Recorded decision: {view.approval.decision === 'allow_once' ? 'Allow once' : 'Deny'} · {delivery_label(view.approval.delivery)}.</p>{/if}
    </div>
    <footer class="dialog_footer">
      <span>Allow once applies only to this saved approval.</span>
      <div class="button_row">
        <button type="button" onclick={() => controller.close_details()}>Close</button>
        <button type="button" class="cancel_button" data-initial-focus disabled={!view.can_deny} onclick={() => void controller.decide('deny')}>Deny</button>
        <button type="button" class="primary" disabled={!view.can_allow} onclick={() => void controller.decide('allow_once')}>Allow once</button>
      </div>
    </footer>
  </dialog>
{/if}

<style>
  .workspace_approval { margin: 12px 18px; flex-shrink: 0; }
  .approval_heading { display: flex; align-items: center; gap: 8px; }
  .workspace_approval p { margin: 8px 0; }
  .approval_error { color: var(--danger, var(--muted_text)); }
  .approval_dialog { width: min(760px, calc(100vw - 36px)); }
  .approval_json { max-height: min(48vh, 440px); overflow: auto; white-space: pre-wrap; overflow-wrap: anywhere; padding: 12px; border: 1px solid var(--border); border-radius: 6px; }
</style>

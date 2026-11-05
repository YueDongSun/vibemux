<script lang="ts">
  import { NATIVE_LIMITS, prompt_error, should_send } from '../lib/native_config';
  import type { Harness, AskTargets } from '../lib/ui_contract';
  let {
    draft, harness, targets, disabled = false, sending = false, uncertain = false,
    on_draft, on_harness, on_send, on_retry,
  }: {
    draft: string; harness: Harness; targets: AskTargets | undefined; disabled?: boolean; sending?: boolean; uncertain?: boolean;
    on_draft: (value: string) => void; on_harness: (value: Harness) => void;
    on_send: (explicit_new?: boolean) => Promise<unknown>; on_retry: () => Promise<unknown>;
  } = $props();
  const bytes = $derived(new TextEncoder().encode(draft).byteLength);
  const available = $derived(targets?.targets.some((target) => target.harness === harness && target.available) ?? false);
  const send_disabled = $derived(disabled || sending || !available || Boolean(prompt_error(draft)) || uncertain);
  const profile = $derived(harness === 'claude' ? 'Claude Code: tool-free, one turn.' : 'Codex: configured read-only, one turn.');
</script>

<div class="native_composer_wrap">
  {#if uncertain}
    <div class="notice warning" role="status">
      <strong>Submission not yet confirmed</strong>
      <p>The prompt is retained. Reconcile the same submission without creating another attempt.</p>
      <div class="button_row">
        <button disabled={sending} onclick={() => void on_retry()}>Reconcile submission</button>
        <button disabled={disabled || sending || !available || Boolean(prompt_error(draft))} onclick={() => void on_send(true)}>Send as a new independent Ask</button>
      </div>
    </div>
  {/if}
  <form class="native_composer" onsubmit={(event) => { event.preventDefault(); if (!send_disabled) void on_send(); }}>
    <textarea
      id="message_composer"
      aria-label="Text prompt for a new independent Ask"
      aria-describedby="composer_profile composer_limit"
      placeholder="Ask a question…"
      rows="3"
      value={draft}
      oninput={(event) => on_draft(event.currentTarget.value)}
      onkeydown={(event) => { if (should_send(event)) { event.preventDefault(); if (!send_disabled) void on_send(); } }}
    ></textarea>
    <div class="composer_toolbar">
      <div class="button_row">
        <button type="button" disabled title="Native Ask is text-only. Browser attachments stay in the browser workspace." aria-label="Attachments unavailable in native Ask">＋ Attach</button>
        <button type="button" disabled title="The configured route controls the model and reasoning profile.">Thinking</button>
      </div>
      <div class="button_row">
        <label class="route_picker"><span class="sr_only">Harness route</span>
          <select value={harness} disabled={sending || uncertain} onchange={(event) => on_harness(event.currentTarget.value as Harness)}>
            <option value="claude">Claude Code</option><option value="codex">Codex</option>
          </select>
        </label>
        <button class="primary send_button" type="submit" disabled={send_disabled}>{sending ? 'Submitting…' : 'Send ↑'}</button>
      </div>
    </div>
  </form>
  <div class="composer_caption"><span id="composer_profile">{profile}</span><span id="composer_limit">{bytes.toLocaleString()} / {NATIVE_LIMITS.prompt_bytes.toLocaleString()} bytes</span></div>
  <p class="composer_note">Each Send starts a new independent Ask. Enter to send · Shift + Enter for a new line.</p>
</div>

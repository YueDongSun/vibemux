<script lang="ts">
  import type { console_response } from '../api_client';
  import { dsh_turn_summary, section_data } from '../console_policy';
  import Section_state from './section_state.svelte';

  let { console_data }: { console_data: console_response | null } = $props();
  const dsh = $derived(section_data(console_data?.dsh));
  const slots = $derived(section_data(console_data?.slots));
</script>

<div class="console_grid">
  <section class="panel console_panel" aria-labelledby="dsh_heading">
    <div class="panel_heading">
      <div>
        <p class="eyebrow">DSH session</p>
        <h2 id="dsh_heading">{dsh ? dsh.phase : 'Session status'}</h2>
      </div>
      {#if dsh}
        <span class="state_pill" data-state={dsh.transport_open ? 'active' : 'idle'}>{dsh.transport_open ? 'Transport open' : 'Transport closed'}</span>
      {/if}
    </div>
    <Section_state section={console_data?.dsh} />
    {#if dsh}
      <dl class="console_facts">
        <div><dt>Provider</dt><dd>{dsh.provider || '—'}</dd></div>
        <div><dt>Model</dt><dd>{dsh.model || '—'}</dd></div>
        <div><dt>Turns</dt><dd>{dsh_turn_summary(dsh)}</dd></div>
        <div><dt>Records</dt><dd>{dsh.records}</dd></div>
      </dl>
      <p class="list_note">An idle session is not a finished task. This view never shows the transcript.</p>
    {/if}
  </section>

  <section class="panel console_panel" aria-labelledby="slots_heading">
    <div class="panel_heading">
      <div>
        <p class="eyebrow">Dual-track workflow</p>
        <h2 id="slots_heading">Slots</h2>
      </div>
      {#if slots}<span class="count_badge">{slots.routes.rows.length}</span>{/if}
    </div>
    <Section_state section={console_data?.slots} />
    {#if slots}
      <dl class="console_facts">
        <div><dt>Evidence class</dt><dd class="mono">{slots.evidence_class || '—'}</dd></div>
      </dl>
      {#if slots.routes.rows.length === 0}
        <p class="empty_state compact">The workflow configuration has no slots.</p>
      {:else}
        <div class="table_scroll">
          <table class="console_table">
            <thead><tr><th scope="col">Slot</th><th scope="col">Route</th><th scope="col">Mode</th></tr></thead>
            <tbody>
              {#each slots.routes.rows as slot}
                <tr>
                  <td class="mono">{slot.slot_id}</td>
                  <td class="mono">{slot.route_label}</td>
                  <td><span class="state_pill" data-state={slot.executes ? 'executes' : 'probe_only'}>{slot.executes ? 'Executes' : 'No executing route'}</span></td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
      {#if slots.routes.truncated}<p class="list_note">More slots exist than this view shows.</p>{/if}
    {/if}
  </section>
</div>

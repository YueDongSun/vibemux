<script lang="ts">
  import type { console_response } from '../api_client';
  import { section_data, short_id } from '../console_policy';
  import Section_state from './section_state.svelte';

  let { console_data }: { console_data: console_response | null } = $props();
  const decisions = $derived(section_data(console_data?.decisions));
</script>

<section class="panel console_panel" aria-labelledby="decisions_heading">
  <div class="panel_heading">
    <div>
      <p class="eyebrow">Decision routing</p>
      <h2 id="decisions_heading">Recorded decisions</h2>
    </div>
    {#if decisions}<span class="count_badge">{decisions.rows.length}</span>{/if}
  </div>
  <Section_state section={console_data?.decisions} />
  {#if decisions}
    {#if decisions.rows.length === 0}
      <p class="empty_state compact">No decisions are recorded.</p>
    {:else}
      <div class="table_scroll">
        <table class="console_table">
          <thead>
            <tr>
              <th scope="col">Decision</th><th scope="col">Status</th><th scope="col">Chosen</th>
              <th scope="col">Source</th><th scope="col">Created</th><th scope="col">Consumed</th>
            </tr>
          </thead>
          <tbody>
            {#each decisions.rows as decision (decision.decision_id)}
              <tr>
                <td class="mono" title={decision.decision_id}>{short_id(decision.decision_id)}</td>
                <td><span class="state_pill" data-state={decision.status}>{decision.status}</span></td>
                <td>{decision.chosen_harness ?? '—'}</td>
                <td class="mono">{decision.source}</td>
                <td>{new Date(decision.created_at).toLocaleString()}</td>
                <td>{decision.consumed ? 'Yes' : 'No'}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
    {#if decisions.truncated}<p class="list_note">Older decisions are not shown, or newer pages are still loading.</p>{/if}
  {/if}
  <p class="list_note">A decision is advice. It runs nothing, and a dispatch still checks every gate.</p>
</section>

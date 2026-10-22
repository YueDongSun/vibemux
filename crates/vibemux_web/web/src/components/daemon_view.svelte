<script lang="ts">
  import type { console_response } from '../api_client';
  import { queue_summary, route_mode_label, section_data } from '../console_policy';
  import Section_state from './section_state.svelte';

  let { console_data }: { console_data: console_response | null } = $props();
  const health = $derived(section_data(console_data?.health));
  const plugins = $derived(section_data(console_data?.plugins));
  const routes = $derived(section_data(console_data?.routes));
</script>

<div class="console_grid">
  <section class="panel console_panel" aria-labelledby="daemon_health_heading">
    <div class="panel_heading">
      <div>
        <p class="eyebrow">Daemon</p>
        <h2 id="daemon_health_heading">Health</h2>
      </div>
      {#if health}
        <span class="state_pill" data-state={health.healthy ? 'active' : 'failed'}>{health.healthy ? 'Healthy' : 'Unhealthy'}</span>
      {/if}
    </div>
    <Section_state section={console_data?.health} />
    {#if health}
      <dl class="console_facts">
        <div><dt>Store schema</dt><dd>{health.store_schema_version}</dd></div>
        <div><dt>Writer queue</dt><dd>{queue_summary(health)}</dd></div>
      </dl>
    {/if}
  </section>

  <section class="panel console_panel" aria-labelledby="daemon_plugins_heading">
    <div class="panel_heading">
      <div>
        <p class="eyebrow">Daemon</p>
        <h2 id="daemon_plugins_heading">Plugins</h2>
      </div>
      {#if plugins}<span class="count_badge">{plugins.rows.length}</span>{/if}
    </div>
    <Section_state section={console_data?.plugins} />
    {#if plugins}
      {#if plugins.rows.length === 0}
        <p class="empty_state compact">No plugins are registered with this daemon.</p>
      {:else}
        <div class="table_scroll">
          <table class="console_table">
            <thead>
              <tr><th scope="col">Plugin</th><th scope="col">State</th><th scope="col">Restarts</th><th scope="col">Last error</th></tr>
            </thead>
            <tbody>
              {#each plugins.rows as plugin}
                <tr>
                  <td class="mono">{plugin.plugin_id}</td>
                  <td><span class="state_pill" data-state={plugin.state}>{plugin.state}</span></td>
                  <td>{plugin.restarts}/{plugin.max_restarts}</td>
                  <td class="mono">{plugin.last_error_code ?? '—'}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
      {#if plugins.truncated}<p class="list_note">More plugins exist than this view shows.</p>{/if}
    {/if}
  </section>

  <section class="panel console_panel wide_panel" aria-labelledby="daemon_routes_heading">
    <div class="panel_heading">
      <div>
        <p class="eyebrow">Harness dispatch</p>
        <h2 id="daemon_routes_heading">Routes</h2>
      </div>
      {#if routes}<span class="count_badge">{routes.rows.length}</span>{/if}
    </div>
    <Section_state section={console_data?.routes} />
    {#if routes}
      {#if routes.rows.length === 0}
        <p class="empty_state compact">The dispatch configuration has no routes.</p>
      {:else}
        <div class="table_scroll">
          <table class="console_table">
            <thead><tr><th scope="col">Harness</th><th scope="col">Protocol</th><th scope="col">Mode</th></tr></thead>
            <tbody>
              {#each routes.rows as route}
                <tr>
                  <td>{route.harness}</td>
                  <td class="mono">{route.protocol}</td>
                  <td><span class="state_pill" data-state={route.mode}>{route_mode_label(route.mode)}</span></td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
      {#if routes.truncated}<p class="list_note">More routes exist than this view shows.</p>{/if}
    {/if}
    <p class="list_note">This view lists the configured routes. It cannot start a probe or a dispatch.</p>
  </section>
</div>

<script lang="ts">
  import { CONSOLE_SECTIONS, type ConsoleSection, type ReadViewState } from '../lib/read_state';
  import ProbePanel from './probe_panel.svelte';
  import type { ProbeState } from '../lib/probe_state';
  let { view, on_section, on_refresh, on_next, on_task_open, probe_controller = null }: {
    view: ReadViewState; on_section: (section: ConsoleSection) => void; on_refresh: () => void;
    on_next: () => void; on_task_open: (task_id: string) => void;
    probe_controller?: ProbeState | null;
  } = $props();
  const labels: Record<ConsoleSection, string> = { overview: 'Overview', harnesses: 'Harnesses', workspace: 'Workspace', tasks: 'Tasks', daemon: 'Daemon', sessions: 'Sessions', decisions: 'Decisions' };
</script>

<section class="console_workspace" aria-labelledby="console_heading">
  <header class="console_header"><div><h1 id="console_heading">Console</h1><p>Read-only observations for the selected project.</p></div><button disabled={!view.project_key || view.console_loading} onclick={on_refresh}>{view.console_loading ? 'Reading…' : 'Refresh'}</button></header>
  {#if !view.project_key}<div class="empty_console"><h2>Choose a project</h2><p>Select a registered project to inspect its available diagnostics.</p></div>
  {:else}
    <div class="console_layout">
      <nav aria-label="Console sections">{#each CONSOLE_SECTIONS as section}<button class:active={view.console_section === section} aria-current={view.console_section === section ? 'page' : undefined} onclick={() => on_section(section)}>{labels[section]}</button>{/each}</nav>
      <div class="console_content" aria-busy={view.console_loading}>
        <header class="section_heading"><h2>{labels[view.console_section]}</h2><small>{view.console?.observed_at ? `Observed ${view.console.observed_at}` : 'No observation yet'}</small></header>
        {#if view.console_error}<p class="console_notice" role="alert">{view.console_error}</p>{/if}
        {#if view.console_section === 'harnesses' && probe_controller}<ProbePanel controller={probe_controller} project_key={view.project_key} />{/if}
        {#if !view.console && view.console_loading}<p class="muted_console">Reading available observations…</p>
        {:else if !view.console}<p class="muted_console">Refresh to read this section.</p>
        {/if}
        {#if view.console}
          {#each view.console.blocks as block (block.key)}
            <section class="console_card" aria-label={block.title}>
              <header><h3>{block.title}</h3><span class="availability">{block.availability === 'loaded' ? 'Loaded' : 'Unavailable'}</span></header>
              {#if block.availability === 'unavailable'}
                {#if block.code}<code class="error_code">{block.code}</code>{/if}
                {#if block.hint}<p class="muted_console">{block.hint}</p>{/if}
              {:else}
                {#if !block.rows.length}<p class="muted_console">No records were returned.</p>
                {:else}<div class="table_scroll"><table><thead><tr>{#each block.columns as column}<th scope="col">{column}</th>{/each}</tr></thead><tbody>{#each block.rows as row (row.key)}<tr>{#each row.cells as cell, index}<td>{#if index === 0 && row.task_id}<button class="task_link" onclick={() => on_task_open(row.task_id ?? '')}>{cell}</button>{:else}{cell}{/if}</td>{/each}</tr>{/each}</tbody></table></div>{/if}
                {#if block.truncated}<p class="bounded_note">This is a bounded observation; additional records are not shown here.</p>{/if}
                {#if block.hint}<p class="muted_console">{block.hint}</p>{/if}
              {/if}
            </section>
          {/each}
          {#if view.console_section === 'decisions' && view.console.next_after_sequence !== null}<div class="console_pagination"><button disabled={view.console_loading} onclick={on_next}>Next decisions page</button><small>Refresh returns to the first audit-sequence page.</small></div>{/if}
        {/if}
      </div>
    </div>
  {/if}
</section>

<style>
  .console_workspace{display:flex;flex:1;min-height:0;min-width:0;flex-direction:column;background:var(--bg);color:var(--text_primary)}.console_header{display:flex;align-items:center;justify-content:space-between;gap:18px;padding:25px 28px 20px;border-bottom:1px solid var(--border)}.console_header h1{font:400 30px/1.2 var(--font_display);margin:0}.console_header p{margin:7px 0 0;font-size:12px;color:var(--muted_text)}.console_layout{display:flex;flex:1;min-width:0;min-height:0}.console_layout>nav{width:170px;flex-shrink:0;padding:18px 12px;border-right:1px solid var(--border);background:var(--surface);display:flex;flex-direction:column;gap:5px}.console_layout>nav button{text-align:left;background:transparent;border-color:transparent;font-size:13px;padding:10px 12px}.console_layout>nav .active{background:var(--accent_tint);border-color:var(--accent_edge);color:var(--accent_text)}.console_content{flex:1;min-width:0;overflow:auto;padding:24px 28px 40px}.section_heading{display:flex;align-items:center;justify-content:space-between;gap:15px;flex-wrap:wrap;margin-bottom:20px}.section_heading h2{font:24px var(--font_display);margin:0}.section_heading small,.muted_console,.bounded_note{font-size:12px;color:var(--muted_text)}.console_card{border:1px solid var(--border);border-radius:12px;background:var(--surface_alt);padding:18px;margin-bottom:16px;min-width:0}.console_card>header{display:flex;justify-content:space-between;align-items:center;gap:16px;margin-bottom:14px}.console_card h3{font-size:14px;margin:0}.availability{font-size:10px;border:1px solid var(--border);border-radius:16px;padding:3px 8px;color:var(--muted_text)}.table_scroll{max-width:100%;overflow:auto}table{border-collapse:collapse;min-width:100%;font-size:12px}th,td{text-align:left;vertical-align:top;padding:10px 12px;border-bottom:1px solid var(--border);max-width:420px;overflow-wrap:anywhere}th{font-size:10px;font-weight:600;color:var(--muted_text);white-space:nowrap}td:first-child{font-family:var(--font_code);font-size:11px}.console_card .muted_console{margin:14px 0 0;line-height:1.55}.error_code{display:inline-block;padding:7px 9px;border-radius:7px;background:var(--surface);font-size:11px;overflow-wrap:anywhere}.task_link{border:0;background:transparent;padding:0;color:var(--accent_text);font:inherit;text-align:left;text-decoration:underline;text-underline-offset:3px}.console_notice{padding:12px 14px;border:1px solid var(--border);border-radius:9px;background:var(--surface);font-size:12px}.console_pagination{display:flex;align-items:center;gap:14px;flex-wrap:wrap}.console_pagination small{font-size:11px;color:var(--muted_text)}.empty_console{padding:44px 28px;color:var(--muted_text);font-size:13px}.empty_console h2{font:24px var(--font_display);color:var(--text_primary)}
  @media(max-width:800px){.console_layout{flex-direction:column}.console_layout>nav{width:100%;flex-direction:row;overflow:auto;border-right:0;border-bottom:1px solid var(--border);padding:10px}.console_layout>nav button{white-space:nowrap;padding:7px 10px}.console_content{padding:20px 16px}.console_header{padding:22px 18px}.console_card{padding:14px}}
</style>

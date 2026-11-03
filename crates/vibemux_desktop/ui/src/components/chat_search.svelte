<script lang="ts">
  import { modal_focus } from '../lib/keyboard_shortcuts';
  import { SEARCH_LIMITS, type SearchHit, type SearchScope } from '../lib/chat_search_contract';
  import type { SearchView } from '../lib/chat_search_state';

  let { view, on_query, on_scope, on_case_sensitive, on_include_archived, on_search, on_more, on_close, on_open }: {
    view: SearchView; on_query: (query: string) => void; on_scope: (scope: SearchScope) => void;
    on_case_sensitive: (value: boolean) => void; on_include_archived: (value: boolean) => void;
    on_search: () => void; on_more: () => void; on_close: () => void; on_open: (hit: SearchHit) => void;
  } = $props();
  const query_bytes = $derived(new TextEncoder().encode(view.query).length);
  const gaps = $derived(view.stats.unavailable_records + view.stats.unavailable_titles + view.stats.purged_chats + view.stats.changed_chats);
</script>

{#if view.is_open}
  <dialog class="shell_dialog search_dialog" aria-label="Search project history" aria-modal="true" use:modal_focus
    oncancel={(event) => { if (!event.defaultPrevented) on_close(); }}>
    <header class="dialog_header"><h2>Search project history</h2><button type="button" class="icon_button" aria-label="Close search" onclick={on_close}>×</button></header>
    <form class="search_form" onsubmit={(event) => { event.preventDefault(); if (view.can_search) on_search(); }}>
      <label class="query_label">Literal text
        <input data-initial-focus type="search" value={view.query} maxlength={SEARCH_LIMITS.query_bytes} autocomplete="off" spellcheck="false"
          placeholder="Search retained titles and messages…" oninput={(event) => on_query(event.currentTarget.value)} />
      </label>
      <div class="search_options">
        <label>Search in
          <select value={view.scope} onchange={(event) => on_scope(event.currentTarget.value as SearchScope)}>
            <option value="all">All</option><option value="saved">Saved</option><option value="workspace">Workspace</option>
          </select>
        </label>
        <label><input type="checkbox" checked={view.case_sensitive} onchange={(event) => on_case_sensitive(event.currentTarget.checked)} />Match case</label>
        <label><input type="checkbox" checked={view.include_archived} onchange={(event) => on_include_archived(event.currentTarget.checked)} />Include archived / finalized</label>
        <button type="submit" class="primary" disabled={!view.can_search}>Search</button>
      </div>
      <p class="search_hint">{query_bytes} / {SEARCH_LIMITS.query_bytes} UTF-8 bytes.</p>
      {#if query_bytes > SEARCH_LIMITS.query_bytes}<p role="alert">Shorten the search to 256 UTF-8 bytes.</p>{/if}
      <p class="search_hint">Searches this registered project's retained private titles and messages. Results are cleared when this window closes. Up to 256 UTF-8 bytes of literal text.</p>
    </form>
    <div class="search_results" aria-label="Search results" aria-busy={view.busy}>
      {#if view.busy}<p role="status">Scanning retained history…</p>{/if}
      {#if view.error}<p class="search_error" role="alert">{view.error}</p>{/if}
      {#if view.notice}<p role="status">{view.notice}</p>{/if}
      {#if gaps > 0}
        <p class="search_gap" role="status">History has gaps: {view.stats.unavailable_records} unavailable records, {view.stats.unavailable_titles} unavailable titles, {view.stats.purged_chats} purged conversations, and {view.stats.changed_chats} changed conversations.</p>
      {/if}
      {#if view.hits.length}
        <ul class="search_hit_list">
          {#each view.hits as hit, index (index)}
            <li><button type="button" class="search_hit" onclick={() => on_open(hit)}>
              <span class="hit_heading"><strong>{hit.title || 'Conversation ' + hit.chat_id.slice(0, 8)}</strong><span>{hit.kind === 'chat' ? 'Saved' : 'Workspace'}</span></span>
              <span class="hit_location">{hit.location.target === 'title' ? 'Title' : 'Record · ' + hit.location.record_kind} · {hit.preview.match_count} {hit.preview.match_count === 1 ? 'match' : 'matches'}</span>
              <span class="hit_preview">{hit.preview.before}<mark>{hit.preview.matched}</mark>{hit.preview.after}</span>
              {#if hit.preview.truncated}<span class="search_hint">Excerpt</span>{/if}
            </button></li>
          {/each}
        </ul>
      {/if}
      {#if view.searched}
        {#if view.complete}
          <p role="status">{view.hits.length ? gaps ? 'Scan complete for available history.' : 'Scan complete.' : gaps ? 'No matches in the available history. Some history could not be searched.' : 'No matches in the retained history.'}</p>
        {:else if !view.capped && !view.busy}
          <p role="status">{view.hits.length ? 'More history remains to be searched.' : 'No matches on this page. More history remains to be searched.'}</p>
        {/if}
        <p class="search_hint">Scanned {view.stats.chats_scanned.toLocaleString()} conversations, {view.stats.records_scanned.toLocaleString()} records, and {view.stats.bytes_scanned.toLocaleString()} bytes.</p>
      {/if}
    </div>
    <footer class="dialog_footer"><button type="button" disabled={!view.can_more} onclick={on_more}>Load more</button><button type="button" onclick={on_close}>Close</button></footer>
  </dialog>
{/if}

<style>
  .search_dialog { width: min(820px, calc(100vw - 36px)); max-height: calc(100vh - 48px); }
  .search_form { display: grid; gap: 10px; padding: 0 18px 14px; }
  .query_label { display: grid; gap: 5px; font-size: 12px; }
  .query_label input { width: 100%; box-sizing: border-box; }
  .search_options { display: flex; flex-wrap: wrap; gap: 12px; align-items: center; }
  .search_options label { display: flex; align-items: center; gap: 6px; font-size: 12px; }
  .search_hint { color: var(--muted_text); font-size: 11px; margin: 0; }
  .search_results { overflow-y: auto; min-height: 100px; max-height: 52vh; padding: 0 18px 14px; }
  .search_results > p { font-size: 12px; }
  .search_error { color: var(--danger, var(--muted_text)); }
  .search_gap { color: var(--muted_text); }
  .search_hit_list { display: grid; gap: 8px; padding: 0; margin: 10px 0; list-style: none; }
  .search_hit { display: grid; gap: 5px; width: 100%; text-align: left; padding: 12px; }
  .hit_heading { display: flex; justify-content: space-between; gap: 12px; font-size: 12px; }
  .hit_heading strong { overflow-wrap: anywhere; min-width: 0; }
  .hit_heading > span, .hit_location { color: var(--muted_text); font-size: 11px; }
  .hit_preview { white-space: pre-wrap; overflow-wrap: anywhere; font-size: 12px; }
  mark { color: inherit; background: var(--search_match, #85632255); border-radius: 2px; }
</style>

<script lang="ts">
  import { modal_focus } from '../lib/keyboard_shortcuts';
  import { PALETTE_LIMITS, search_palette, type PaletteEntry, type PaletteAction } from '../lib/palette_entries';
  let { entries, on_select, on_close }: { entries: PaletteEntry[]; on_select: (action: PaletteAction) => void; on_close: () => void } = $props();
  let query = $state(''), selected = $state(0);
  const results = $derived(search_palette(entries, query));
  const history_search = $derived(entries.find((entry) => entry.action.kind === 'search_history' && !entry.disabled));
  function select(entry: PaletteEntry | undefined) { if (entry && !entry.disabled) on_select(entry.action); }
  function keydown(event: KeyboardEvent) {
    if (event.isComposing || event.keyCode === 229) return;
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') { event.preventDefault(); selected = results.length ? (selected + (event.key === 'ArrowDown' ? 1 : -1) + results.length) % results.length : 0; }
    else if (event.key === 'Enter') { event.preventDefault(); select(results[selected]); }
  }
</script>

<dialog class="shell_dialog palette_dialog" aria-label="Search projects, conversations, and actions" use:modal_focus oncancel={(event) => { if (!event.defaultPrevented) on_close(); }}>
  <div class="palette_input_row"><span aria-hidden="true">⌕</span><input data-initial-focus aria-label="Search projects, conversations, and actions" role="combobox" aria-expanded="true" aria-controls="palette_results" aria-autocomplete="list" aria-activedescendant={results[selected] ? 'palette_result_' + selected : undefined} placeholder="Search projects, conversations, or actions…" maxlength={PALETTE_LIMITS.query_characters} bind:value={query} oninput={() => selected = 0} onkeydown={keydown} /><button class="icon_button" aria-label="Close command palette" onclick={on_close}>×</button></div>
  <div id="palette_results" class="palette_results" role="listbox" aria-label="Matching entries">
    {#each results as entry, index (entry.id)}<button id={'palette_result_' + index} role="option" aria-selected={selected === index} class:active={selected === index} disabled={entry.disabled} onpointermove={() => selected = index} onclick={() => select(entry)}><span class="entry_kind">{entry.kind}</span><span><strong>{entry.label}</strong><small>{entry.detail}</small></span><span aria-hidden="true">↵</span></button>{/each}
    {#if !results.length}<p class="palette_empty" role="status">No matching loaded entries.</p>{#if history_search}<button onclick={() => select(history_search)}>Search conversation history</button>{/if}{/if}
  </div>
  <footer class="palette_footer"><span>↑ ↓ to choose · Enter to open · Esc to close</span><span>Loaded conversations and Ask text · Search history for more.</span></footer>
</dialog>

<script lang="ts">
  import { tick, untrack } from 'svelte';
  import { restore_focus } from '../lib/keyboard_shortcuts';
  import { reveal_transcript_element } from '../lib/transcript_scroll';
  import { FIND_LIMITS, find_text_matches, match_excerpt, next_match_index, range_for_match, transcript_text, type TextMatch } from '../lib/transcript_search';
  type Hit = { section: string; text: string; match: TextMatch; range: Range };
  let { root, revision, section_labels = {}, on_close }: { root: HTMLElement | undefined; revision: number; section_labels?: Record<string, string>; on_close: () => void } = $props();
  let query = $state(''), selected = $state(0), truncated = $state(false);
  let hits = $state.raw<Hit[]>([]);
  let previous_query = '';
  const current = $derived(hits[selected]);
  const excerpt = $derived(current ? match_excerpt(current.text, current.match) : null);
  const highlight_api = globalThis as unknown as { Highlight?: new (...ranges: Range[]) => unknown; CSS?: { highlights?: { set(name: string, value: unknown): void; delete(name: string): void } } };

  function clear_highlights() { highlight_api.CSS?.highlights?.delete('native_transcript_find'); highlight_api.CSS?.highlights?.delete('native_transcript_find_current'); }
  function focus_find(node: HTMLInputElement) {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    node.focus();
    return { destroy() { clear_highlights(); restore_focus(previous); } };
  }
  $effect(() => {
    const source = root, needle = query;
    revision;
    let cancelled = false;
    void tick().then(() => {
      if (cancelled || !source) return;
      const found: Hit[] = [];
      let was_truncated = false;
      for (const body of source.querySelectorAll<HTMLElement>('[data-transcript-body]')) {
        const snapshot = transcript_text(body), result = find_text_matches(snapshot.text, needle);
        was_truncated ||= result.truncated;
        for (const match of result.matches) {
          if (found.length >= FIND_LIMITS.matches) { was_truncated = true; break; }
          const range = range_for_match(snapshot.parts, match);
          if (range) found.push({ section: section_labels[body.dataset.transcriptBody ?? ''] ?? (body.dataset.transcriptBody === 'prompt' ? 'Your prompt' : 'Assistant reply'), text: snapshot.text, match, range });
        }
      }
      hits = found; truncated = was_truncated;
      const previous_index = untrack(() => selected);
      selected = needle === previous_query ? Math.max(0, Math.min(previous_index, found.length - 1)) : 0;
      previous_query = needle;
    });
    return () => { cancelled = true; clear_highlights(); };
  });
  $effect(() => {
    if (highlight_api.Highlight && highlight_api.CSS?.highlights) {
      highlight_api.CSS.highlights.set('native_transcript_find', new highlight_api.Highlight(...hits.map((hit) => hit.range)));
      highlight_api.CSS.highlights.set('native_transcript_find_current', new highlight_api.Highlight(...(current ? [current.range] : [])));
    }
    reveal_transcript_element(current?.range.startContainer.parentElement ?? null, current?.range);
  });
  function move(direction: 1 | -1) { selected = Math.max(0, next_match_index(selected, direction, hits.length)); }
  function keydown(event: KeyboardEvent) {
    if (event.isComposing || event.keyCode === 229) return;
    if (event.key === 'Enter') { event.preventDefault(); event.stopPropagation(); move(event.shiftKey ? -1 : 1); }
    else if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); on_close(); }
  }
</script>

<div class="transcript_find" role="search" aria-label="Find in current transcript">
  <div class="find_toolbar"><label><span class="sr_only">Find text in current transcript</span><input use:focus_find placeholder="Find in this transcript…" maxlength={FIND_LIMITS.query_characters} bind:value={query} onkeydown={keydown} /></label><span class="find_count" role="status">{query ? (hits.length ? selected + 1 : 0) + ' / ' + hits.length + (truncated ? '+' : '') : 'Find text'}</span><button aria-label="Previous match" disabled={!hits.length} onclick={() => move(-1)}>↑</button><button aria-label="Next match" disabled={!hits.length} onclick={() => move(1)}>↓</button><button aria-label="Close transcript find" onclick={on_close}>×</button></div>
  {#if excerpt && current}<p class="find_excerpt"><span>{current.section}:</span> {excerpt.before}<mark>{excerpt.match}</mark>{excerpt.after}</p>{:else if query}<p class="find_excerpt">No matches in the displayed prompt or assistant text.</p>{/if}
  {#if truncated}<p class="find_excerpt">Showing the first {FIND_LIMITS.matches} matches. Narrow the search for more precise results.</p>{/if}
</div>

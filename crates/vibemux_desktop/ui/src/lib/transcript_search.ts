export const FIND_LIMITS = { query_characters: 256, matches: 500, excerpt_characters: 70 } as const;
export interface TextMatch { start: number; end: number }

export function find_text_matches(text: string, query: string): { matches: TextMatch[]; truncated: boolean } {
  if (!query) return { matches: [], truncated: false };
  const literal = query.slice(0, FIND_LIMITS.query_characters).replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const matches: TextMatch[] = [];
  for (const match of text.matchAll(new RegExp(literal, 'giu'))) {
    if (matches.length === FIND_LIMITS.matches) return { matches, truncated: true };
    matches.push({ start: match.index, end: match.index + match[0].length });
  }
  return { matches, truncated: false };
}

export function next_match_index(index: number, direction: 1 | -1, count: number): number {
  return count ? (index + direction + count) % count : -1;
}

export function match_excerpt(text: string, match: TextMatch): { before: string; match: string; after: string } {
  return { before: text.slice(Math.max(0, match.start - FIND_LIMITS.excerpt_characters), match.start), match: text.slice(match.start, match.end), after: text.slice(match.end, match.end + FIND_LIMITS.excerpt_characters) };
}

export interface TextPart { node: Text; start: number; end: number }
export function transcript_text(root: HTMLElement): { text: string; parts: TextPart[] } {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, { acceptNode(node) {
    return node.parentElement?.closest('button, .code-toolbar, .code-expand') ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT;
  } });
  let text = '', current: Node | null;
  const parts: TextPart[] = [];
  while ((current = walker.nextNode())) {
    const value = current.textContent ?? '';
    parts.push({ node: current as Text, start: text.length, end: text.length + value.length });
    text += value;
  }
  return { text, parts };
}

export function range_for_match(parts: TextPart[], match: TextMatch): Range | null {
  const start = parts.find((part) => part.start <= match.start && part.end > match.start);
  const end = parts.find((part) => part.start < match.end && part.end >= match.end);
  if (!start || !end) return null;
  const range = document.createRange();
  range.setStart(start.node, match.start - start.start);
  range.setEnd(end.node, match.end - end.start);
  return range;
}

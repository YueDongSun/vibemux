/** Reveal a selected text range or element inside its nearest marked scroll root. */
export function reveal_transcript_element(element: Element | null, selection?: Range): boolean {
  if (!element) return false;
  const disclosures: HTMLDetailsElement[] = [];
  if (element instanceof HTMLElement && element.tagName === 'DETAILS') disclosures.push(element as HTMLDetailsElement);
  let ancestor: Element | null = element.parentElement;
  while (ancestor) {
    if (ancestor instanceof HTMLElement && ancestor.hasAttribute('data-transcript-scroll')) {
      // Find indexes text inside folded cards. Reveal only the selected hit's
      // disclosures within this root, before reading its now-visible geometry.
      for (const disclosure of disclosures.reverse()) disclosure.open = true;
      const target = selection?.getBoundingClientRect() ?? element.getBoundingClientRect();
      const container = ancestor.getBoundingClientRect();
      const viewport_top = container.top + ancestor.clientTop;
      const target_center = target.top + (target.bottom - target.top) / 2;
      const next_top = ancestor.scrollTop + target_center - viewport_top - ancestor.clientHeight / 2;
      const maximum_top = Math.max(0, ancestor.scrollHeight - ancestor.clientHeight);
      ancestor.scrollTo({
        top: Math.max(0, Math.min(maximum_top, next_top)),
        behavior: 'instant',
      });
      return true;
    }
    if (ancestor instanceof HTMLElement && ancestor.tagName === 'DETAILS') disclosures.push(ancestor as HTMLDetailsElement);
    ancestor = ancestor.parentElement;
  }
  return false;
}

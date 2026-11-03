export type ShortcutAction = 'palette' | 'settings' | 'find' | 'help' | 'new_ask' | 'toggle_sidebar' | 'dismiss';
export interface ShortcutEvent {
  key: string; ctrlKey: boolean; metaKey: boolean; altKey: boolean; shiftKey: boolean;
  isComposing: boolean; keyCode: number; repeat: boolean; defaultPrevented: boolean;
}
export const SHORTCUTS = [
  { action: 'new_ask', keys: 'Ctrl / ⌘ + N', label: 'New independent Ask; keep the current draft' },
  { action: 'palette', keys: 'Ctrl / ⌘ + K', label: 'Find projects, retained Asks, and actions' },
  { action: 'find', keys: 'Ctrl / ⌘ + F', label: 'Find in the current transcript' },
  { action: 'settings', keys: 'Ctrl / ⌘ + ,', label: 'Open settings' },
  { action: 'toggle_sidebar', keys: 'Ctrl / ⌘ + B', label: 'Collapse or expand the sidebar' },
  { action: 'help', keys: 'Ctrl / ⌘ + /', label: 'Show keyboard shortcuts' },
  { action: 'dismiss', keys: 'Escape', label: 'Close the current overlay; never cancel execution' },
] as const;

export function shortcut_action(event: ShortcutEvent): ShortcutAction | null {
  if (event.defaultPrevented || event.isComposing || event.keyCode === 229 || event.repeat || event.altKey) return null;
  if (event.key === 'Escape' && !event.ctrlKey && !event.metaKey) return 'dismiss';
  if ((!event.ctrlKey && !event.metaKey) || event.shiftKey) return null;
  const actions: Record<string, ShortcutAction> = { k: 'palette', ',': 'settings', f: 'find', '/': 'help', n: 'new_ask', b: 'toggle_sidebar' };
  return actions[event.key.toLowerCase()] ?? null;
}

export function dispatch_shortcut(event: ShortcutEvent & { preventDefault(): void }, handlers: Partial<Record<ShortcutAction, () => boolean>>): boolean {
  const action = shortcut_action(event);
  if (!action || !handlers[action]?.()) return false;
  event.preventDefault();
  return true;
}

export interface FocusTarget { isConnected: boolean; focus(): void }
export function restore_focus(previous: FocusTarget | null, fallback: FocusTarget | null = null): boolean {
  const target = previous?.isConnected ? previous : fallback?.isConnected ? fallback : null;
  if (!target) return false;
  target.focus();
  return true;
}

/** Native dialog focus containment with restoration to the invoking field. */
export function modal_focus(node: HTMLDialogElement, initial_selector = '[data-initial-focus]') {
  const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  let composing = false;
  const composition_start = () => { composing = true; };
  const composition_end = () => { composing = false; };
  const cancel = (event: Event) => { if (composing) event.preventDefault(); };
  node.addEventListener('compositionstart', composition_start);
  node.addEventListener('compositionend', composition_end);
  node.addEventListener('cancel', cancel, { capture: true });
  node.showModal();
  node.querySelector<HTMLElement>(initial_selector)?.focus();
  return { destroy() {
    node.removeEventListener('compositionstart', composition_start);
    node.removeEventListener('compositionend', composition_end);
    node.removeEventListener('cancel', cancel, { capture: true });
    node.close(); restore_focus(previous);
  } };
}

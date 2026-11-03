export const NATIVE_LIMITS = {
  prompt_bytes: 32 * 1024,
  refresh_ms: 1200,
} as const;

export type Presentation = 'claude' | 'codex';

export function prompt_error(prompt: string): string | null {
  if (!prompt.trim()) return 'Enter a text prompt.';
  if (new TextEncoder().encode(prompt).byteLength > NATIVE_LIMITS.prompt_bytes) {
    return 'Keep the prompt within 32 KiB of UTF-8 text.';
  }
  return null;
}

export function should_send(event: Pick<KeyboardEvent, 'key' | 'isComposing' | 'keyCode' | 'shiftKey' | 'altKey'>): boolean {
  return event.key === 'Enter' && !event.isComposing && event.keyCode !== 229 && !event.shiftKey && !event.altKey;
}

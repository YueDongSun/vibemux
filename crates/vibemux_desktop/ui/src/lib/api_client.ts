import { invoke as tauri_invoke } from '@tauri-apps/api/core';
import { listen as tauri_listen } from '@tauri-apps/api/event';

import type { Backend } from './backend';
import type { CommandMap, CommandName, EventMap, UiEventName } from './ui_contract';

/// The production backend: the fixed Tauri command list over IPC. The
/// webview holds no daemon credential; the Rust side owns it.
export function tauri_backend(): Backend {
  return {
    mode: 'native',
    async invoke<C extends CommandName>(command: C, args: CommandMap[C]['args']): Promise<CommandMap[C]['result']> {
      return tauri_invoke<CommandMap[C]['result']>(command, args);
    },
    async listen<E extends UiEventName>(event: E, handler: (payload: EventMap[E]) => void): Promise<() => void> {
      return tauri_listen<EventMap[E]>(event, (event) => handler(event.payload));
    },
  };
}

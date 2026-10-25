import { invoke as tauri_invoke } from '@tauri-apps/api/core';
import { listen as tauri_listen } from '@tauri-apps/api/event';

import type { Backend } from './backend';
import type { CommandName, UiEventName } from './ui_contract';

/// The production backend: the fixed Tauri command list over IPC. The
/// webview holds no daemon credential; the Rust side owns it.
export function tauri_backend(): Backend {
  return {
    async invoke<T>(command: CommandName, args?: Record<string, unknown>): Promise<T> {
      return tauri_invoke<T>(command, args);
    },
    async listen(event: UiEventName, handler: (payload: unknown) => void): Promise<() => void> {
      return tauri_listen(event, (event) => handler(event.payload));
    },
  };
}

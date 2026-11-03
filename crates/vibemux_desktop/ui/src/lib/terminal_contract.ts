/** Path-free terminal observations; native tokens bind actions to reviewed data. */
export interface TerminalCandidate {
  plugin_id: string;
  pane_id: string;
  instance_id: string;
  display_name: string;
}
export interface TerminalView {
  status: string;
  binding_id: string | null;
  plugin_id: string | null;
  pane_id: string | null;
  instance_id: string | null;
  checked_at: string | null;
  candidates: TerminalCandidate[];
}
export type TerminalRequest =
  | { operation: 'inspect'; previous_observation_id?: string }
  | { operation: 'link'; observation_id: string; pane_id: string; instance_id: string }
  | { operation: 'focus'; observation_id: string }
  | { operation: 'unlink'; observation_id: string };
export type TerminalOutcome = 'observed' | 'linked' | 'focused' | 'unlinked';
export interface TerminalReply {
  project_id: string;
  task_id: string;
  run_id: string;
  observation_id: string;
  terminal: TerminalView;
  outcome: TerminalOutcome;
}

import { BridgeError } from './errors.mjs';

export function advertisedCommands(help) {
  const commands = new Set();
  for (const line of help.split(/\r?\n/)) {
    const match = /^\s*vibemuxctl\s+(workflow|tasks|daemon|slots)\s+([a-z][a-z0-9_-]*(?:\|[a-z][a-z0-9_-]*)*)(?=\s|$)/.exec(line);
    if (match) {
      for (const verb of match[2].split('|')) commands.add(match[1] + '.' + verb);
    }
  }
  return commands;
}

export class Capabilities {
  constructor(transport) {
    this.transport = transport;
    this.last = null;
    this.pending = null;
  }

  async get() {
    if (this.last && Date.now() - this.last.time < 5000) return this.last;
    if (!this.pending) {
      this.pending = this.transport.run(['--format', 'text', 'workflow', '--help'],
        { text: true, timeoutMs: 10000 })
        .then((help) => {
          const result = {
            commands: advertisedCommands(help), time: Date.now(), observed_at: new Date().toISOString(),
          };
          this.last = result;
          return result;
        }).finally(() => { this.pending = null; });
    }
    return this.pending;
  }

  async require(command) {
    const observed = await this.get();
    if (!observed.commands.has(command)) throw new BridgeError('operation_unsupported', 501);
  }

  async view(config, project) {
    const observed = await this.get();
    const has = (command) => observed.commands.has(command);
    return {
      ok: true, project_id: project.id, source: 'cli_help',
      observed_at: observed.observed_at,
      // Advertisement is independent of daemon reachability and live qualification.
      daemon_qualification: 'unverified',
      capabilities: {
        tasks: has('tasks.list'), task_detail: has('tasks.show'),
        workflow_status: has('workflow.status'),
        workflow_index: has('workflow.list'),
        goals: config.enableWrites && has('workflow.submit'),
        operation_lookup: has('workflow.operation'),
        runtime_readiness: has('workflow.readiness'),
        // Historical worker reports are not a canonical unresolved inbox.
        current_attention: false,
        intake: config.enableWrites && has('workflow.intake'),
        followup: config.enableWrites && has('workflow.followup'),
        reports: has('workflow.reports'),
        pause: config.enableWrites && has('workflow.pause'),
        cancel: config.enableWrites && has('workflow.cancel'),
      },
      server_config: {
        allow_import: project.allowImport,
        allow_report_content: project.allowReportContent,
        presets: project.presets.map((preset) => ({ id: preset.id, label: preset.label ?? preset.id })),
      },
    };
  }
}

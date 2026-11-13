import { BridgeError, object, safeInteger } from './errors.mjs';

const CODE = /^[a-z0-9_]{1,96}$/;
const READ_TIMEOUT_MS = 5000;

function errorCode(error) {
  return error instanceof BridgeError && CODE.test(error.code)
    ? error.code : 'readiness_unavailable';
}

function invalid() { throw new BridgeError('cli_invalid_output', 502, 'rejected'); }

function daemonView(value) {
  if (value.ok !== true || typeof value.healthy !== 'boolean'
    || !safeInteger(value.store_schema_version)
    || !safeInteger(value.queue_depth)
    || typeof value.queue_saturated !== 'boolean') invalid();
  return {
    reachable: true, healthy: value.healthy, store_schema_version: value.store_schema_version,
    queue_depth: value.queue_depth, queue_saturated: value.queue_saturated, code: null,
  };
}

function workflowView(value) {
  const readiness = value.readiness;
  if (value.ok !== true || !object(readiness) || readiness.schema_version !== 1
    || typeof readiness.configured !== 'boolean'
    || typeof readiness.compiler_available !== 'boolean'
    || typeof readiness.start_ready !== 'boolean'
    || !Array.isArray(readiness.blockers) || readiness.blockers.length > 64
    || !readiness.blockers.every((code) => typeof code === 'string' && CODE.test(code))) invalid();
  return {
    supported: true, source: 'daemon_readiness',
    configured: readiness.configured, compiler_available: readiness.compiler_available,
    start_ready: readiness.start_ready, blockers: readiness.blockers,
  };
}

export class Readiness {
  constructor(transport, capabilities) {
    this.transport = transport;
    this.capabilities = capabilities;
    // Coalesce concurrent observations only; no durable task/operation state.
    this.pending = new Map();
  }

  get(project) {
    if (!this.pending.has(project.id)) {
      this.pending.set(project.id, this.observe(project).finally(() => {
        this.pending.delete(project.id);
      }));
    }
    return this.pending.get(project.id);
  }

  async observe(project) {
    const result = {
      ok: true, project_id: project.id, schema_version: 1, observed_at: null,
      cli: { available: false, code: null },
      daemon: { reachable: false, healthy: null, store_schema_version: null,
        queue_depth: null, queue_saturated: null, code: null },
      workflow: { supported: false, source: 'unavailable', configured: null,
        compiler_available: null, start_ready: null, blockers: [] },
    };
    let commands;
    try {
      commands = (await this.capabilities.get()).commands;
      result.cli.available = true;
    } catch (error) {
      const code = errorCode(error);
      result.cli.code = code;
      result.daemon.code = code;
      result.workflow.blockers = [code];
      result.observed_at = new Date().toISOString();
      return result;
    }
    const command = (args) => this.transport.command(project, args, { timeoutMs: READ_TIMEOUT_MS });
    const daemon = async () => {
      try {
        if (!commands.has('daemon.health')) throw new BridgeError('operation_unsupported', 501);
        result.daemon = daemonView(await command(['daemon', 'health']));
      } catch (error) {
        result.daemon.code = errorCode(error);
      }
    };
    const workflow = async () => {
      const supported = commands.has('workflow.readiness');
      result.workflow.supported = supported;
      try {
        if (supported) {
          result.workflow = workflowView(await command(['workflow', 'readiness']));
        } else if (commands.has('slots.list')) {
          const value = await command(['slots', 'list']);
          if (!object(value.slots) || !Array.isArray(value.slots.slots)) invalid();
          result.workflow.source = 'legacy_slots';
          result.workflow.configured = true;
          // Slots/configuration existence cannot certify a live compiler or start.
          result.workflow.blockers = ['runtime_readiness_unsupported'];
        } else {
          result.workflow.blockers = ['runtime_readiness_unsupported'];
        }
      } catch (error) {
        const code = errorCode(error);
        if (code === 'workflow_unconfigured') result.workflow.configured = false;
        result.workflow.blockers = [code];
      }
    };
    // A configuration may intentionally have one CLI slot. These two probes
    // must not reject each other as bridge_busy within one readiness request.
    await daemon();
    await workflow();
    result.observed_at = new Date().toISOString();
    return result;
  }
}

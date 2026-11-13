import { spawn } from 'node:child_process';
import { mkdtemp, chmod, open, unlink, rmdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { BridgeError, assert, object } from './errors.mjs';
import { MAX_REQUEST_BYTES } from './config.mjs';

const CODE = /^[a-z0-9_]{1,96}$/;
const ERROR_HTTP_STATUS = new Map([
  ['workflow_operation_not_found', 404],
  ['workflow_operation_conflict', 409],
  ['workflow_operation_capacity', 429],
  ['workflow_operation_invalid', 400],
  ['cli_invalid_arguments', 501],
]);
const DEFINITE_REFUSALS = new Set([
  'cli_invalid_arguments', 'workflow_request_invalid', 'workflow_followup_invalid',
  'workflow_contract_mismatch', 'control_invalid_request', 'control_unsupported_version',
  'workflow_not_found', 'workflow_unconfigured', 'gateway_live_attestation_unavailable',
  'workflow_request_conflict', 'workflow_policy_invalid', 'workflow_compiler_unavailable',
  'workflow_contract_rejected', 'workflow_slot_unavailable', 'workflow_config_invalid',
  'workflow_untrusted_executable', 'workflow_verifier_changed', 'workflow_invalid_phase',
  'workflow_operation_conflict', 'workflow_operation_capacity', 'workflow_operation_invalid',
  'store_workflow_version_mismatch', 'store_workflow_followup_invalid',
  'store_workflow_contract_mismatch', 'store_workflow_idempotency_conflict',
]);

const CHILD_ENVIRONMENT = new Set([
  'PATH', 'SYSTEMROOT', 'WINDIR', 'TEMP', 'TMP', 'TMPDIR', 'HOME', 'USERPROFILE',
  'LOCALAPPDATA', 'APPDATA', 'COMSPEC', 'PATHEXT', 'LANG', 'LC_ALL', 'LC_CTYPE', 'TZ',
  'XDG_RUNTIME_DIR',
]);

export function childEnvironment(config, source = process.env) {
  // The CLI talks to a local daemon. Provider credentials and the HTTP bearer
  // token have no purpose in this child; keep only operating-system essentials.
  return Object.fromEntries(Object.entries(source).filter(([key]) =>
    CHILD_ENVIRONMENT.has(key.toUpperCase())
      && key.toUpperCase() !== config.tokenEnv?.toUpperCase()));
}

function parsed(text) {
  try {
    const value = JSON.parse(text);
    return object(value) ? value : null;
  } catch {
    return null;
  }
}

function errorFromResult(result, mutation) {
  const payload = parsed(result.stderr) ?? parsed(result.stdout);
  const code = payload?.ok === false && CODE.test(payload.error_code)
    ? payload.error_code : 'cli_failed';
  const outcome = mutation && !DEFINITE_REFUSALS.has(code) ? 'unknown' : 'rejected';
  const status = ERROR_HTTP_STATUS.get(code) ?? 502;
  return new BridgeError(code, status, outcome);
}

export class CliTransport {
  constructor(config, { prefix = [], env = childEnvironment(config) } = {}) {
    this.config = config;
    // Dependency injection for fixture executables; loadConfig has no prefix field.
    this.prefix = Object.freeze([...prefix]);
    this.env = env;
    this.active = new Set();
    this.closing = false;
  }

  async run(args, { mutation = false, text = false, timeoutMs } = {}) {
    if (this.closing) throw new BridgeError('bridge_stopping', 503);
    if (this.active.size >= this.config.maxConcurrency) throw new BridgeError('bridge_busy', 429);
    assert(Array.isArray(args) && args.every((arg) => typeof arg === 'string'));
    const capture = {
      child: null, done: null, resolve: null, closed: false,
      stop: null,
    };
    capture.done = new Promise((resolve) => { capture.resolve = resolve; });
    this.active.add(capture);
    try {
      const result = await new Promise((resolve, reject) => {
        let spawned = false;
        let total = 0;
        const stdout = [];
        const stderr = [];
        let stopError = null;
        let forceTimer;
        let finishTimer;
        let settled = false;
        const finish = (error, value) => {
          if (settled) return;
          settled = true;
          clearTimeout(timer);
          clearTimeout(forceTimer);
          clearTimeout(finishTimer);
          if (error) reject(error); else resolve(value);
        };
        const stop = (code, status) => {
          if (stopError) return;
          stopError = new BridgeError(code, status, mutation && spawned ? 'unknown' : 'not_started');
          child.kill('SIGTERM');
          forceTimer = setTimeout(() => child.kill('SIGKILL'), 250);
          // Only this short-lived CLI child is owned here. The daemon and its
          // workflow continue under their own owner, even if the caller leaves.
          finishTimer = setTimeout(() => {
            child.stdout.destroy();
            child.stderr.destroy();
            finish(stopError);
          }, 1250);
        };
        const child = spawn(this.config.executable, [...this.prefix, ...args], {
          shell: false, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'],
          env: this.env,
        });
        capture.child = child;
        capture.stop = () => stop('bridge_stopping', 503);
        const timer = setTimeout(
          () => stop('cli_timeout', 504),
          timeoutMs ?? this.config.commandTimeoutMs,
        );
        const consume = (chunks) => (chunk) => {
          total += chunk.length;
          if (total > this.config.maxOutputBytes) stop('cli_output_limit', 502);
          else if (!stopError) chunks.push(chunk);
        };
        child.stdout.on('data', consume(stdout));
        child.stderr.on('data', consume(stderr));
        child.once('spawn', () => { spawned = true; });
        child.once('error', () => finish(new BridgeError(
          'cli_unavailable', 503, mutation && spawned ? 'unknown' : 'not_started',
        )));
        child.once('close', (code, signal) => {
          capture.closed = true;
          this.active.delete(capture);
          capture.resolve();
          if (stopError) return finish(stopError);
          try {
            const decode = (parts) => new TextDecoder('utf-8', { fatal: true }).decode(Buffer.concat(parts));
            finish(null, { code, signal, stdout: decode(stdout), stderr: decode(stderr) });
          } catch {
            finish(new BridgeError('cli_invalid_output', 502, mutation ? 'unknown' : 'rejected'));
          }
        });
      });
      if (result.code !== 0 || result.signal) throw errorFromResult(result, mutation);
      if (text) return result.stdout;
      const value = parsed(result.stdout);
      if (!value) throw new BridgeError('cli_invalid_output', 502, mutation ? 'unknown' : 'rejected');
      if (value.ok !== true) throw errorFromResult(result, mutation);
      return value;
    } finally {
      // A bounded HTTP failure does not prove that the OS child exited. Keep
      // its ownership slot until close, even after an unsuccessful kill.
      if (!capture.child) {
        this.active.delete(capture);
        capture.resolve();
      }
    }
  }

  command(project, args, options = {}) {
    return this.run(['--format', 'json', ...args, '--project-root', project.root], options);
  }

  async document(project, verb, document) {
    const encoded = Buffer.from(JSON.stringify(document), 'utf8');
    assert(encoded.length <= MAX_REQUEST_BYTES, 'body_too_large', 413);
    const directory = await mkdtemp(path.join(tmpdir(), 'vibemux-bridge-'));
    const filename = path.join(directory, 'request.json');
    let handle;
    let attempted = false;
    let commandError;
    try {
      // On Windows, the user's temporary-directory ACL is the privacy boundary;
      // POSIX mode bits supplement that boundary on Unix.
      await chmod(directory, 0o700);
      handle = await open(filename, 'wx', 0o600);
      await handle.writeFile(encoded);
      await handle.close();
      handle = undefined;
      attempted = true;
      return await this.command(project, ['workflow', verb, '--request-file', filename], { mutation: true });
    } catch (error) {
      commandError = error;
      throw error;
    } finally {
      try {
        await handle?.close();
        // Exact task-owned paths only. No recursive cleanup or following links.
        await unlink(filename).catch((error) => { if (error.code !== 'ENOENT') throw error; });
        await rmdir(directory);
      } catch {
        // Never turn an acknowledged/possibly applied mutation into "not started"
        // because local cleanup failed. Preserve an existing command outcome.
        if (!commandError) throw new BridgeError('private_file_cleanup_failed', 500,
          attempted ? 'unknown' : 'not_started');
      }
    }
  }

  async close() {
    this.closing = true;
    const active = [...this.active];
    for (const command of active) command.stop?.();
    let timer;
    try {
      await Promise.race([
        Promise.all(active.map((command) => command.done)),
        new Promise((_resolve, reject) => {
          timer = setTimeout(() => reject(new BridgeError('cli_cleanup_incomplete', 503, 'unknown')), 2000);
        }),
      ]);
    } finally {
      clearTimeout(timer);
    }
  }
}

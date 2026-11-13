import { lstat, realpath, open } from 'node:fs/promises';
import path from 'node:path';
import { BridgeError, assert, exactKeys, object, safeInteger } from './errors.mjs';

export const CONFIG_BYTES = 64 * 1024;
export const MAX_REQUEST_BYTES = 56 * 1024;
export const MAX_CONFIG_PROJECTS = 32;
const ALIAS = /^[a-z][a-z0-9_-]{0,47}$/;

async function regularPath(candidate, kind) {
  assert(typeof candidate === 'string' && path.isAbsolute(candidate), 'invalid_configuration');
  const resolved = await realpath(candidate).catch(() => {
    throw new BridgeError('invalid_configuration');
  });
  const stat = await lstat(resolved);
  assert(kind === 'directory' ? stat.isDirectory() : stat.isFile(), 'invalid_configuration');
  return resolved;
}

export async function validateConfig(input, env = process.env) {
  try {
    exactKeys(input, [
      'schema_version', 'executable', 'port', 'token_env', 'enable_writes',
      'allowed_origins', 'projects', 'command_timeout_ms', 'max_output_bytes',
      'max_concurrency',
    ], ['schema_version', 'executable', 'projects']);
    assert(input.schema_version === 1, 'invalid_configuration');
    const executable = await regularPath(input.executable, 'file');
    assert(!/\.(?:cmd|bat|ps1)$/i.test(executable), 'invalid_configuration');
    if (process.platform === 'win32') assert(/\.exe$/i.test(executable), 'invalid_configuration');
    const port = input.port ?? 4317;
    assert(safeInteger(port, 1, 65535), 'invalid_configuration');
    const tokenEnv = input.token_env ?? 'VIBEMUX_BRIDGE_TOKEN';
    assert(/^[A-Z][A-Z0-9_]{0,79}$/.test(tokenEnv), 'invalid_configuration');
    const token = env[tokenEnv];
    assert(typeof token === 'string' && /^[!-~]{32,256}$/.test(token), 'invalid_configuration');
    assert(input.enable_writes === undefined || typeof input.enable_writes === 'boolean', 'invalid_configuration');
    const commandTimeoutMs = input.command_timeout_ms ?? 125000;
    const maxOutputBytes = input.max_output_bytes ?? 512 * 1024;
    const maxConcurrency = input.max_concurrency ?? 4;
    assert(safeInteger(commandTimeoutMs, 100, 180000), 'invalid_configuration');
    assert(safeInteger(maxOutputBytes, 1024, 2 * 1024 * 1024), 'invalid_configuration');
    assert(safeInteger(maxConcurrency, 1, 8), 'invalid_configuration');
    const allowedOrigins = input.allowed_origins ?? [
      'http://127.0.0.1:' + port, 'http://localhost:' + port,
    ];
    assert(Array.isArray(allowedOrigins) && allowedOrigins.length <= 16, 'invalid_configuration');
    for (const origin of allowedOrigins) {
      const url = new URL(origin);
      assert(['http:', 'https:'].includes(url.protocol)
        && url.origin === origin && !url.username && !url.password, 'invalid_configuration');
    }
    assert(Array.isArray(input.projects)
      && input.projects.length > 0
      && input.projects.length <= MAX_CONFIG_PROJECTS, 'invalid_configuration');
    const ids = new Set();
    const projects = [];
    for (const project of input.projects) {
      exactKeys(project, [
        'id', 'label', 'root', 'allow_import', 'allow_report_content', 'presets',
      ], ['id', 'root']);
      assert(ALIAS.test(project.id) && !ids.has(project.id), 'invalid_configuration');
      ids.add(project.id);
      const label = project.label ?? project.id;
      assert(typeof label === 'string' && label.length <= 100
        && !/[\u0000-\u001f]/.test(label), 'invalid_configuration');
      for (const name of ['allow_import', 'allow_report_content']) {
        assert(project[name] === undefined || typeof project[name] === 'boolean', 'invalid_configuration');
      }
      const presets = project.presets ?? [];
      assert(Array.isArray(presets) && presets.length <= 16, 'invalid_configuration');
      const presetIds = new Set();
      for (const preset of presets) {
        exactKeys(preset, ['id', 'label', 'request', 'policy'], ['id', 'request', 'policy']);
        assert(ALIAS.test(preset.id) && !presetIds.has(preset.id), 'invalid_configuration');
        presetIds.add(preset.id);
        assert(typeof (preset.label ?? preset.id) === 'string'
          && (preset.label ?? preset.id).length <= 100, 'invalid_configuration');
        assert(object(preset.request) && object(preset.policy), 'invalid_configuration');
        // Goal injection is only request_text. TaskSpecs with quoted source spans
        // cannot safely be reused with a new goal, so presets require a compiler.
        assert(preset.request.task_specs === undefined
          || (Array.isArray(preset.request.task_specs) && preset.request.task_specs.length === 0),
        'invalid_configuration');
        assert(Buffer.byteLength(JSON.stringify(preset)) <= MAX_REQUEST_BYTES, 'invalid_configuration');
      }
      projects.push(Object.freeze({
        id: project.id, label, root: await regularPath(project.root, 'directory'),
        allowImport: project.allow_import === true,
        allowReportContent: project.allow_report_content === true,
        presets: structuredClone(presets),
      }));
    }
    return Object.freeze({
      executable, port, token, tokenEnv, enableWrites: input.enable_writes === true,
      allowedOrigins: Object.freeze([...allowedOrigins]), projects: Object.freeze(projects),
      commandTimeoutMs, maxOutputBytes, maxConcurrency,
    });
  } catch (error) {
    if (error instanceof BridgeError && error.code === 'invalid_configuration') throw error;
    throw new BridgeError('invalid_configuration');
  }
}

export async function loadConfig(filename, env = process.env) {
  let handle;
  try {
    const before = await lstat(filename);
    assert(before.isFile() && !before.isSymbolicLink() && before.size <= CONFIG_BYTES, 'invalid_configuration');
    handle = await open(filename, 'r');
    const after = await handle.stat();
    assert(after.isFile() && after.dev === before.dev && after.ino === before.ino
      && after.size <= CONFIG_BYTES, 'invalid_configuration');
    const buffer = Buffer.alloc(CONFIG_BYTES + 1);
    const { bytesRead } = await handle.read(buffer, 0, buffer.length, 0);
    assert(bytesRead <= CONFIG_BYTES, 'invalid_configuration');
    return await validateConfig(JSON.parse(new TextDecoder('utf-8', { fatal: true })
      .decode(buffer.subarray(0, bytesRead))), env);
  } catch {
    throw new BridgeError('invalid_configuration');
  } finally {
    await handle?.close();
  }
}

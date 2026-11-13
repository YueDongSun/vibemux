import { mkdtemp, readFile, readdir, unlink, rmdir } from 'node:fs/promises';
import { randomBytes } from 'node:crypto';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import http from 'node:http';
import { CliTransport } from '../src/cli.mjs';
import { createBridgeServer } from '../src/server.mjs';
import { CONTRACT } from './fixtures/data.mjs';

export async function harness(test, scenario = 'superagent', overrides = {}) {
  const directory = await mkdtemp(path.join(tmpdir(), 'vibemux-bridge-test-'));
  const project = {
    id: 'lab', label: 'Synthetic lab', root: directory, allowImport: true,
    allowReportContent: false, presets: [],
  };
  const config = {
    executable: process.execPath, port: 4317, token: randomBytes(32).toString('hex'),
    tokenEnv: 'VIBEMUX_BRIDGE_TOKEN', enableWrites: true,
    allowedOrigins: ['http://localhost:4317'],
    projects: [project, { ...project, id: 'other', label: 'Other synthetic project' }],
    commandTimeoutMs: 2000, maxOutputBytes: 128 * 1024, maxConcurrency: 4,
    ...overrides,
  };
  const fixture = fileURLToPath(new URL('./fixtures/cli.mjs', import.meta.url));
  const transport = new CliTransport(config, { prefix: [fixture, scenario, directory] });
  const bridge = createBridgeServer(config, { transport });
  await new Promise((resolve, reject) => {
    bridge.server.once('error', reject);
    bridge.server.listen(0, '127.0.0.1', resolve);
  });
  const base = 'http://127.0.0.1:' + bridge.server.address().port;
  test.after(async () => {
    await bridge.close();
    for (const file of await readdir(directory)) await unlink(path.join(directory, file));
    await rmdir(directory);
  });
  return {
    directory, config, bridge, transport, base, project,
    async request(route, { method = 'GET', body, headers = {}, auth = true } = {}) {
      const response = await fetch(base + '/api/v1' + route, {
        method, headers: {
          ...(auth ? { authorization: 'Bearer ' + config.token } : {}),
          ...(body !== undefined ? { 'content-type': 'application/json' } : {}),
          ...headers,
        },
        ...(body !== undefined ? { body: typeof body === 'string' ? body : JSON.stringify(body) } : {}),
      });
      return { status: response.status, headers: response.headers, body: await response.json() };
    },
    async calls() {
      try { return (await readFile(path.join(directory, 'calls.jsonl'), 'utf8'))
        .trim().split('\n').filter(Boolean).map((line) => JSON.parse(line)); }
      catch (error) { if (error.code === 'ENOENT') return []; throw error; }
    },
    async rawRequest(headers) {
      return new Promise((resolve, reject) => {
        const request = http.request(base + '/api/v1/projects', { headers }, (response) => {
          const chunks = [];
          response.on('data', (chunk) => chunks.push(chunk));
          response.once('error', reject);
          response.once('end', () => resolve({
            status: response.statusCode, body: JSON.parse(Buffer.concat(chunks).toString('utf8')),
          }));
        });
        request.once('error', reject);
        request.end();
      });
    },
  };
}

export const goalPreset = {
  id: 'compiler_fixture', label: 'Synthetic compiler fixture',
  request: { schema_version: 1, request_text: '', task_specs: [], verifier_digest: CONTRACT },
  policy: { schema_version: 1 },
};

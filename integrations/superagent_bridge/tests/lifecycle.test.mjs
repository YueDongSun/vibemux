import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, writeFile, unlink, rmdir } from 'node:fs/promises';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

for (const mode of ['shutdown', 'disconnect']) {
  test('real bridge parent IPC ' + mode + ' closes only its own server', async (t) => {
    const directory = await mkdtemp(path.join(tmpdir(), 'vibemux-bridge-ipc-'));
    const listener = createServer();
    await new Promise((resolve) => listener.listen(0, '127.0.0.1', resolve));
    const port = listener.address().port;
    await new Promise((resolve) => listener.close(resolve));
    const token = 'synthetic-bridge-ipc-test-token-only';
    const filename = path.join(directory, 'config.json');
    await writeFile(filename, JSON.stringify({
      schema_version: 1, executable: process.execPath, port,
      projects: [{ id: 'example', root: directory }],
    }), { mode: 0o600 });
    const child = spawn(process.execPath, [
      fileURLToPath(new URL('../src/main.mjs', import.meta.url)), '--config', filename,
    ], { shell: false, env: { ...process.env, VIBEMUX_BRIDGE_TOKEN: token },
      stdio: ['ignore', 'pipe', 'pipe', 'ipc'] });
    child.stdout.resume();
    child.stderr.resume();
    t.after(async () => {
      if (child.exitCode === null && child.signalCode === null) {
        child.kill('SIGKILL');
        await once(child, 'exit').catch(() => {});
      }
      await unlink(filename);
      await rmdir(directory);
    });
    const [ready] = await once(child, 'message', { signal: AbortSignal.timeout(5000) });
    assert.deepEqual(ready, { type: 'superagent:ready', pid: child.pid, port });
    const response = await fetch(`http://127.0.0.1:${port}/api/v1/projects`, {
      headers: { authorization: 'Bearer ' + token },
    });
    assert.equal(response.status, 200);
    assert.equal((await response.json()).projects[0].id, 'example');
    const ended = once(child, 'exit', { signal: AbortSignal.timeout(5000) });
    if (mode === 'shutdown') child.send({ type: 'superagent:shutdown' });
    else child.disconnect();
    const [code, signal] = await ended;
    assert.equal(code, 0);
    assert.equal(signal, null);
  });
}

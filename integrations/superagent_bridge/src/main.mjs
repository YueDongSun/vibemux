import { loadConfig } from './config.mjs';
import { createBridgeServer } from './server.mjs';
import { BridgeError } from './errors.mjs';

async function main() {
  const args = process.argv.slice(2);
  if (args.length !== 2 || args[0] !== '--config') {
    process.stderr.write('Usage: node src/main.mjs --config <private-config.json>\n');
    process.exitCode = 2;
    return;
  }
  const config = await loadConfig(args[1]);
  const bridge = createBridgeServer(config);
  await new Promise((resolve, reject) => {
    bridge.server.once('error', reject);
    bridge.server.listen(config.port, '127.0.0.1', resolve);
  });
  process.stdout.write('VibeMux supervisor bridge listening on loopback port ' + config.port + '.\n');
  let closing;
  const close = () => {
    closing ??= bridge.close().catch(() => { process.exitCode = 1; });
  };
  process.once('SIGINT', close);
  process.once('SIGTERM', close);
}

main().catch((error) => {
  const code = error instanceof BridgeError ? error.code : 'bridge_start_failed';
  process.stderr.write(code + '\n');
  process.exitCode = 1;
});

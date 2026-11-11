import { readFileSync } from 'node:fs';
import { createRequire, registerHooks } from 'node:module';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { compile } from 'svelte/compiler';

const local_require = createRequire(new URL('../../package.json', import.meta.url));
const renderer_packages = /^(?:svelte|@lucide\/svelte|dompurify|marked|highlight\.js)(?:\/|$)/;

// No server or browser process. Compile the actual components for Svelte's
// server renderer; resolve their packages from the desktop's independent install.
export function install_svelte_ssr() {
  return registerHooks({
    resolve(specifier, context, next) {
      if (renderer_packages.test(specifier)) {
        return { url: pathToFileURL(local_require.resolve(specifier)).href, shortCircuit: true };
      }
      return next(specifier, context);
    },
    load(url, context, next) {
      if (url.endsWith('.svelte')) {
        const filename = fileURLToPath(url);
        const { js } = compile(readFileSync(filename, 'utf8'), { filename, generate: 'server' });
        return {
          format: 'module', shortCircuit: true,
          source: js.code.replaceAll('import.meta.env.MODE', '"test"'),
        };
      }
      return next(url, context);
    },
  });
}

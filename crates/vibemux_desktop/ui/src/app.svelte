<script lang="ts">
  import { onMount } from 'svelte';
  import Workspace from '@chat/app.svelte';
  import { load_backend, type Backend } from './lib/backend';
  import { create_native_host, type NativeHost } from './lib/native_host';

  let host = $state<NativeHost | null>(null);
  onMount(() => {
    let disposed = false;
    void load_backend().then((backend) => {
      if (!disposed) host = create_native_host(backend);
    }).catch(() => {
      // Even if IPC cannot load, render the real workspace in native-only mode.
      // Never fall back to the browser provider or claim a ready connection.
      const unavailable: Backend = {
        async invoke() { throw { code: 'native_backend_unavailable', message: 'Native commands are unavailable.' }; },
        async listen() { return () => undefined; },
      };
      if (!disposed) host = create_native_host(unavailable);
    });
    return () => { disposed = true; };
  });
</script>

{#if host}
  <Workspace {host} />
{:else}
  <main class="native-loading" role="status">Loading the VibeMux workspace…</main>
{/if}

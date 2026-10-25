<script lang="ts">
  import { onMount } from 'svelte';

  import { load_backend, type Backend } from './lib/backend';

  let backend = $state<Backend | null>(null);
  let backend_error = $state('');

  onMount(() => {
    void load_backend()
      .then((loaded) => {
        backend = loaded;
      })
      .catch((error: unknown) => {
        backend_error = error instanceof Error ? error.message : String(error);
      });
  });
</script>

<div class="shell">
  <aside class="sidebar">
    <div class="brand">VibeMux</div>
    <p class="sidebar_note">
      Projects, chats, tasks, and the console appear here.
    </p>
  </aside>

  <main class="main">
    <header class="top_bar">
      <span class="breadcrumb">VibeMux</span>
      <span class="connection_dot" title={backend_error ? backend_error : 'Backend ready'}></span>
    </header>

    <section class="empty_state">
      <h1 class="greeting">VibeMux</h1>
      <p class="empty_note">
        The desktop shell is running. Its screens arrive in the next steps.
      </p>
    </section>
  </main>
</div>

<script lang="ts">
  import { onMount } from 'svelte';
  import MarkdownView from '../../../../vibemux_chat/web/src/components/markdown_view.svelte';
  import { load_backend } from '../lib/backend';
  import { NativeLinks } from '../lib/native_links';
  import type { CodeArtifact } from './native_transcript.svelte';

  let { content = '', notify = () => {}, on_artifact = () => {} }: {
    content?: string; notify?: (notice: string) => void; on_artifact?: (artifact: CodeArtifact) => void;
  } = $props();
  let links = $state<NativeLinks | null>(null);
  let enabled = $state(false);
  onMount(() => {
    let disposed = false;
    void load_backend().then((backend) => {
      if (disposed) return;
      links = new NativeLinks((request) => backend.invoke('open_link', { request }), (notice) => notify(notice), backend.mode === 'fixture');
      enabled = backend.mode === 'native';
    }).catch(() => { if (!disposed) notify('Link actions are unavailable.'); });
    return () => { disposed = true; links?.dispose(); };
  });
</script>

<MarkdownView {content} {notify} {on_artifact} links_enabled={enabled} on_link={(url) => void links?.open(url)} />

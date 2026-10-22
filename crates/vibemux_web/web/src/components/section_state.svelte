<script lang="ts">
  import type { console_section } from '../api_client';
  import { section_notice } from '../console_policy';

  let { section }: { section: console_section<unknown> | null | undefined } = $props();
  const notice = $derived(section_notice(section));
</script>

{#if notice}
  <p class="section_notice" role="status">
    <strong>{notice.title}</strong>
    {#if notice.detail}<span class="mono">{notice.detail}</span>{/if}
  </p>
{/if}

<script>
  import DOMPurify from "dompurify";
  import { render_markdown, markdown_sanitize_options } from "../services/markdown_renderer.js";
  let { host, content = "", on_artifact = () => {}, notify = () => {} } = $props();
  const rendered = $derived.by(() => {
    const result = render_markdown(content, host.capabilities);
    return { ...result, html: DOMPurify.sanitize(result.html, markdown_sanitize_options(host.capabilities)) };
  });
  async function handle_click(event) {
    const target = event.target.closest("button[data-action]");
    if (!target || !event.currentTarget.contains(target)) return;
    const block = rendered.blocks[Number(target.dataset.code)];
    if (!block) return;
    if (target.dataset.action === "copy") {
      try {
        await navigator.clipboard.writeText(block.content);
        notify("Code copied");
      } catch {
        notify("Clipboard unavailable. Select the code to copy it.");
      }
    } else if (target.dataset.action === "artifact") on_artifact(block);
    else {
      const pre = target.closest(".code-block").querySelector("pre");
      const collapsed = pre.classList.toggle("code-collapsed");
      target.textContent = collapsed
        ? `Expand all ${block.content.split("\n").length} lines`
        : "Collapse code";
    }
  }
</script>

<div class="markdown" onclick={handle_click} role="presentation">
  {@html rendered.html}
</div>

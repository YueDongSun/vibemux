<script lang="ts">
  import DOMPurify from "dompurify";
  import { render_markdown, type CodeArtifact } from "../markdown_presentation";
  let { content = "", on_artifact = () => {}, notify = () => {}, links_enabled = true, on_link }: {
    content?: string; on_artifact?: (artifact: CodeArtifact) => void;
    notify?: (message: string) => void; links_enabled?: boolean; on_link?: (url: string) => void;
  } = $props();
  const rendered = $derived.by(() => {
    const result = render_markdown(content, links_enabled);
    return { blocks: result.blocks, html: DOMPurify.sanitize(result.html, {
      ADD_ATTR: ["target", "rel", "data-code", "data-action"],
      ADD_TAGS: ["button"],
      FORBID_TAGS: ["style", "iframe", "form", "input"],
      FORBID_ATTR: ["style"],
    }) };
  });
  async function handle_click(event: MouseEvent) {
    if (!(event.target instanceof Element) || !(event.currentTarget instanceof Element)) return;
    const anchor = event.target.closest<HTMLAnchorElement>("a");
    if (anchor && event.currentTarget.contains(anchor)) {
      if (!links_enabled || on_link) event.preventDefault();
      if (links_enabled && on_link && event.button === 0) on_link(anchor.getAttribute("href") ?? "");
      return;
    }
    if (event.type === "auxclick") return;
    const target = event.target.closest<HTMLButtonElement>("button[data-action]");
    if (!target || !event.currentTarget.contains(target)) return;
    const block = rendered.blocks[Number(target.dataset.code)];
    if (!block) return;
    if (target.dataset.action === "copy") {
      try { await navigator.clipboard.writeText(block.content); notify("Code copied"); }
      catch { notify("Clipboard unavailable. Select the code to copy it."); }
    } else if (target.dataset.action === "artifact") on_artifact(block);
    else {
      const pre = target.closest(".code-block")?.querySelector("pre");
      if (!pre) return;
      const collapsed = pre.classList.toggle("code-collapsed");
      target.textContent = collapsed ? "Expand all " + block.content.split("\n").length + " lines" : "Collapse code";
    }
  }
</script>

<div class="markdown" onclick={handle_click} onauxclick={handle_click} role="presentation">
  {@html rendered.html}
</div>

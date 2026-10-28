<script>
  import { Marked } from "marked";
  import DOMPurify from "dompurify";
  import hljs from "highlight.js/lib/common";
  let { content = "", on_artifact = () => {}, notify = () => {} } = $props();
  const escape_html = (value) =>
    String(value).replace(
      /[&<>"']/g,
      (char) =>
        ({
          "&": "&amp;",
          "<": "&lt;",
          ">": "&gt;",
          '"': "&quot;",
          "'": "&#39;",
        })[char],
    );
  const rendered = $derived.by(() => {
    const blocks = [];
    const parser = new Marked({
      gfm: true,
      renderer: {
        html: (token) => escape_html(token.text),
        image: (token) => escape_html(token.text || "Image"),
        code: ({ text, lang }) => {
          const language = (lang || "text").split(/\s/)[0];
          const index = blocks.length;
          blocks.push({
            title:
              language === "html" ? "Your web page" : `${language} artifact`,
            language,
            content: text,
          });
          const lines = text.split("\n").length;
          const highlighted = hljs.getLanguage(language)
            ? hljs.highlight(text, { language, ignoreIllegals: true }).value
            : escape_html(text);
          return `<div class="code-block"><div class="code-toolbar"><span>${escape_html(language)}</span><div>${lines > 12 ? `<button type="button" class="text-btn" data-code="${index}" data-action="artifact">Open artifact ↗</button>` : ""}<button type="button" class="text-btn" aria-label="Copy code" data-code="${index}" data-action="copy">Copy</button></div></div><pre class="${lines > 12 ? "code-collapsed" : ""}"><code class="hljs language-${escape_html(language)}">${highlighted}</code></pre>${lines > 12 ? `<button type="button" class="code-expand" data-code="${index}" data-action="expand">Expand all ${lines} lines</button>` : ""}</div>`;
        },
      },
    });
    const html = DOMPurify.sanitize(parser.parse(content), {
      ADD_ATTR: ["target", "rel", "data-code", "data-action"],
      ADD_TAGS: ["button"],
      FORBID_TAGS: ["style", "iframe", "form", "input"],
      FORBID_ATTR: ["style"],
    });
    return { html, blocks };
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

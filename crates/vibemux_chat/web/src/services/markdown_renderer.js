import { Marked } from "marked";
import hljs from "highlight.js/lib/common";

const escape_html = (value) => String(value).replace(/[&<>"']/g, (char) => ({
  "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
})[char]);

// Rendering and sanitization both deny links unless the host opts in. Raw HTML
// and remote images are never rendered in either host.
export function render_markdown(content, { links = false } = {}) {
  const blocks = [];
  const parser = new Marked({
    gfm: true,
    renderer: {
      html: (token) => escape_html(token.text),
      image: (token) => escape_html(token.text || "Image"),
      ...(!links ? {
        // Render the existing label tokens, never re-lex token.text: an
        // autolink's text is itself a URL and re-lexing would recurse forever.
        link(token) { return this.parser.parseInline(token.tokens); },
      } : {}),
      code: ({ text, lang }) => {
        const language = (lang || "text").split(/\s/)[0];
        const index = blocks.length;
        blocks.push({
          title: language === "html" ? "Your web page" : `${language} artifact`,
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
  return { html: parser.parse(content, { async: false }), blocks };
}

export function markdown_sanitize_options({ links = false } = {}) {
  return {
    ADD_ATTR: ["target", "rel", "data-code", "data-action"],
    ADD_TAGS: ["button"],
    FORBID_TAGS: ["style", "iframe", "form", "input", "img", ...(!links ? ["a"] : [])],
    FORBID_ATTR: ["style", ...(!links ? ["href", "target"] : [])],
  };
}

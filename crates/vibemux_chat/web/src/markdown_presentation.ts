import { Marked } from "marked";
import hljs from "highlight.js/lib/common";

export type CodeArtifact = { title: string; language: string; content: string };

export function escape_html(value: unknown): string {
  return String(value).replace(/[&<>"']/g, (char) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  })[char] ?? char);
}

/** Pure presentation stage. The component sanitizes its output before insertion. */
export function render_markdown(content: string, links_enabled = true): { html: string; blocks: CodeArtifact[] } {
  const blocks: CodeArtifact[] = [];
  const parser = new Marked({
    gfm: true,
    renderer: {
      html: (token) => escape_html(token.text),
      image: (token) => escape_html(token.text || "Image"),
      link: function (token) {
        const label = this.parser.parseInline(token.tokens);
        return links_enabled
          ? '<a href="' + escape_html(token.href) + '"' + (token.title ? ' title="' + escape_html(token.title) + '"' : '') + '>' + label + '</a>'
          : '<span class="disabled-link">' + label + '</span>';
      },
      code: ({ text, lang }) => {
        const language = (lang || "text").split(/\s/)[0];
        const index = blocks.length;
        blocks.push({ title: language === "html" ? "Your web page" : language + " artifact", language, content: text });
        const lines = text.split("\n").length;
        const highlighted = hljs.getLanguage(language)
          ? hljs.highlight(text, { language, ignoreIllegals: true }).value
          : escape_html(text);
        return '<div class="code-block"><div class="code-toolbar"><span>' + escape_html(language) +
          '</span><div>' + (lines > 12 ? '<button type="button" class="text-btn" data-code="' + index + '" data-action="artifact">Open artifact ↗</button>' : '') +
          '<button type="button" class="text-btn" aria-label="Copy code" data-code="' + index + '" data-action="copy">Copy</button></div></div><pre class="' +
          (lines > 12 ? "code-collapsed" : "") + '"><code class="hljs language-' + escape_html(language) + '">' + highlighted +
          '</code></pre>' + (lines > 12 ? '<button type="button" class="code-expand" data-code="' + index + '" data-action="expand">Expand all ' + lines + ' lines</button>' : '') + '</div>';
      },
    },
  });
  return { html: parser.parse(content, { async: false }), blocks };
}

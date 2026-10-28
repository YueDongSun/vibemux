// Controlled local HTTP fixture for interaction QA. It is excluded from production builds.
const fixture_key = "qa-fixture-key";
const html_artifact = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Slow Studio</title>
<style>
body { margin:0; background:#f3efe4; color:#283c32; font-family:Georgia,serif; }
nav { padding:24px 8%; display:flex; justify-content:space-between; border-bottom:1px solid #cbd0c1; }
main { max-width:800px; padding:72px 8%; }
small { letter-spacing:.2em; font-family:Arial,sans-serif; }
h1 { font-size:clamp(40px,8vw,80px); font-weight:400; line-height:1.05; }
p { max-width:480px; line-height:1.7; font-family:Arial,sans-serif; }
button { background:#283c32; border:0; border-radius:99px; padding:16px 24px; color:#fff; cursor:pointer; }
footer { padding:32px 8%; border-top:1px solid #cbd0c1; }
</style>
</head>
<body>
<nav><strong>slow studio.</strong><span>Thoughtfully made.</span></nav>
<main><small>INDEPENDENT CREATIVE STUDIO</small><h1>Good things<br>take a little care.</h1>
<p>We build thoughtful identities and digital experiences for people with a different way of seeing the world.</p>
<button onclick="document.getElementById('result').textContent='Let’s make something meaningful.'">Start a conversation ↗</button>
<p id="result" aria-live="polite"></p></main>
<footer>Strategy · Identity · Digital experiences</footer>
</body></html>`;
function response_text(prompt) {
  if (prompt.startsWith("perf:")) return "Controlled QA fixture reply.";
  const intro = "**Controlled QA fixture — no live model was called.**\n\n";
  if (/html|landing|web page|studio|artifact/i.test(prompt))
    return (
      intro +
      "Here’s a complete page for a thoughtful creative studio. Open the artifact to explore the live preview.\n\n```html\n" +
      html_artifact +
      "\n```\n\nThe button works inside the isolated preview."
    );
  if (/long code/i.test(prompt))
    return (
      intro +
      "## A readable code example\n\n```javascript\n" +
      Array.from(
        { length: 36 },
        (_, index) => `const item_${index + 1} = ${index + 1};`,
      ).join("\n") +
      "\n```"
    );
  return (
    intro +
    "## A clearer place to start\n\nLet’s make some room for **" +
    prompt.replace(/[<>`]/g, "").slice(0, 120) +
    '**.\n\n### Start with the essentials\n\n1. Define the outcome in one sentence.\n2. Choose the smallest useful first step.\n3. Leave time to reflect and improve.\n\n- Keep the scope focused.\n- Make progress visible.\n- Adjust when you learn something new.\n\n| Step | Focus | Outcome |\n| --- | --- | --- |\n| Discover | Ask better questions | A shared direction |\n| Create | Try one small idea | Something you can use |\n| Refine | Learn from feedback | A stronger next version |\n\n> A useful plan makes your next action easier to see.\n\nUse `one_clear_goal` as your starting point.\n\n```javascript\nconst next_step = "Make one useful thing";\nconsole.log(next_step);\n```'
  );
}
export function qa_provider() {
  let qa_origin = "http://127.0.0.1:5182";
  return {
    name: "controlled-qa-provider",
    configResolved(config) {
      qa_origin = `http://127.0.0.1:${config.server.port || 5182}`;
    },
    transform(source, id) {
      const path = id.replaceAll("\\", "/");
      if (
        path.endsWith("/src/config.js") ||
        path.endsWith("/src/services/provider.js")
      )
        return source.replaceAll("https://api.anthropic.com", qa_origin);
    },
    configureServer(server) {
      server.middlewares.use(async (request, response, next) => {
        if (
          !["/v1/models", "/v1/messages"].some((path) =>
            request.url?.startsWith(path),
          )
        )
          return next();
        if (request.headers["x-api-key"] !== fixture_key) {
          response.writeHead(401, { "Content-Type": "application/json" });
          response.end(
            JSON.stringify({
              error: {
                type: "authentication_error",
                message: "Controlled fixture rejected this key",
              },
            }),
          );
          return;
        }
        if (request.url.startsWith("/v1/models")) {
          response.writeHead(200, { "Content-Type": "application/json" });
          response.end(
            JSON.stringify({
              data: [{ id: "claude-sonnet-4-6", display_name: "Sonnet 4.6" }],
            }),
          );
          return;
        }
        const chunks = [];
        for await (const chunk of request) chunks.push(chunk);
        let body;
        try {
          body = JSON.parse(Buffer.concat(chunks).toString());
        } catch {
          response.writeHead(400);
          response.end();
          return;
        }
        const prompt =
          body.messages
            ?.at(-1)
            ?.content?.filter((item) => item.type === "text")
            .map((item) => item.text)
            .join(" ") || "";
        response.writeHead(200, {
          "Content-Type": "text/event-stream",
          "Cache-Control": "no-cache",
          Connection: "keep-alive",
        });
        const event = (value) =>
          response.write(`data: ${JSON.stringify(value)}\n\n`);
        event({
          type: "message_start",
          message: {
            role: "assistant",
            content: [],
            usage: { input_tokens: 25, output_tokens: 0 },
          },
        });
        event({
          type: "content_block_start",
          index: 0,
          content_block: { type: "text", text: "" },
        });
        const text = response_text(prompt),
          parts = text.match(/[\s\S]{1,36}/g) || [];
        let index = 0;
        const timer = setInterval(() => {
          if (response.destroyed) {
            clearInterval(timer);
            return;
          }
          if (index < parts.length) {
            event({
              type: "content_block_delta",
              index: 0,
              delta: { type: "text_delta", text: parts[index++] },
            });
          } else {
            clearInterval(timer);
            event({ type: "content_block_stop", index: 0 });
            event({
              type: "message_delta",
              delta: { stop_reason: "end_turn" },
              usage: { output_tokens: Math.ceil(text.length / 4) },
            });
            event({ type: "message_stop" });
            response.end();
          }
        }, 60);
        response.on("close", () => clearInterval(timer));
      });
    },
  };
}

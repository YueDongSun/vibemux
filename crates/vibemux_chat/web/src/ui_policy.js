import { Lexer } from "marked";

export function group_conversations(conversations, now = new Date()) {
  const today = new Date(now);
  today.setHours(0, 0, 0, 0);
  const yesterday = new Date(today);
  yesterday.setDate(yesterday.getDate() - 1);
  const groups = { Pinned: [], Today: [], Yesterday: [], Older: [] };
  [...conversations]
    .sort((a, b) => new Date(b.updated_at) - new Date(a.updated_at))
    .forEach((chat) => {
      const date = new Date(chat.updated_at);
      groups[
        chat.pinned
          ? "Pinned"
          : date >= today
            ? "Today"
            : date >= yesterday
              ? "Yesterday"
              : "Older"
      ].push(chat);
    });
  return Object.entries(groups).filter(([, items]) => items.length);
}
export function extract_artifact(text) {
  const token = Lexer.lex(text).find(
    (item) =>
      item.type === "code" &&
      (item.text.split("\n").length > 12 ||
        ["html", "svg"].includes(item.lang?.split(/\s/)[0])),
  );
  const language = token?.lang?.split(/\s/)[0] || "text";
  return token
    ? {
        title: language === "html" ? "Your web page" : `${language} artifact`,
        language,
        content: token.text,
      }
    : text.length > 2500
      ? { title: "Conversation document", language: "markdown", content: text }
      : null;
}

export function stopped_snapshot(chat) {
  const copy = JSON.parse(JSON.stringify(chat));
  copy.messages = copy.messages.map((message) =>
    message.status === "streaming"
      ? { ...message, status: "stopped" }
      : message,
  );
  return copy;
}

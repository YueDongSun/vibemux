export const models = [
  {
    id: "claude-sonnet-4-6",
    name: "Sonnet 4.6",
    description: "A balance of speed and intelligence",
  },
  {
    id: "claude-opus-4-6",
    name: "Opus 4.6",
    description: "For your most ambitious work",
  },
  {
    id: "claude-haiku-4-5",
    name: "Haiku 4.5",
    description: "Quick answers, beautifully simple",
  },
];
export const default_settings = {
  interface_style: "claude",
  theme: "system",
  font_size: "default",
  density: "comfortable",
  enter_to_send: true,
  auto_scroll: true,
  default_model: models[0].id,
  default_thinking: false,
};
export const app_config = {
  storage_key: "vibemux_chat_v1",
  page_size: 40,
  max_characters: 30000,
  max_attachment_bytes: 10485760,
  max_attachments: 5,
  search_delay: 180,
  toast_duration: 7000,
  save_delay: 400,
  api_origin: "https://api.anthropic.com",
};
export const shortcuts = [
  ["New conversation", "Alt + N"],
  ["Focus composer", "Alt + /"],
  ["Toggle sidebar", "Alt + B"],
  ["Toggle theme", "Alt + T"],
  ["Search all conversations", "Alt + K"],
  ["Send message", "Ctrl / ⌘ + Enter"],
  ["Stop generation / close dialog", "Escape"],
];

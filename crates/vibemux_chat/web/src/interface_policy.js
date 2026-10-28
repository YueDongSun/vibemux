export const interface_options = [
  {
    id: "claude",
    name: "Claude",
    description: "A calm space for conversation",
  },
  { id: "codex", name: "Codex", description: "A focused coding workspace" },
];

export function valid_interface(value) {
  return interface_options.some((option) => option.id === value);
}

export function resolve_interface(search, saved_style = "claude") {
  const requested = new URLSearchParams(search).get("view");
  return valid_interface(requested)
    ? requested
    : valid_interface(saved_style)
      ? saved_style
      : "claude";
}

export function interface_url(current_url, style) {
  if (!valid_interface(style)) throw new TypeError("Unknown interface style.");
  const url = new URL(current_url);
  url.searchParams.set("view", style);
  return url.pathname + url.search + url.hash;
}

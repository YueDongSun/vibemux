import { mount } from "svelte";
import App from "./app.svelte";
import { create_browser_host } from "./services/browser_host.js";
import "./tokens.css";
import "./style.css";
import "./codex_layout.css";
mount(App, {
  target: document.getElementById("app"),
  props: { host: create_browser_host() },
});

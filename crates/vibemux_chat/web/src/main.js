import { mount } from "svelte";
import App from "./app.svelte";
import "./tokens.css";
import "./style.css";
import "./codex_layout.css";
mount(App, { target: document.getElementById("app") });

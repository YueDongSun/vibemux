import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { qa_provider } from "./scripts/qa_provider.mjs";
export default defineConfig(({ command, mode }) => {
  if (mode === "qa" && command !== "serve")
    throw new Error(
      "QA fixtures are development-only and cannot be built for production.",
    );
  return {
    plugins: [svelte(), ...(mode === "qa" ? [qa_provider()] : [])],
    build: { outDir: "dist", sourcemap: false },
  };
});

import { defineConfig } from "vite";
import { wasm } from "./plugins/wasm.ts";

export default defineConfig({
  // A relative base lets the built page work from any path.
  base: "./",
  plugins: [wasm()],
  // The worker imports the wasm-bindgen glue, which needs import.meta.url.
  worker: { format: "es" },
});

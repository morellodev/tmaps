import { defineConfig } from "vite";

export default defineConfig({
  // A relative base lets the built page work from any path.
  base: "./",
  // The worker imports the wasm-bindgen glue, which needs import.meta.url.
  worker: { format: "es" },
});

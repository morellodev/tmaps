import init, { prepare_tile } from "./pkg/tmaps_canvas.js";

const ready = Promise.withResolvers();
let lang = "";
const fetches = new Map();

const handlers = {
  init({ module, lang: language }) {
    lang = language;
    init({ module_or_path: module }).then(ready.resolve, (error) => {
      ready.reject(error);
      postMessage({ type: "failed", error: describe(error) });
    });
  },

  async tileset({ url }) {
    try {
      const response = await fetch(url);
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const json = await response.text();
      postMessage({ type: "tileset", json, attribution: JSON.parse(json).attribution ?? "" });
    } catch (error) {
      postMessage({ type: "tileset", error: describe(error) });
    }
  },

  async tile({ job, url }) {
    const controller = new AbortController();
    fetches.set(job, controller);
    try {
      const response = await fetch(url, { signal: controller.signal });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const bytes = new Uint8Array(await response.arrayBuffer());
      await ready.promise;
      controller.signal.throwIfAborted();
      const [labels, paths] = prepare_tile(bytes, lang);
      postMessage({ type: "tile", job, labels, paths }, [labels.buffer]);
    } catch (error) {
      if (!controller.signal.aborted) postMessage({ type: "tile", job, error: describe(error) });
    } finally {
      fetches.delete(job);
    }
  },

  cancel({ job }) {
    fetches.get(job)?.abort();
  },
};

function describe(error) {
  return String(error?.message ?? error);
}

addEventListener("message", ({ data }) => handlers[data.type]?.(data));
addEventListener("messageerror", () => {
  ready.reject(new Error("a message from the page cannot be read"));
  postMessage({ type: "failed", error: "a message from the page cannot be read" });
});

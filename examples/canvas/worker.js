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
      const bytes = await tileBytes(await fetch(url, { signal: controller.signal }));
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

// Tile servers send 404 or 204 for a tile with no data, and some send gzip without a
// Content-Encoding header, so the browser does not unzip it.
async function tileBytes(response) {
  if (response.status === 404) return new Uint8Array();
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  const bytes = new Uint8Array(await response.arrayBuffer());
  if (bytes[0] !== 0x1f || bytes[1] !== 0x8b) return bytes;
  const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream("gzip"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

function describe(error) {
  return String(error?.message ?? error);
}

addEventListener("message", ({ data }) => handlers[data.type]?.(data));
addEventListener("messageerror", () => {
  ready.reject(new Error("a message from the page cannot be read"));
  postMessage({ type: "failed", error: "a message from the page cannot be read" });
});

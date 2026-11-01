import init, { prepare_tile } from "../pkg/tmaps_canvas.js";
import type { FromWorker, TilePath, ToWorker } from "./messages.ts";

const ready = Promise.withResolvers<unknown>();
let lang = "";
const fetches = new Map<number, AbortController>();

function send(message: FromWorker, transfer: Transferable[] = []) {
  postMessage(message, transfer);
}

function start(module: WebAssembly.Module, language: string) {
  lang = language;
  init({ module_or_path: module }).then(ready.resolve, (error: unknown) => {
    ready.reject(error);
    send({ type: "failed", error: describe(error) });
  });
}

async function loadTileSet(url: string) {
  try {
    const response = await fetch(url);
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const json = await response.text();
    send({ type: "tileset", json, attribution: JSON.parse(json).attribution ?? "" });
  } catch (error) {
    send({ type: "tileset", error: describe(error) });
  }
}

async function loadTile(job: number, url: string) {
  const controller = new AbortController();
  fetches.set(job, controller);
  try {
    const bytes = await tileBytes(await fetch(url, { signal: controller.signal }));
    await ready.promise;
    controller.signal.throwIfAborted();
    const [labels, paths] = prepare_tile(bytes, lang) as [Uint8Array<ArrayBuffer>, TilePath[]];
    send({ type: "tile", job, labels, paths }, [labels.buffer]);
  } catch (error) {
    if (!controller.signal.aborted) send({ type: "tile", job, error: describe(error) });
  } finally {
    fetches.delete(job);
  }
}

// Tile servers send 404 or 204 for a tile with no data, and some send gzip without a
// Content-Encoding header, so the browser does not unzip it.
async function tileBytes(response: Response) {
  if (response.status === 404) return new Uint8Array();
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  const bytes = new Uint8Array(await response.arrayBuffer());
  if (bytes[0] !== 0x1f || bytes[1] !== 0x8b) return bytes;
  const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream("gzip"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

function describe(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

addEventListener("message", ({ data }: MessageEvent<ToWorker>) => {
  switch (data.type) {
    case "init":
      return start(data.module, data.lang);
    case "tileset":
      return void loadTileSet(data.url);
    case "tile":
      return void loadTile(data.job, data.url);
    case "cancel":
      return fetches.get(data.job)?.abort();
  }
});
addEventListener("messageerror", () => {
  ready.reject(new Error("a message from the page cannot be read"));
  send({ type: "failed", error: "a message from the page cannot be read" });
});

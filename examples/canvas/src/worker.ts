import { initSync, prepare_tile } from "../pkg/tmaps_canvas.js";
import { describe } from "./errors.ts";
import type { FromWorker, TilePath, ToWorker } from "./messages.ts";

const fetches = new Map<number, AbortController>();
let initError: unknown;

function send(message: FromWorker, transfer: Transferable[] = []) {
  postMessage(message, transfer);
}

// The page sends "init" before the first "tile", and messages arrive in order, so prepare_tile
// always runs after initSync.
function start(module: WebAssembly.Module) {
  try {
    initSync({ module });
  } catch (error) {
    failInit(error);
  }
}

function failInit(error: unknown) {
  initError = error;
  send({ type: "failed", error: describe(error) });
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
    controller.signal.throwIfAborted();
    if (initError) throw initError;
    const [labels, paths] = prepare_tile(bytes, navigator.language) as [Uint8Array<ArrayBuffer>, TilePath[]];
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

addEventListener("message", ({ data }: MessageEvent<ToWorker>) => {
  switch (data.type) {
    case "init":
      return start(data.module);
    case "tileset":
      return void loadTileSet(data.url);
    case "tile":
      return void loadTile(data.job, data.url);
    case "cancel":
      return fetches.get(data.job)?.abort();
  }
});
// Only "init" has data that can fail to deserialize: the WebAssembly.Module.
addEventListener("messageerror", () => failInit(new Error("a message from the page cannot be read")));

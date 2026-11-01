import init, { WebMap } from "../pkg/tmaps_canvas.js";
import { describe } from "./errors.ts";
import type { FromWorker, TilePath, TileResult, TileSetResult } from "./messages.ts";

const TILEJSON = "https://tiles.openfreemap.org/planet";
const OSM_COPYRIGHT = "https://www.openstreetmap.org/copyright";
const START = { lat: 45.46, lon: 9.19, zoom: 12 };
const KEY_PAN = 100;
const KEY_ZOOM = 0.5;

function element<T extends Element>(selector: string, type: new () => T): T {
  const found = document.querySelector(selector);
  if (!(found instanceof type)) throw new Error(`The page has no ${selector} element`);
  return found;
}

const canvas = element("#map", HTMLCanvasElement);
const status = element("#status", HTMLOutputElement);
const attribution = element("#attribution", HTMLElement);

function showError(message: string) {
  status.value = message;
  status.classList.replace("visually-hidden", "error");
}

// The page compiles the module one time and sends it to the workers, so that they do not
// download and compile it again.
const wasm = WebAssembly.compileStreaming(fetch(new URL("../pkg/tmaps_canvas_bg.wasm", import.meta.url)));

type WorkerEntry = { worker: Worker; jobs: number };

// Workers fetch and prepare the TileJSON document and the tiles, so that the main thread only
// draws. Half of the cores, as in MapLibre: the main thread and the browser use the others.
const WORKER_COUNT = Math.min(4, Math.max(1, Math.floor(navigator.hardwareConcurrency / 2)));
const tileSet = Promise.withResolvers<TileSetResult>();
const workers = Array.from({ length: WORKER_COUNT }, (): WorkerEntry => {
  const worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
  const entry = { worker, jobs: 0 };
  worker.addEventListener("message", ({ data }: MessageEvent<FromWorker>) => {
    if (data.type === "tile") tileArrived(entry, data);
    else if (data.type === "tileset") tileSet.resolve(data);
    else showError(`The map cannot load tiles: ${data.error}`);
  });
  worker.addEventListener("messageerror", () => console.warn("A message from a tile worker cannot be read"));
  worker.addEventListener("error", (event) => {
    showError(`The map cannot load tiles: ${event.message || "the tile worker stopped"}`);
  });
  return entry;
});
workers[0].worker.postMessage({ type: "tileset", url: TILEJSON });

let map: WebMap;
try {
  const module = await wasm;
  await init({ module_or_path: module });
  map = new WebMap(canvas);
  for (const { worker } of workers) worker.postMessage({ type: "init", module });
} catch (error) {
  showError(`The map cannot start: ${describe(error)}`);
  throw error;
}

try {
  const [zoom, lat, lon] = location.hash.slice(1).split("/").map(Number);
  if (!Number.isFinite(zoom)) throw new Error("no zoom in the URL");
  map.set_view(lat, lon, zoom);
} catch {
  map.set_view(START.lat, START.lon, START.zoom);
}

const dark = matchMedia("(prefers-color-scheme: dark)");
map.set_dark(dark.matches);
dark.addEventListener("change", (event) => {
  map.set_dark(event.matches);
  redraw();
});

let frame = 0;
function redraw() {
  frame ||= requestAnimationFrame((start) => {
    frame = 0;
    stepGlide(start);
    addArrivedTiles(start);
    requestTiles();
    if (map.draw(start)) redraw();
    describeView();
  });
}

type Tile = { z: number; x: number; y: number };
type ArrivedTile = Tile & { labels: Uint8Array; paths: TilePath[] };

const arrived: ArrivedTile[] = [];
// Each fetch of a tile is a job with its own number, so that a late result of a cancelled job
// cannot close a newer job for the same tile.
const jobs = new Map<number, { entry: WorkerEntry; key: string; tile: Tile }>();
const tiles = new Map<string, number>();
let lastJob = 0;

function tileArrived(entry: WorkerEntry, result: TileResult) {
  const job = jobs.get(result.job);
  if (!job) return;
  jobs.delete(result.job);
  tiles.delete(job.key);
  entry.jobs -= 1;
  if ("error" in result) {
    console.warn(`Tile ${job.key} failed: ${result.error}`);
    return;
  }
  arrived.push({ ...job.tile, labels: result.labels, paths: result.paths });
  redraw();
}

function requestTiles() {
  const stale = map.stale_tiles();
  for (let i = 0; i < stale.length; i += 3) {
    const key = stale.slice(i, i + 3).join("/");
    const id = tiles.get(key);
    if (id === undefined) continue;
    const job = jobs.get(id);
    if (!job) continue;
    job.entry.worker.postMessage({ type: "cancel", job: id });
    job.entry.jobs -= 1;
    jobs.delete(id);
    tiles.delete(key);
  }
  const missing = map.missing_tiles();
  for (let i = 0; i < missing.length; i += 3) {
    const [z, x, y] = missing.slice(i, i + 3);
    const url = map.tile_url(z, x, y);
    if (url === undefined) continue;
    const key = `${z}/${x}/${y}`;
    const entry = workers.reduce((least, each) => (each.jobs < least.jobs ? each : least));
    const id = ++lastJob;
    entry.worker.postMessage({ type: "tile", job: id, url });
    entry.jobs += 1;
    jobs.set(id, { entry, key, tile: { z, x, y } });
    tiles.set(key, id);
  }
}

// A frame adds only the tiles that fit in its budget, and leaves the rest to the next frames.
const TILE_BUDGET_MS = 4;
function addArrivedTiles(start: number) {
  do {
    const tile = arrived.shift();
    if (!tile) return;
    try {
      const paths = tile.paths.map(([kind, minZoom, data]) => [kind, minZoom, new Path2D(data)]);
      map.add_tile(tile.z, tile.x, tile.y, tile.labels, paths);
    } catch (error) {
      console.warn(`Tile ${tile.z}/${tile.x}/${tile.y} failed`, error);
    }
  } while (performance.now() - start < TILE_BUDGET_MS);
  if (arrived.length) redraw();
}

// The status is a live region, so it changes only when the view stops moving.
let settle = 0;
function describeView() {
  clearTimeout(settle);
  settle = setTimeout(() => {
    if (status.classList.contains("error")) return;
    const [lat, lon, zoom] = [map.lat().toFixed(4), map.lon().toFixed(4), map.zoom().toFixed(1)];
    status.value = `${lat}, ${lon} · zoom ${zoom}`;
    history.replaceState(null, "", `#${zoom}/${lat}/${lon}`);
  }, 500);
}

// Key and double-click steps glide to their end, so that the map does not jump. In each
// GLIDE_HALF_LIFE_MS, the map moves half of the distance that is left.
const GLIDE_HALF_LIFE_MS = 50;
const GLIDE_MAX = { pan: 3 * KEY_PAN, zoom: 3 };
const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)");
const glide = { dx: 0, dy: 0, zoom: 0, x: 0, y: 0, time: null as number | null };

type GlideStep = { dx?: number; dy?: number; zoom?: number; x?: number; y?: number };

function glideBy({ dx = 0, dy = 0, zoom = 0, x = 0, y = 0 }: GlideStep) {
  if (reducedMotion.matches) {
    map.pan_by(dx, dy);
    if (zoom) map.zoom_at(zoom, x, y);
  } else {
    glide.time ??= performance.now();
    const limit = (value: number, max: number) => Math.min(max, Math.max(-max, value));
    glide.dx = limit(glide.dx + dx, GLIDE_MAX.pan);
    glide.dy = limit(glide.dy + dy, GLIDE_MAX.pan);
    if (zoom) Object.assign(glide, { zoom: limit(glide.zoom + zoom, GLIDE_MAX.zoom), x, y });
  }
  redraw();
}

function stopGlide() {
  Object.assign(glide, { dx: 0, dy: 0, zoom: 0, time: null });
}

function stepGlide(now: number) {
  if (glide.time === null || (!glide.dx && !glide.dy && !glide.zoom)) {
    glide.time = null;
    return;
  }
  // A frame can start before the key press, so its time can be earlier.
  const part = 1 - 0.5 ** (Math.max(0, now - glide.time) / GLIDE_HALF_LIFE_MS);
  glide.time = Math.max(now, glide.time);
  const take = (key: "dx" | "dy" | "zoom", last: number) => {
    const step = Math.abs(glide[key]) < last ? glide[key] : glide[key] * part;
    glide[key] -= step;
    return step;
  };
  map.pan_by(take("dx", 0.5), take("dy", 0.5));
  const zoom = take("zoom", 0.002);
  if (zoom) map.zoom_at(zoom, glide.x, glide.y);
  redraw();
}

type Point = { x: number; y: number };

const pointers = new Map<number, Point>();
canvas.addEventListener("pointerdown", (event) => {
  if (event.button !== 0 || pointers.size === 2) return;
  pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
  canvas.setPointerCapture(event.pointerId);
});
canvas.addEventListener("pointermove", (event) => {
  const last = pointers.get(event.pointerId);
  if (!last || (event.clientX === last.x && event.clientY === last.y)) return;
  const next = { x: event.clientX, y: event.clientY };
  stopGlide();
  const other = [...pointers].find(([id]) => id !== event.pointerId);
  if (other) pinch(other[1], last, next);
  else map.pan_by(last.x - next.x, last.y - next.y);
  pointers.set(event.pointerId, next);
  redraw();
});
for (const type of ["pointerup", "pointercancel"] as const) {
  canvas.addEventListener(type, (event) => pointers.delete(event.pointerId));
}

// The point under the middle of the two fingers follows the middle, and the zoom follows the
// distance between the fingers: two times the distance is one zoom level.
function pinch(still: Point, from: Point, to: Point) {
  const box = canvas.getBoundingClientRect();
  const middle = (a: Point, b: Point) => ({ x: (a.x + b.x) / 2 - box.left, y: (a.y + b.y) / 2 - box.top });
  const [before, after] = [middle(still, from), middle(still, to)];
  map.pan_by(before.x - after.x, before.y - after.y);
  const spread = Math.hypot(to.x - still.x, to.y - still.y) / Math.hypot(from.x - still.x, from.y - still.y);
  if (Number.isFinite(spread) && spread > 0) map.zoom_at(Math.log2(spread), after.x, after.y);
}

canvas.addEventListener(
  "wheel",
  (event) => {
    event.preventDefault();
    stopGlide();
    const unit: Record<number, number> = {
      [WheelEvent.DOM_DELTA_LINE]: 16,
      [WheelEvent.DOM_DELTA_PAGE]: canvas.clientHeight,
    };
    // Trackpad pinches come as wheel events with ctrlKey and small deltas.
    const speed = event.ctrlKey ? 0.01 : 0.002;
    map.zoom_at(-event.deltaY * (unit[event.deltaMode] ?? 1) * speed, event.offsetX, event.offsetY);
    redraw();
  },
  { passive: false },
);

canvas.addEventListener("dblclick", (event) => {
  glideBy({ zoom: event.shiftKey ? -1 : 1, x: event.offsetX, y: event.offsetY });
});

canvas.addEventListener("keydown", (event) => {
  const center = { x: canvas.clientWidth / 2, y: canvas.clientHeight / 2 };
  const steps: Record<string, GlideStep> = {
    ArrowLeft: { dx: -KEY_PAN },
    ArrowRight: { dx: KEY_PAN },
    ArrowUp: { dy: -KEY_PAN },
    ArrowDown: { dy: KEY_PAN },
    "+": { zoom: KEY_ZOOM, ...center },
    "=": { zoom: KEY_ZOOM, ...center },
    "-": { zoom: -KEY_ZOOM, ...center },
  };
  const step = steps[event.key];
  if (!step || event.metaKey || event.ctrlKey || event.altKey) return;
  event.preventDefault();
  glideBy(step);
});

new ResizeObserver(redraw).observe(canvas);

// ResizeObserver misses a new pixel ratio at the same CSS size, for example on another screen.
function watchPixelRatio() {
  matchMedia(`(resolution: ${devicePixelRatio}dppx)`).addEventListener(
    "change",
    () => {
      watchPixelRatio();
      redraw();
    },
    { once: true },
  );
}
watchPixelRatio();

// The TileJSON attribution is HTML from the network: copy only its text and its web links.
function showAttribution(html: string) {
  const parts = copyCredits(new DOMParser().parseFromString(html, "text/html").body);
  const namesOsm = parts.some((part) =>
    typeof part === "string"
      ? part.includes("OpenStreetMap")
      : part.textContent.includes("OpenStreetMap") || `.${part.hostname}`.endsWith(".openstreetmap.org"),
  );
  if (!namesOsm) parts.push(parts.length ? " " : "", link(OSM_COPYRIGHT, "© OpenStreetMap contributors"));
  attribution.replaceChildren(...parts);
}

const HIDDEN_TEXT = new Set(["STYLE", "SCRIPT", "TEMPLATE", "TEXTAREA", "NOSCRIPT"]);
function copyCredits(parent: Node): (string | HTMLAnchorElement)[] {
  return [...parent.childNodes].flatMap((node) => {
    if (node instanceof Text) return [node.data];
    if (!(node instanceof Element) || HIDDEN_TEXT.has(node.tagName)) return [];
    if (node.tagName !== "A") return copyCredits(node);
    const href = webUrl(node.getAttribute("href"));
    return [href ? link(href, node.textContent) : node.textContent];
  });
}

// Relative links in the credit point to the tile server, not to this page.
function webUrl(href: string | null) {
  if (href === null) return null;
  try {
    const url = new URL(href, TILEJSON);
    return ["https:", "http:"].includes(url.protocol) ? url.href : null;
  } catch {
    return null;
  }
}

function link(href: string, text: string) {
  const anchor = document.createElement("a");
  anchor.href = href;
  anchor.target = "_blank";
  anchor.textContent = text;
  return anchor;
}

try {
  const result = await tileSet.promise;
  if ("error" in result) throw new Error(result.error);
  map.set_tile_set(result.json);
  showAttribution(result.attribution);
  redraw();
} catch (error) {
  showError(`The map server is not available: ${describe(error)}`);
  showAttribution("");
  map.draw(performance.now());
}

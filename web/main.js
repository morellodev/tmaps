import init, { WebMap } from "./pkg/tmaps_canvas.js";

const TILEJSON = "https://tiles.openfreemap.org/planet";
const OSM_COPYRIGHT = "https://www.openstreetmap.org/copyright";
const START = { lat: 45.46, lon: 9.19, zoom: 12 };
const KEY_PAN = 100;
const KEY_ZOOM = 0.5;

const canvas = document.querySelector("#map");
const status = document.querySelector("#status");
const attribution = document.querySelector("#attribution");

function showError(message) {
  status.value = message;
  status.classList.replace("visually-hidden", "error");
}

let map;
try {
  await init();
  map = new WebMap(canvas);
} catch (error) {
  showError(`The map cannot start: ${error.message}`);
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
    addArrivedTiles(start);
    fetchMissingTiles();
    if (map.draw(start)) redraw();
    describeView();
  });
}

// Workers fetch and prepare the tiles, so that the main thread stays free for drawing.
const arrived = [];
const workers = Array.from({ length: 2 }, () => {
  const worker = new Worker(new URL("worker.js", import.meta.url), { type: "module" });
  worker.addEventListener("message", ({ data }) => {
    if (data.error) {
      console.warn(`Tile ${data.z}/${data.x}/${data.y} failed: ${data.error}`);
      return;
    }
    arrived.push(data);
    redraw();
  });
  worker.addEventListener("error", (event) => {
    showError(`The map cannot load tiles: ${event.message || "the tile worker stopped"}`);
  });
  return worker;
});

let nextWorker = 0;
function fetchMissingTiles() {
  const ids = map.missing_tiles();
  for (let i = 0; i < ids.length; i += 3) {
    const [z, x, y] = ids.slice(i, i + 3);
    workers[nextWorker].postMessage({ z, x, y, url: map.tile_url(z, x, y), lang: navigator.language });
    nextWorker = (nextWorker + 1) % workers.length;
  }
}

// A frame adds only the tiles that fit in its budget, and leaves the rest to the next frames.
const TILE_BUDGET_MS = 4;
function addArrivedTiles(start) {
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

let pointer = null;
canvas.addEventListener("pointerdown", (event) => {
  if (!event.isPrimary || event.button !== 0) return;
  pointer = { id: event.pointerId, x: event.clientX, y: event.clientY };
  canvas.setPointerCapture(event.pointerId);
});
canvas.addEventListener("pointermove", (event) => {
  if (event.pointerId !== pointer?.id) return;
  map.pan_by(pointer.x - event.clientX, pointer.y - event.clientY);
  pointer = { id: event.pointerId, x: event.clientX, y: event.clientY };
  redraw();
});
for (const type of ["pointerup", "pointercancel"]) {
  canvas.addEventListener(type, (event) => {
    if (event.pointerId === pointer?.id) pointer = null;
  });
}

canvas.addEventListener(
  "wheel",
  (event) => {
    event.preventDefault();
    const unit = { [WheelEvent.DOM_DELTA_LINE]: 16, [WheelEvent.DOM_DELTA_PAGE]: canvas.clientHeight };
    // Trackpad pinches come as wheel events with ctrlKey and small deltas.
    const speed = event.ctrlKey ? 0.01 : 0.002;
    map.zoom_at(-event.deltaY * (unit[event.deltaMode] ?? 1) * speed, event.offsetX, event.offsetY);
    redraw();
  },
  { passive: false },
);

canvas.addEventListener("dblclick", (event) => {
  map.zoom_at(event.shiftKey ? -1 : 1, event.offsetX, event.offsetY);
  redraw();
});

canvas.addEventListener("keydown", (event) => {
  const center = [canvas.clientWidth / 2, canvas.clientHeight / 2];
  const actions = {
    ArrowLeft: () => map.pan_by(-KEY_PAN, 0),
    ArrowRight: () => map.pan_by(KEY_PAN, 0),
    ArrowUp: () => map.pan_by(0, -KEY_PAN),
    ArrowDown: () => map.pan_by(0, KEY_PAN),
    "+": () => map.zoom_at(KEY_ZOOM, ...center),
    "=": () => map.zoom_at(KEY_ZOOM, ...center),
    "-": () => map.zoom_at(-KEY_ZOOM, ...center),
  };
  const action = actions[event.key];
  if (!action || event.metaKey || event.ctrlKey || event.altKey) return;
  event.preventDefault();
  action();
  redraw();
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
function showAttribution(html) {
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
function copyCredits(parent) {
  return [...parent.childNodes].flatMap((node) => {
    if (node.nodeType === Node.TEXT_NODE) return [node.data];
    if (node.nodeType !== Node.ELEMENT_NODE || HIDDEN_TEXT.has(node.tagName)) return [];
    if (node.tagName !== "A") return copyCredits(node);
    const href = webUrl(node.getAttribute("href"));
    return [href ? link(href, node.textContent) : node.textContent];
  });
}

// Relative links in the credit point to the tile server, not to this page.
function webUrl(href) {
  if (href === null) return null;
  try {
    const url = new URL(href, TILEJSON);
    return ["https:", "http:"].includes(url.protocol) ? url.href : null;
  } catch {
    return null;
  }
}

function link(href, text) {
  const anchor = document.createElement("a");
  anchor.href = href;
  anchor.target = "_blank";
  anchor.textContent = text;
  return anchor;
}

try {
  const response = await fetch(TILEJSON);
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  const tileJson = await response.text();
  map.set_tile_set(tileJson);
  showAttribution(JSON.parse(tileJson).attribution ?? "");
  redraw();
} catch (error) {
  showError(`The map server is not available: ${error.message}`);
  showAttribution("");
  map.draw(performance.now());
}

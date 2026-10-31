import init, { WebMap } from "./pkg/tmaps_canvas.js";

const TILEJSON = "https://tiles.openfreemap.org/planet";
const START = { lat: 45.46, lon: 9.19, zoom: 12 };
const KEY_PAN = 100;
const KEY_ZOOM = 0.5;

const canvas = document.querySelector("#map");
const status = document.querySelector("#status");
const attribution = document.querySelector("#attribution");

await init();
const map = new WebMap(canvas, navigator.language);
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
  frame ||= requestAnimationFrame(() => {
    frame = 0;
    fetchMissingTiles();
    map.draw();
    describeView();
  });
}

function fetchMissingTiles() {
  const ids = map.missing_tiles();
  for (let i = 0; i < ids.length; i += 3) {
    fetchTile(ids[i], ids[i + 1], ids[i + 2]);
  }
}

async function fetchTile(z, x, y) {
  try {
    const response = await fetch(map.tile_url(z, x, y));
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    map.add_tile(z, x, y, new Uint8Array(await response.arrayBuffer()));
    redraw();
  } catch (error) {
    console.warn(`Tile ${z}/${x}/${y} failed`, error);
  }
}

// The status is a live region, so it changes only when the view stops moving.
let settle = 0;
function describeView() {
  clearTimeout(settle);
  settle = setTimeout(() => {
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

try {
  const response = await fetch(TILEJSON);
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  map.set_tile_set(await response.text());
  attribution.textContent = map.attribution();
  redraw();
} catch (error) {
  status.value = `The map server is not available: ${error.message}`;
  map.draw();
}

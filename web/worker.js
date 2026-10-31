import init, { prepare_tile } from "./pkg/tmaps_canvas.js";

const ready = init();

addEventListener("message", async ({ data: { z, x, y, url, lang } }) => {
  try {
    await ready;
    const response = await fetch(url);
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const [labels, paths] = prepare_tile(new Uint8Array(await response.arrayBuffer()), lang);
    postMessage({ z, x, y, labels, paths }, [labels.buffer]);
  } catch (error) {
    postMessage({ z, x, y, error: String(error?.message ?? error) });
  }
});

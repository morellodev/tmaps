// The worker shares the wasm-bindgen types with the page. They name HTMLCanvasElement for WebMap,
// which a worker cannot create, so the type is empty here.
interface HTMLCanvasElement {}

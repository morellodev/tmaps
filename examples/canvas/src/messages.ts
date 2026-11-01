export type TilePath = [kind: number, minZoom: number, data: string];

export type ToWorker =
  | { type: "init"; module: WebAssembly.Module }
  | { type: "tileset"; url: string }
  | { type: "tile"; job: number; url: string }
  | { type: "cancel"; job: number };

export type TileSetResult = { type: "tileset"; json: string; attribution: string } | { type: "tileset"; error: string };

export type TileResult =
  | { type: "tile"; job: number; labels: Uint8Array<ArrayBuffer>; paths: TilePath[] }
  | { type: "tile"; job: number; error: string };

export type FromWorker = TileSetResult | TileResult | { type: "failed"; error: string };

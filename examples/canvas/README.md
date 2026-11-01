# tmaps-canvas demo

This page shows the map of `tmaps-canvas` on an HTML canvas. It uses Vite and needs pnpm and Docker. pnpm downloads the pnpm and Node versions that `devEngines` in the root `package.json` gives.

## Start

Run these commands in this folder:

```sh
pnpm install
pnpm dev
```

Vite builds `tmaps-canvas` in Docker before it starts, so the host needs no Rust toolchain. While `dev` runs, a change to `tmaps-canvas`, `tmaps-core`, or the Cargo files builds the wasm again and reloads the page.

To build the page into `dist/` and type-check it, run `pnpm build`. To serve the build, run `pnpm preview`. The `typecheck` script needs the wasm types in `pkg/`, so run `dev` or `build` one time before you use it.

## Docker

The build uses a `tmaps-canvas-wasm` Docker image with the wasm target and the `wasm-bindgen-cli` version from `Cargo.lock`, because the CLI and the crate must have the same version. Docker makes the image again only when one of these versions or `plugins/wasm.Dockerfile` changes. Two Docker volumes, `tmaps-canvas-target` and `tmaps-canvas-registry`, keep the build cache between runs. To keep the cache in a host folder instead, set `TMAPS_CANVAS_CACHE_DIR` to that folder. CI uses this folder to save the cache with `actions/cache`. On Linux, the container writes the files as root, so you need `sudo` to delete them.

Docker keeps the image of each earlier version. To free the disk space, run these commands:

```sh
docker image prune --all --force --filter label=tmaps-canvas-wasm
docker volume rm tmaps-canvas-target tmaps-canvas-registry
```

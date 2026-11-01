import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdirSync, readFileSync } from "node:fs";
import { userInfo } from "node:os";
import { basename, resolve, sep } from "node:path";
import type { Plugin, ResolvedConfig } from "vite";

const REPO = resolve(import.meta.dirname, "../../..");
const OUT_DIR = resolve(import.meta.dirname, "../pkg");
const DOCKERFILE = resolve(import.meta.dirname, "wasm.Dockerfile");
const RUST_SOURCES = ["crates/tmaps-canvas", "crates/tmaps-core", "Cargo.toml", "Cargo.lock"].map((path) =>
  resolve(REPO, path),
);
const CONTAINER_TARGET_DIR = "/tmp/target";

export function wasm(): Plugin {
  let config: ResolvedConfig;
  // A restart of the dev server must stop its build, or the next server starts a second build.
  const stop = new AbortController();
  const logError = (error: unknown) => config.logger.error(String(error), { timestamp: true });
  const build = oneAtATime(async () => {
    const start = performance.now();
    await buildWasm(stop.signal);
    const seconds = ((performance.now() - start) / 1000).toFixed(1);
    config.logger.info(`tmaps-canvas wasm built in ${seconds}s`, { timestamp: config.command === "serve" });
  });
  return {
    name: "tmaps-canvas-wasm",
    configResolved(resolved) {
      config = resolved;
    },
    buildStart() {
      const result = build();
      // A failed first build must not stop the dev server, so that a fix can start a new build.
      return config.command === "serve" ? result?.catch(logError) : result;
    },
    configureServer(server) {
      server.watcher.add(RUST_SOURCES);
    },
    hotUpdate({ file }) {
      if (this.environment.name !== "client" || !isRustSource(file)) return;
      build()?.catch(logError);
    },
    closeBundle() {
      if (config.command === "serve") stop.abort();
    },
  };
}

// A call during a run queues one more run, so that the last change always gets built. Only the
// call that queues a run gets its promise, so that each failure is reported one time.
function oneAtATime(task: () => Promise<void>) {
  let current = Promise.resolve();
  let queued = false;
  return () => {
    if (queued) return;
    queued = true;
    // The failure of a run goes to its caller, so it must not stop the next run.
    current = current.catch(() => {}).then(() => {
      queued = false;
      return task();
    });
    return current;
  };
}

// Editors and the OS write temp files next to the sources, such as .lib.rs.swp and .DS_Store.
function isRustSource(file: string) {
  const inSources = RUST_SOURCES.some((source) => file === source || file.startsWith(source + sep));
  return inSources && (file.endsWith(".rs") || /Cargo\.(toml|lock)$/.test(basename(file)));
}

async function buildWasm(signal: AbortSignal) {
  mkdirSync(OUT_DIR, { recursive: true });
  const image = await buildImage(toolVersions(), signal);
  await run("docker", runArgs(image), { signal });
}

function toolVersions() {
  const read = (file: string) => readFileSync(resolve(REPO, file), "utf8");
  const rust = read("Cargo.toml").match(/^rust-version = "(.+)"$/m)?.[1];
  // The wasm-bindgen CLI must have the same version as the wasm-bindgen crate in Cargo.lock.
  const wasmBindgen = read("Cargo.lock").match(/^name = "wasm-bindgen"\nversion = "(.+)"$/m)?.[1];
  if (!rust || !wasmBindgen) {
    throw new Error("Cargo.toml has no rust-version, or Cargo.lock has no wasm-bindgen");
  }
  return { rust, wasmBindgen };
}

// The image keeps the wasm target and the wasm-bindgen CLI, so that Docker installs them only
// when a version changes.
async function buildImage({ rust, wasmBindgen }: { rust: string; wasmBindgen: string }, signal: AbortSignal) {
  const image = `tmaps-canvas-wasm:rust-${rust}-wasm-bindgen-${wasmBindgen}`;
  const args = [
    "build", "--quiet",
    "--tag", image,
    "--build-arg", `RUST_VERSION=${rust}`,
    "--build-arg", `WASM_BINDGEN_VERSION=${wasmBindgen}`,
    "--file", DOCKERFILE,
    import.meta.dirname,
  ];
  // With --quiet, docker build writes only the image ID to stdout.
  await run("docker", args, { signal, stdout: "ignore" });
  return image;
}

function runArgs(image: string) {
  const { uid, gid } = userInfo();
  const wasmFile = `${CONTAINER_TARGET_DIR}/wasm32-unknown-unknown/release/tmaps_canvas.wasm`;
  const steps = [
    "cargo build --quiet -p tmaps-canvas --target wasm32-unknown-unknown --release --locked",
    `wasm-bindgen --target web --out-dir /out ${wasmFile}`,
    `chown -R ${uid}:${gid} /out`,
  ];
  // The named volumes keep the build cache and the crate downloads between runs. As PID 1, sh
  // ignores SIGINT and SIGTERM, so tini forwards them to the whole process group.
  return [
    "run", "--rm", "--init",
    "-e", "TINI_KILL_PROCESS_GROUP=1",
    "-v", `${REPO}:/src:ro`,
    "-v", `${OUT_DIR}:/out`,
    "-v", `tmaps-canvas-target:${CONTAINER_TARGET_DIR}`,
    "-v", "tmaps-canvas-registry:/usr/local/cargo/registry",
    "-w", "/src",
    "-e", `CARGO_TARGET_DIR=${CONTAINER_TARGET_DIR}`,
    image,
    "sh", "-c", steps.join(" && "),
  ];
}

async function run(
  command: string,
  args: string[],
  { signal, stdout = "inherit" }: { signal: AbortSignal; stdout?: "inherit" | "ignore" },
) {
  const child = spawn(command, args, { signal, stdio: ["ignore", stdout, "inherit"] });
  const [code, exitSignal] = await once(child, "exit");
  if (code !== 0) throw new Error(`${command} stopped with ${exitSignal ?? `exit code ${code}`}`);
}

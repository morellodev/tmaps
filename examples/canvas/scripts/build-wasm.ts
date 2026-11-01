import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { userInfo } from "node:os";

const root = resolve(import.meta.dirname, "../../..");
const out = resolve(import.meta.dirname, "../pkg");
const read = (file: string) => readFileSync(resolve(root, file), "utf8");

const rustVersion = read("Cargo.toml").match(/^rust-version = "(.+)"$/m)?.[1];
// The wasm-bindgen CLI must have the same version as the wasm-bindgen crate in Cargo.lock.
const bindgenVersion = read("Cargo.lock").match(/^name = "wasm-bindgen"\nversion = "(.+)"$/m)?.[1];
if (!rustVersion || !bindgenVersion) {
  throw new Error("Cargo.toml has no rust-version, or Cargo.lock has no wasm-bindgen");
}

const { uid, gid } = userInfo();
const steps = [
  "rustup target add wasm32-unknown-unknown",
  `cargo install wasm-bindgen-cli --version ${bindgenVersion} --locked`,
  "cargo build -p tmaps-canvas --target wasm32-unknown-unknown --release --locked",
  "wasm-bindgen --target web --out-dir /out /tmp/target/wasm32-unknown-unknown/release/tmaps_canvas.wasm",
  `chown -R ${uid}:${gid} /out`,
];

mkdirSync(out, { recursive: true });
// The named volumes keep the build cache and the crate downloads between runs.
execFileSync(
  "docker",
  [
    "run", "--rm",
    "-v", `${root}:/src:ro`,
    "-v", `${out}:/out`,
    "-v", "tmaps-canvas-target:/tmp/target",
    "-v", "tmaps-canvas-registry:/usr/local/cargo/registry",
    "-w", "/src",
    "-e", "CARGO_TARGET_DIR=/tmp/target",
    `rust:${rustVersion}-slim`,
    "sh", "-c", steps.join(" && "),
  ],
  { stdio: "inherit" },
);

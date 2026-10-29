# tmaps

## Commands

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --locked
```

To test the MSRV (`rust-version` in `Cargo.toml`), use a container. Do not install more rustup toolchains.

```sh
docker run --rm -v "$PWD":/src:ro -w /src -e CARGO_TARGET_DIR=/tmp/target rust:<rust-version>-slim cargo test --workspace --locked
```

## Rules

- tmaps targets only Linux and macOS terminals. Write fixes, key handling, and path logic for these two platforms only. Windows-only workarounds, review findings, and test-plan items are out of scope.
- Use Photon for geocoding, not Nominatim. The Nominatim usage policy does not allow autocomplete.
- Providers are concrete types. Add a trait only when a second adapter exists, such as a fake for TUI tests.
- Keep the renderer in `tmaps-core` free of ratatui, so that snapshot tests can use it.
- The CLI output is a contract for agents. When you change a command, a flag, or a JSON document, update `skills/tmaps/SKILL.md` and `skills/tmaps/references/cli-contract.md`.

ARG RUST_VERSION
FROM rust:${RUST_VERSION}-slim
ARG WASM_BINDGEN_VERSION
RUN rustup target add wasm32-unknown-unknown \
 && cargo install wasm-bindgen-cli --version ${WASM_BINDGEN_VERSION} --locked \
 && rm -rf /usr/local/cargo/registry

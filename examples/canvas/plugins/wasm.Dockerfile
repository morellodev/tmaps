ARG RUST_VERSION

# The prebuilt CLI saves the compile of `cargo install`, which takes most of a cold build.
FROM curlimages/curl:8.23.0 AS wasm-bindgen
ARG WASM_BINDGEN_VERSION
WORKDIR /tmp
# The release names its targets like `uname -m`. The checksum file names a path that does not
# exist here, so only its hash is checked.
RUN set -eu \
 && name="wasm-bindgen-${WASM_BINDGEN_VERSION}-$(uname -m)-unknown-linux-musl" \
 && url="https://github.com/wasm-bindgen/wasm-bindgen/releases/download/${WASM_BINDGEN_VERSION}/${name}.tar.gz" \
 && curl --fail --silent --show-error --location --remote-name "${url}" --remote-name "${url}.sha256sum" \
 && printf '%s  %s\n' "$(cut -d ' ' -f 1 "${name}.tar.gz.sha256sum")" "${name}.tar.gz" > checksum \
 && sha256sum -c checksum \
 && tar xzf "${name}.tar.gz" \
 && mv "${name}/wasm-bindgen" /tmp/wasm-bindgen-cli

FROM rust:${RUST_VERSION}-slim
LABEL tmaps-canvas-wasm=""
RUN rustup target add wasm32-unknown-unknown
COPY --from=wasm-bindgen /tmp/wasm-bindgen-cli /usr/local/cargo/bin/wasm-bindgen

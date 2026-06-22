FROM rust:1-bookworm AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        clang \
        cmake \
        npm \
        pkg-config \
        protobuf-compiler \
    && rm -rf /var/lib/apt/lists/* \
    && npm install -g pnpm

WORKDIR /workspace

COPY . .

# Write pnpm packages directly under `node_modules/`, not symlinked into `.pnpm/`.
# `--config.node-linker=hoisted` does this instead of symlinking into `.pnpm/`,
# so the downstream
# `COPY --from=builder /workspace/runtime-deps/node_modules/onnxruntime-web`
# step picks up a real directory.
RUN pnpm add --prod --dir /workspace/runtime-deps --config.node-linker=hoisted \
        --ignore-scripts --config.ignoredBuiltDependencies=protobufjs \
        onnxruntime-web pyodide
RUN cargo build -p et-ws-server --release --locked

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 --user-group app

WORKDIR /app

COPY --from=builder /workspace/target/release/et-ws-server /usr/local/bin/et-ws-server
COPY --from=builder /workspace/services/ws-server/static ./services/ws-server/static
COPY --from=builder /workspace/services/ws-wasm-agent/pkg ./services/ws-wasm-agent/pkg
COPY --from=builder /workspace/services/ws-modules ./services/ws-modules
COPY --from=builder /workspace/data/model-modules ./data/model-modules
COPY --from=builder /workspace/runtime-deps/node_modules/onnxruntime-web ./node_modules/onnxruntime-web
COPY --from=builder /workspace/runtime-deps/node_modules/pyodide ./node_modules/pyodide

RUN mkdir -p /app/storage \
    && chown -R app:app /app

USER app

EXPOSE 8080 8443

CMD ["et-ws-server"]

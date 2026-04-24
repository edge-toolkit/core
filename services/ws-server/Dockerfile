FROM rust:1-bookworm AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        clang \
        cmake \
        npm \
        pkg-config \
        protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace

COPY . .

RUN npm install --omit=dev --prefix /workspace/runtime-deps onnxruntime-web
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

RUN mkdir -p /app/storage \
    && chown -R app:app /app

USER app

EXPOSE 8080 8443

CMD ["et-ws-server"]

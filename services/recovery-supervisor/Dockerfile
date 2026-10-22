# syntax=docker/dockerfile:1

FROM registry.k8s.io/kubectl:v1.35.0 AS kubectl
FROM rust:1-bookworm AS builder
RUN bash <<'EOF'
set -euo pipefail
apt-get update
apt-get install -y --no-install-recommends clang cmake pkg-config
apt-get clean
rm -rf /var/lib/apt/lists/* /var/cache/apt/archives/*.deb
EOF

WORKDIR /workspace
COPY . .
RUN cargo build --release --locked -p et-recovery-supervisor

FROM debian:bookworm-slim
LABEL org.opencontainers.image.source="https://github.com/edge-toolkit/core"
RUN bash <<'EOF'
set -euo pipefail
apt-get update
apt-get install -y --no-install-recommends ca-certificates
apt-get clean
rm -rf /var/lib/apt/lists/* /var/cache/apt/archives/*.deb
useradd --create-home --uid 10001 --user-group app
EOF

COPY --from=kubectl /bin/kubectl /usr/local/bin/kubectl
COPY --from=builder /workspace/target/release/et-recovery-supervisor /usr/local/bin/et-recovery-supervisor
COPY --from=builder /workspace/target/release/et-recovery-healthcheck /usr/local/bin/et-recovery-healthcheck
ENV HOME=/app/runtime
WORKDIR /app
USER 10001
# The service persists its final report on Ctrl-C; use that signal for container shutdown too.
STOPSIGNAL SIGINT
HEALTHCHECK --interval=10s --timeout=3s --start-period=60s --retries=3 CMD ["et-recovery-healthcheck"]
CMD ["et-recovery-supervisor"]

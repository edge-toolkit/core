# pyo3-math1

This directory contains generated deployment configs for the `pyo3-math1` scenario.
Files: `mise.toml`, `compose.yaml`.

The scenario exposes these workflow modules: pyo3-math1, wasi-math1-sender.

`secrets.env` holds the scenario's derived OpenObserve and OTLP credentials, and is deliberately not
committed. Regenerating this scenario writes it; if it is missing, run
`mise run regen-verification` (or `et-cli generate-deployment`) before starting the stack.

## Run With Mise

From this directory, start the scenario with:

```bash
mise run generated-scenario
```

That task starts both OpenObserve and `ws-server` for this scenario.

### Open The OpenObserve UI

From this directory, open the OpenObserve UI with:

```bash
mise run open-o2
```

## Run With Docker Compose

From this directory, start the scenario with:

```bash
docker compose up --build
```

The compose stack starts OpenObserve and builds `ws-server` in two layers: the module-less hub
image from the repository's `services/ws-server/Dockerfile`, then the `Dockerfile` in this
directory, which stages this scenario's modules onto it. The hub is build-only and never runs as a
container of its own.
`ws-server` runs with host networking so it advertises the same LAN IP as the `mise` deployment.

### Open The UIs

OpenObserve is available at <http://localhost:5080/>.
`ws-server` is available at <http://localhost:8080/> and <https://localhost:8443/>.

Stop the scenario with:

```bash
docker compose down
```

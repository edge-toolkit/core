# facility-security-scenario

This directory contains generated deployment configs for the `facility-security-scenario` scenario.
Files: `mise.toml`, `compose.yaml`, `k3s.yaml`.

The scenario exposes these workflow modules: face-detection, har1, pyface1.

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

## Run With k3s

The manifests reference images by name and never build them, so build and import each one first.
This scenario's image layers its modules onto the module-less hub image and takes that hub as a
_named build context_ rather than building it, so the hub has to exist first -- a plain
`docker build` of the scenario Dockerfile fails on `FROM hub`. From the repository root:

```bash
scenario=facility-security-scenario
hub=et-ws-server-hub:latest
image="et-ws-server-$scenario:latest"
dockerfile="verification/local/output/$scenario/Dockerfile"
docker build -t "$hub" -f services/ws-server/Dockerfile .
docker build --build-context "hub=docker-image://$hub" -t "$image" -f "$dockerfile" .
docker save "$image" | sudo k3s ctr images import -
```

Each runner image builds straight from its own `services/ws-<kind>-runner/Dockerfile`, needs no
build context, and is tagged `et-ws-<kind>-runner:latest`.

### Load The Credential

`secrets.env` is generated but deliberately not committed, so the `Secret` is created from it rather
than shipped inside `k3s.yaml`. From this directory:

```bash
ns=et-facility-security-scenario
kubectl create namespace "$ns" --save-config
kubectl create secret generic "$ns-secrets" --from-env-file=secrets.env -n "$ns"
```

### Apply

```bash
kubectl apply -f k3s.yaml
```

The runners exit and restart until the hub reports ready, so `CrashLoopBackOff` while the hub
starts is expected here rather than a fault. Watch it settle with:

```bash
kubectl get pods -n "$ns" --watch
```

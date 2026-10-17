# math1-cron

This directory contains generated deployment configs for the `math1-cron` scenario.
Files: `compose.yaml`, `k3s.yaml`, `mise.toml`, `process-compose.yaml`.

The scenario exposes these workflow modules: math1, math1-sender.

`secrets.env` holds the scenario's derived OpenObserve and OTLP credentials. It is derived from the
scenario input, so regenerating this deployment rewrites it; if it is missing, regenerate before
starting the stack. A deployment generated outside the repository is written with a `.gitignore`
covering it, so its credential is not committed by whatever repository it lands in.

## Run With Docker Compose

From this directory, start the scenario with:

```bash
docker compose up --build
```

The compose stack starts OpenObserve and builds `ws-server` in two layers: the module-less hub
image from the repository's `services/ws-server/Dockerfile`, then the `Dockerfile` in this
directory, which stages this scenario's modules onto it.
`ws-server` runs with host networking so it advertises the same LAN IP as the `mise` deployment.

### Open The UIs

OpenObserve is available at <http://localhost:5080/>.
`ws-server` is available at <http://localhost:8080/> and <https://localhost:8443/>.

Stop the scenario with:

```bash
docker compose down
```

### Scheduled Agents With Docker Compose

`docker compose up` runs each scheduled agent's runner once, as it does every service. The
`scheduler` service, Ofelia, then starts that same container again on each tick of its schedule,
skipping a tick while the previous run is still going. It mounts the Docker socket to do so, which
gives it the Docker daemon's privileges.

## Run With k3s

The manifests reference images by name and never build them, so build and import each one first.
This scenario's image layers its modules onto the module-less hub image and takes that hub as a
_named build context_ rather than building it, so the hub has to exist first -- a plain
`docker build` of the scenario Dockerfile fails on `FROM hub`. From the repository root:

```bash
scenario=math1-cron
hub=et-ws-server-hub:latest
image="et-ws-server-$scenario:latest"
dockerfile="verification/local/output/$scenario/Dockerfile"
docker build -t "$hub" -f services/ws-server/Dockerfile .
docker build --build-context "hub=docker-image://$hub" -t "$image" -f "$dockerfile" .
docker save "$image" | sudo k3s ctr images import -
```

Then this scenario's runner images, which need no build context of their own:

```bash
docker build -t et-ws-web-runner:latest -f services/ws-web-runner/Dockerfile .
docker save et-ws-web-runner:latest | sudo k3s ctr images import -
```

### Load The Credential

The credential reaches the pods as a `Secret` created from `secrets.env`, rather than written into
`k3s.yaml` where it would be committed alongside the manifests. From this directory:

```bash
ns=et-math1-cron
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

## Run With process-compose

From this directory, install the process-compose release `mise.toml` pins, then start the scenario
and watch each process with:

```bash
mise install
process-compose up --no-server -f process-compose.yaml
```

Each process runs one task of `mise.toml`, in order: `ws-server` waits for OpenObserve to report
healthy, and every runner waits for `ws-server`. A scheduled agent's runner is started on each
tick of its schedule and runs its module once; a tick that finds the previous run still going is
skipped. `--no-server` keeps process-compose's own API off port 8080, where `ws-server` listens.
Quitting the TUI stops every process. Open the OpenObserve UI, once it is up, by starting the
`open-o2` process from the TUI.

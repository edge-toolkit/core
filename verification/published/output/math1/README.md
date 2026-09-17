# math1

This directory contains generated deployment configs for the `math1` scenario.
Files: `mise.toml`, `compose.yaml`, `k3s.yaml`.

The scenario exposes these workflow modules: math1, math1-sender.

This scenario is rendered against released artifacts rather than builds of this repository:
the images it names are published, and the binaries it runs are the released ones. Only the
image carrying its own module set is still built locally, since no release can hold a module
set particular to one deployment.

`secrets.env` holds the scenario's derived OpenObserve and OTLP credentials. It is derived from the
scenario input, so regenerating this deployment rewrites it; if it is missing, regenerate before
starting the stack. A deployment generated outside the repository is written with a `.gitignore`
covering it, so its credential is not committed by whatever repository it lands in.

## Run With Mise

Fetch the binaries the tasks below name before the first run:

```bash
mise install
```

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

The compose stack starts OpenObserve and builds one image: the `Dockerfile` in this directory,
which stages this scenario's modules onto the released hub image it names as a build context.
`ws-server` runs with host networking so it advertises the same LAN IP as the `mise` deployment.

### Open The UIs

OpenObserve is available at <http://localhost:5080/>.
`ws-server` is available at <http://localhost:8080/> and <https://localhost:8443/>.

Stop the scenario with:

```bash
docker compose down
```

## Run With k3s

The manifests reference images by name and never build them, so this scenario's own image has
to reach the node first. It layers its modules onto the module-less hub image and takes that
hub as a _named build context_ rather than building it, so a plain `docker build` of the
scenario Dockerfile fails on `FROM hub` -- pointing the context at the published hub is what
supplies it. From the repository root:

```bash
scenario=math1
hub=ghcr.io/edge-toolkit/core/et-ws-server:latest
image="et-ws-server-$scenario:latest"
dockerfile="verification/published/output/$scenario/Dockerfile"
docker build --build-context "hub=docker-image://$hub" -t "$image" -f "$dockerfile" .
docker save "$image" | sudo k3s ctr images import -
```

The cluster pulls this scenario's runner images, so nothing has to be built or imported for
them:

- `ghcr.io/edge-toolkit/core/et-ws-web-runner:latest`

### Load The Credential

The credential reaches the pods as a `Secret` created from `secrets.env`, rather than written into
`k3s.yaml` where it would be committed alongside the manifests. From this directory:

```bash
ns=et-math1
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

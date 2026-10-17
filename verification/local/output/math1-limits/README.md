# math1-limits

This directory contains generated deployment configs for the `math1-limits` scenario.
Files: `mise.toml`, `compose.yaml`, `k3s.yaml`, `dekit.yaml`.

The scenario exposes these workflow modules: math1, math1-sender.

`secrets.env` holds the scenario's derived OpenObserve and OTLP credentials. It is derived from the
scenario input, so regenerating this deployment rewrites it; if it is missing, regenerate before
starting the stack. A deployment generated outside the repository is written with a `.gitignore`
covering it, so its credential is not committed by whatever repository it lands in.

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

### Limits Not Applied With Mise

Each process runs directly on the host rather than in a container, so the limits the scenario
sets for these are not applied; the compose and k3s deployments enforce them:

- `openobserve`
- `ws-server`
- `math1-twin`

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

## Run With k3s

The manifests reference images by name and never build them, so build and import each one first.
This scenario's image layers its modules onto the module-less hub image and takes that hub as a
_named build context_ rather than building it, so the hub has to exist first -- a plain
`docker build` of the scenario Dockerfile fails on `FROM hub`. From the repository root:

```bash
scenario=math1-limits
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

### Prepare The Nodes

Some of this scenario's limits have no field in the Pod API, so `k3s.yaml` runs these container
runtime plugins on every node to write them into each container's cgroup:
`nri-memory-qos` and `ulimit-adjuster`.

Each limited pod names the plugins it needs, and containerd refuses it on a node where one of them
did not adjust it -- once NRI's validator is on, which k3s leaves off. Before applying `k3s.yaml`,
copy the template below from this directory to every machine of the cluster and restart k3s there
(`k3s-agent` on a machine that joined as an agent):

```bash
sudo install -D -m 0644 containerd-config-v3.toml.tmpl /var/lib/rancher/k3s/agent/etc/containerd/config-v3.toml.tmpl
sudo systemctl restart k3s
```

### Load The Credential

The credential reaches the pods as a `Secret` created from `secrets.env`, rather than written into
`k3s.yaml` where it would be committed alongside the manifests. From this directory:

```bash
ns=et-math1-limits
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

## Run With dekit

From this directory, start the scenario and watch each process in its own pane with:

```bash
dekit up
dekit attach
```

Each task runs one task of `mise.toml`, so these are the processes `mise run generated-scenario`
starts, but in order: `ws-server` waits for OpenObserve to report healthy, and every runner
waits for `ws-server`. The tasks keep running after the terminal closes; stop them with:

```bash
dekit down
```

Open the OpenObserve UI, once it is up, with:

```bash
dekit start open-o2
```

### Limits Not Applied With dekit

Each process runs directly on the host rather than in a container, so the limits the scenario
sets for these are not applied; the compose and k3s deployments enforce them:

- `openobserve`
- `ws-server`
- `math1-twin`

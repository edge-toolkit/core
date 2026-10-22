# pyo3-math1-recovery

This directory contains the generated `k3s.yaml` for the `pyo3-math1-recovery` scenario.

The scenario exposes these workflow modules: pyo3-math1.

`secrets.env` holds the scenario's derived OpenObserve and OTLP credentials. It is derived from the
scenario input, so regenerating this deployment rewrites it; if it is missing, regenerate before
starting the stack. A deployment generated outside the repository is written with a `.gitignore`
covering it, so its credential is not committed by whatever repository it lands in.

## Run With k3s

The manifests reference images by name and never build them, so build and import each one first.
This scenario's image layers its modules onto the module-less hub image and takes that hub as a
_named build context_ rather than building it, so the hub has to exist first -- a plain
`docker build` of the scenario Dockerfile fails on `FROM hub`. From the repository root:

```bash
scenario=pyo3-math1-recovery
hub=et-ws-server-hub:latest
image="et-ws-server-$scenario:latest"
dockerfile="verification/local/output/$scenario/Dockerfile"
docker build -t "$hub" -f services/ws-server/Dockerfile .
docker build --build-context "hub=docker-image://$hub" -t "$image" -f "$dockerfile" .
docker save "$image" | sudo k3s ctr images import -
```

Then this scenario's runner images, which need no build context of their own:

```bash
docker build -t et-ws-pyo3-runner:latest -f services/ws-pyo3-runner/Dockerfile .
docker save et-ws-pyo3-runner:latest | sudo k3s ctr images import -
```

Build and import the recovery service image from the repository root:

```bash
image=et-recovery-supervisor:latest
docker build -t "$image" -f services/recovery-supervisor/Dockerfile .
docker save "$image" | sudo k3s ctr images import -
```

The supervisor starts automatically with scoped ServiceAccount credentials and persistent state.
Application modules must implement lifecycle reporting and any required checkpoint restoration.

### Load The Credential

The credential reaches the pods as a `Secret` created from `secrets.env`, rather than written into
`k3s.yaml` where it would be committed alongside the manifests. From this directory:

```bash
ns=et-pyo3-math1-recovery
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

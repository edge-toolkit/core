# Vessel FL prototype

`mise.prototype.toml` adds the demo-only dependency setup and local MLflow
process that the generated `mise.toml` does not yet model. Regeneration
preserves both prototype files.

From the repository root:

```sh
mise -C verification/local/output/vessel-fl-demo -E prototype run setup-python
mise -C verification/local/output/vessel-fl-demo -E prototype run check-demo
```

Run the hub and the native demo processes in separate terminals:

```sh
mise -C verification/local/output/vessel-fl-demo -E prototype run ws-server
mise -C verification/local/output/vessel-fl-demo -E prototype run demo
```

The standard config-based scenario is defined by
`../../input/vessel-fl-demo.yaml`. It demonstrates reusable runner instances;
the prototype environment remains useful while MLflow and the dataset are
host-local demo dependencies.

## Docker Compose

The prototype override derives a CPU-only Pyo3 image, mounts the prepared
dataset read-only, and adds a loopback-only MLflow server with persistent
storage. From this directory:

```sh
export VESSEL_FL_DATA_ROOT="$HOME/Datasets/croatia-postprocessed/croatia_1g_logmel_images"
docker compose -f compose.yaml -f compose.prototype.yaml up --build
```

Open Edge Toolkit at <http://127.0.0.1:8080/> and MLflow at
<http://127.0.0.1:5000/>. The first image build downloads the pinned CPU
PyTorch, Pillow, and MLflow packages and can take several minutes. Stop and
remove the containers with:

```sh
docker compose -f compose.yaml -f compose.prototype.yaml down
```

Add `--volumes` only when the MLflow history and generated model storage should
also be deleted.

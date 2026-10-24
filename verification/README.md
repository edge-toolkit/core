# Verification scenarios

Scenario inputs and the deployments `et-cli` generates from them, committed so that any change to a generator shows
up as a diff here.

- `local/` renders each scenario against builds of this tree, and `published/` renders the same scenarios against
  the released artifacts.
- `<set>/input/<name>.yaml` is one scenario, and its header comment says what it covers.
- `<set>/output/<name>/` is every format generated from it, with a `README.md` on running each. A scheduled scenario
  has no `mise.toml` or `dekit.yaml`, which cannot schedule, and a `recovery:` one is k3s only.

`mise run regen-verification` rewrites every output, and `mise run verification-check` fails when a committed one
has drifted from what its input generates.

The math1-shaped scenarios also run end to end, each checking the model the twin stores:

- `utilities/cli/tests/scenario_runners.rs` runs the generated mise runner tasks against an in-process hub.
- `mise run k3s-verify [local|published] --scenario <name>` applies `k3s.yaml` to a throwaway k3d cluster with one
  node per machine, and checks each pod landed on its machine's node. It expects the images built first.
- `mise run compose-verify --scenario <name>` brings `compose.yaml` up one machine profile at a time.

Both also check a scenario's resource limits in the running containers when it sets any.

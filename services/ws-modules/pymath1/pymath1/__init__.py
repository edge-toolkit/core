"""math1 twin in Python (Pyodide): a storage-driven FedAvg simulation.

The JS shim hands this module the raw input JSON it fetched from ws-server storage (client
datasets + hyperparameters, injected by the test harness's fake agent). fed_avg() runs rounds of
local full-batch gradient-descent epochs per client and merges the local models with a
sample-count-weighted average. Only + - * / on floats (no math-module calls), so the result is
bit-identical to the other math1 language twins. run() returns the output JSON the shim stores to
this agent's bucket for the harness to verify.
"""

from __future__ import annotations

import json
from collections.abc import Callable


def fed_avg(clients: list[list[list[float]]], rounds: int, epochs: int, learning_rate: float) -> tuple[float, float]:
    """Run the FedAvg simulation and return the final global (weight, bias)."""
    weight = 0.0
    bias = 0.0
    total_samples = 0.0
    for samples in clients:
        total_samples += float(len(samples))
    for _ in range(rounds):
        merged_weight = 0.0
        merged_bias = 0.0
        for samples in clients:
            count = float(len(samples))
            client_weight = weight
            client_bias = bias
            for _ in range(epochs):
                grad_weight = 0.0
                grad_bias = 0.0
                for sample in samples:
                    residual = client_weight * sample[0] + client_bias - sample[1]
                    grad_weight += residual * sample[0]
                    grad_bias += residual
                client_weight -= learning_rate * (2.0 * grad_weight / count)
                client_bias -= learning_rate * (2.0 * grad_bias / count)
            merged_weight += client_weight * count
            merged_bias += client_bias * count
        weight = merged_weight / total_samples
        bias = merged_bias / total_samples
    return weight, bias


def run(agent_id: str, input_json: str, log: Callable[[str], None]) -> str:
    """Run FedAvg on the fetched input and return the output JSON for the shim to store."""
    log(f"[pymath1] connected as {agent_id}")
    params = json.loads(input_json)
    clients = params["clients"]
    rounds = params["rounds"]
    epochs = params["epochs"]
    learning_rate = params["learning_rate"]
    log(f"[pymath1] running FedAvg - {len(clients)} clients x {rounds} rounds x {epochs} local epochs")
    weight, bias = fed_avg(clients, rounds, epochs, learning_rate)
    log(f"[pymath1] global model weight={weight!r} bias={bias!r}")
    return json.dumps({"module": "pymath1", "weight": weight, "bias": bias})

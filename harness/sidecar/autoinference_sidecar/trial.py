"""One Trial in the sidecar: launch → warm → measure N repeats → stop. Returns raw repeats;
the Rust side owns summarization, caching, the ledger and events."""
from __future__ import annotations

import os
import time
from typing import Any

from . import bench, loadgen
from .engines import LaunchSpec, MockEngine, get_backend, mock_cost_per_1m_tok


def run_trial(engine: str, model: str, sku: str, config: dict[str, Any], workload: dict[str, Any], repeats: int, timeout_s: int) -> dict[str, Any]:
    backend = get_backend(engine)
    spec = LaunchSpec(engine=engine, model=model, config=dict(config or {}))
    log_dir = os.path.join(os.environ.get("AUTOINFERENCE_DATA_DIR", os.path.expanduser("~/.autoinference")), "logs", "engines")
    t0 = time.time()
    handle = backend.launch(spec, timeout_s=timeout_s, log_dir=log_dir)
    samples: list[dict[str, float]] = []
    try:
        if isinstance(backend, MockEngine):
            for i in range(max(1, repeats)):
                samples.append(backend.measure(handle, sku, workload, i))
        else:
            tier = str(workload.get("loadgen", "quick"))
            if tier == "quick":
                warm = dict(workload)
                warm["requests"] = max(4, int(workload.get("concurrency", 32)) // 2)
                bench.run_load(handle.base_url, model, warm)  # warm-up: discarded (engine/aiperf tiers warm themselves)
            art = os.path.join(log_dir, "..", "artifacts", "loadgen")
            for _ in range(max(1, repeats)):
                samples.append(loadgen.run(tier, engine, handle.base_url, model, workload, art))
    finally:
        backend.stop(handle)
    out: dict[str, Any] = {
        "engine": engine,
        "engine_version": handle.version,
        "gpu_count": handle.gpu_count,
        "command": handle.command,
        "log_path": handle.log_path,
        "repeats": samples,
        "loadgen": "mock" if isinstance(backend, MockEngine) else str(workload.get("loadgen", "quick")),
        "wall_s": time.time() - t0,
    }
    if isinstance(backend, MockEngine) and samples:
        med = sorted(s["tok_s"] for s in samples)[len(samples) // 2]
        out["mock_cost_per_1m_tok"] = mock_cost_per_1m_tok(sku, med, handle.gpu_count)
    return out

"""Tiered load generators. All three return the same sample shape so trials are comparable.

| tier     | tool                                   | when                                             |
|----------|----------------------------------------|--------------------------------------------------|
| quick    | built-in stdlib generator (bench.py)   | every commit / every candidate — seconds          |
| engine   | `vllm bench serve` / `sglang.bench_serving` | closer to the engine's own reference numbers |
| aiperf   | NVIDIA AIPerf (`aiperf profile`)       | robust stress runs: percentiles, arrival patterns, GPU power/util telemetry |

AIPerf: https://developer.nvidia.com/blog/benchmarking-llm-inference-at-scale-with-aiperf/
Metric tags parsed from its JSON export: output_token_throughput, time_to_first_token,
inter_token_latency, request_latency, request_throughput, nvidia_average_gpu_power (when DCGM/pynvml
is available). Flags verified against the mirrored source (inference-engine-KB/repos/aiperf).
"""
from __future__ import annotations

import glob
import json
import os
import shutil
import subprocess
import sys
import time
from typing import Any

from . import bench

TIERS = ("quick", "engine", "aiperf")


class LoadgenUnavailable(RuntimeError):
    pass


def run(tier: str, engine: str, base_url: str, model: str, workload: dict[str, Any], artifact_dir: str) -> dict[str, float]:
    if tier == "quick":
        return bench.run_load(base_url, model, workload)
    if tier == "engine":
        return _engine_bench(engine, base_url, model, workload, artifact_dir)
    if tier == "aiperf":
        return _aiperf(base_url, model, workload, artifact_dir)
    raise ValueError(f"unknown loadgen tier `{tier}`; expected one of {TIERS}")


# ---- engine-native benches ------------------------------------------------------------------

def _engine_bench(engine: str, base_url: str, model: str, workload: dict[str, Any], artifact_dir: str) -> dict[str, float]:
    os.makedirs(artifact_dir, exist_ok=True)
    conc = int(workload.get("concurrency", 32))
    n = int(workload.get("requests", 128))
    ptoks = int(workload.get("prompt_tokens", 512))
    otoks = int(workload.get("output_tokens", 128))
    out_json = os.path.join(artifact_dir, f"{engine}-bench-{int(time.time())}.json")
    if engine == "vllm":
        exe = shutil.which("vllm")
        if not exe:
            raise LoadgenUnavailable("`vllm` not on PATH (engine tier needs the engine's own bench tool)")
        cmd = [
            exe, "bench", "serve", "--backend", "vllm", "--base-url", base_url, "--model", model,
            "--dataset-name", "random", "--random-input-len", str(ptoks), "--random-output-len", str(otoks),
            "--num-prompts", str(n), "--max-concurrency", str(conc), "--ignore-eos", "--seed", "42",
            "--num-warmups", str(max(4, conc // 2)), "--percentile-metrics", "ttft,tpot,itl,e2el",
            "--metric-percentiles", "50,90,99", "--save-result", "--result-dir", artifact_dir,
            "--result-filename", os.path.basename(out_json), "--disable-tqdm",
        ]
    elif engine == "sglang":
        if subprocess.run([sys.executable, "-c", "import sglang"], capture_output=True).returncode != 0:
            raise LoadgenUnavailable("`sglang` not importable in the sidecar interpreter (engine tier needs sglang.bench_serving)")
        cmd = [
            sys.executable, "-m", "sglang.bench_serving", "--backend", "sglang", "--base-url", base_url, "--model", model,
            "--dataset-name", "random", "--random-input-len", str(ptoks), "--random-output-len", str(otoks),
            "--num-prompts", str(n), "--max-concurrency", str(conc), "--seed", "42", "--output-file", out_json, "--disable-tqdm",
        ]
    else:
        raise LoadgenUnavailable(f"engine tier has no bench tool for `{engine}`")
    t0 = time.perf_counter()
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=3600)
    if r.returncode != 0:
        raise RuntimeError(f"{engine} bench failed ({r.returncode}): {r.stderr[-800:]}")
    wall = time.perf_counter() - t0
    data = _read_last_json(out_json)
    return {
        "tok_s": float(data.get("output_throughput", 0.0)),
        "ttft_p50_ms": float(data.get("median_ttft_ms", 0.0)),
        "ttft_p99_ms": float(data.get("p99_ttft_ms", 0.0)),
        "tpot_ms": float(data.get("median_tpot_ms", data.get("median_itl_ms", 0.0))),
        "gpu_util": 0.0,
        "mem_bw_util": 0.0,
        "duration_s": float(data.get("duration", wall)),
        "loadgen": "engine",
        "artifact": out_json,
    }


def _read_last_json(path: str) -> dict[str, Any]:
    """`vllm bench serve --append-result` and sglang may write JSON-lines; take the last object."""
    with open(path) as f:
        text = f.read().strip()
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        last = [ln for ln in text.splitlines() if ln.strip()][-1]
        return json.loads(last)


# ---- AIPerf --------------------------------------------------------------------------------

def _aiperf(base_url: str, model: str, workload: dict[str, Any], artifact_dir: str) -> dict[str, float]:
    exe = shutil.which("aiperf")
    if not exe:
        raise LoadgenUnavailable("`aiperf` not on PATH — install with `uv tool install aiperf` (or pip install aiperf)")
    os.makedirs(artifact_dir, exist_ok=True)
    conc = int(workload.get("concurrency", 32))
    n = int(workload.get("requests", 128))
    ptoks = int(workload.get("prompt_tokens", 512))
    otoks = int(workload.get("output_tokens", 128))
    arrival = str(workload.get("arrival", "constant"))  # constant | poisson | gamma
    run_dir = os.path.join(artifact_dir, f"aiperf-{int(time.time())}")
    cmd = [
        exe, "profile", "--model", model, "--url", base_url, "--endpoint-type", str(workload.get("endpoint_type", "chat")),
        "--streaming", "--concurrency", str(conc), "--request-count", str(n),
        "--warmup-request-count", str(int(workload.get("warmup_requests", max(4, conc // 2)))),
        "--synthetic-input-tokens-mean", str(ptoks), "--synthetic-input-tokens-stddev", str(int(workload.get("prompt_tokens_stddev", 0))),
        "--output-tokens-mean", str(otoks), "--output-tokens-stddev", str(int(workload.get("output_tokens_stddev", 0))),
        "--extra-inputs", "ignore_eos:true", "--random-seed", "42", "--artifact-dir", run_dir, "--ui-type", "simple",
    ]
    if arrival != "constant":
        cmd += ["--arrival-pattern", arrival]
    if workload.get("request_rate"):
        cmd += ["--request-rate", str(workload["request_rate"])]
    if workload.get("tokenizer"):
        cmd += ["--tokenizer", str(workload["tokenizer"])]
    t0 = time.perf_counter()
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=7200)
    if r.returncode != 0:
        raise RuntimeError(f"aiperf failed ({r.returncode}): {r.stderr[-800:]}")
    wall = time.perf_counter() - t0
    files = glob.glob(os.path.join(run_dir, "**", "profile_export_aiperf.json"), recursive=True)
    if not files:
        raise RuntimeError(f"aiperf produced no profile_export_aiperf.json under {run_dir}")
    with open(files[0]) as f:
        data = json.load(f)
    m = _aiperf_metrics(data)
    return {
        "tok_s": m.get("output_token_throughput", {}).get("avg", 0.0),
        "ttft_p50_ms": _ms(m.get("time_to_first_token", {}), "p50"),
        "ttft_p99_ms": _ms(m.get("time_to_first_token", {}), "p99"),
        "tpot_ms": _ms(m.get("inter_token_latency", {}), "p50"),
        "gpu_util": _pct(m.get("nvidia_gpu_utilization", m.get("gpu_utilization", {}))),
        "mem_bw_util": 0.0,
        "duration_s": m.get("benchmark_duration", {}).get("avg", wall),
        "loadgen": "aiperf",
        "artifact": files[0],
        "request_latency_p99_ms": _ms(m.get("request_latency", {}), "p99"),
        "gpu_power_w": m.get("nvidia_average_gpu_power", {}).get("avg", 0.0),
        "request_throughput": m.get("request_throughput", {}).get("avg", 0.0),
    }


_WANT = {
    "output_token_throughput", "time_to_first_token", "inter_token_latency", "request_latency", "request_throughput",
    "benchmark_duration", "nvidia_average_gpu_power", "nvidia_gpu_utilization", "gpu_utilization", "total_token_throughput",
}


def _aiperf_metrics(data: Any) -> dict[str, dict[str, Any]]:
    """Find metric records by tag anywhere in the export (schema-tolerant): returns
    {tag: {avg, p50, p90, p99, unit, ...}}."""
    found: dict[str, dict[str, Any]] = {}

    def visit(node: Any, key_hint: str | None = None) -> None:
        if isinstance(node, dict):
            tag = node.get("tag") or key_hint
            if tag in _WANT and any(k in node for k in ("avg", "p50", "p99", "value")):
                rec = dict(node)
                if "value" in rec and "avg" not in rec:
                    rec["avg"] = rec["value"]
                found.setdefault(tag, rec)
            for k, v in node.items():
                visit(v, k if isinstance(v, dict) else None)
        elif isinstance(node, list):
            for v in node:
                visit(v)

    visit(data)
    return found


def _ms(rec: dict[str, Any], stat: str) -> float:
    v = rec.get(stat, rec.get("avg", 0.0)) or 0.0
    unit = str(rec.get("unit", "ms")).lower()
    if unit in ("s", "sec", "seconds"):
        return float(v) * 1000.0
    if unit in ("ns", "nanoseconds"):
        return float(v) / 1e6
    if unit in ("us", "µs", "microseconds"):
        return float(v) / 1e3
    return float(v)


def _pct(rec: dict[str, Any]) -> float:
    v = float(rec.get("avg", 0.0) or 0.0)
    return v / 100.0 if v > 1.0 else v

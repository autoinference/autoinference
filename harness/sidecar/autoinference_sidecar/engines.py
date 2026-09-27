"""Engine backends: render a launch command from registry-named knobs, start the server,
wait for health, stop it. `MockEngine` is a synthetic roofline model so the whole trial
loop runs on a laptop; its numbers are never real and are labelled as such.
"""
from __future__ import annotations

import hashlib
import json
import math
import os
import random
import shutil
import subprocess
import time
import urllib.request
from dataclasses import dataclass, field
from typing import Any

# Datasheet priors used only by the mock model (the Rust side owns the real SKU table).
_SKU = {
    "a100-40gb": (1555, 108, 312), "a100-80gb": (2039, 108, 312),
    "h100-sxm": (3350, 132, 989), "h100-pcie": (2000, 114, 756), "h200-sxm": (4800, 132, 989),
    "b200": (8000, 148, 2250), "b300": (8000, 160, 2250), "gb200-nvl72": (8000, 148, 2250),
    "rtx-pro-6000-blackwell": (1792, 188, 1000),
}


def _model_size_b(model: str) -> float:
    """Best-effort parameter count from the model name (e.g. 'Llama-3.1-70B' → 70)."""
    import re

    m = re.search(r"(\d+(?:\.\d+)?)\s*[bB](?![a-zA-Z])", model)
    return float(m.group(1)) if m else 8.0


@dataclass
class LaunchSpec:
    engine: str
    model: str
    config: dict[str, Any]
    port: int = 18000


@dataclass
class Handle:
    engine: str
    version: str
    base_url: str
    proc: subprocess.Popen | None = None
    command: list[str] = field(default_factory=list)
    log_path: str | None = None
    gpu_count: int = 1
    mock: dict[str, Any] | None = None


class Backend:
    name = "base"

    def command(self, spec: LaunchSpec) -> list[str]:
        raise NotImplementedError

    def version(self) -> str:
        return "unknown"

    def launch(self, spec: LaunchSpec, timeout_s: int, log_dir: str) -> Handle:
        cmd = self.command(spec)
        if shutil.which(cmd[0]) is None:
            raise RuntimeError(f"`{cmd[0]}` not found on PATH — install {self.name} in this environment (the sidecar runs inside your engine venv)")
        os.makedirs(log_dir, exist_ok=True)
        log_path = os.path.join(log_dir, f"{self.name}-{int(time.time())}.log")
        log = open(log_path, "ab")  # noqa: SIM115
        proc = subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        base = f"http://127.0.0.1:{spec.port}"
        deadline = time.time() + timeout_s
        while time.time() < deadline:
            if proc.poll() is not None:
                raise RuntimeError(f"{self.name} exited early (code {proc.returncode}); see {log_path}")
            try:
                with urllib.request.urlopen(base + "/health", timeout=2) as r:  # noqa: S310
                    if r.status == 200:
                        break
            except Exception:  # noqa: BLE001
                time.sleep(2)
        else:
            proc.kill()
            raise TimeoutError(f"{self.name} did not become healthy in {timeout_s}s; see {log_path}")
        tp = int(spec.config.get("tensor-parallel-size", spec.config.get("tp-size", spec.config.get("tp", 1))) or 1)
        return Handle(self.name, self.version(), base, proc, cmd, log_path, gpu_count=tp)

    @staticmethod
    def stop(h: Handle) -> None:
        if h.proc and h.proc.poll() is None:
            try:
                os.killpg(os.getpgid(h.proc.pid), 15)
                h.proc.wait(timeout=30)
            except Exception:  # noqa: BLE001
                h.proc.kill()


def _flag(name: str) -> str:
    return name if name.startswith("-") else "--" + name


def _argv_from_config(config: dict[str, Any]) -> list[str]:
    out: list[str] = []
    for k, v in config.items():
        if v is None or v is False:
            continue
        if v is True:
            out.append(_flag(k))
        elif isinstance(v, (dict, list)):
            out += [_flag(k), json.dumps(v)]
        else:
            out += [_flag(k), str(v)]
    return out


class VllmBackend(Backend):
    name = "vllm"

    def command(self, spec: LaunchSpec) -> list[str]:
        exe = shutil.which("vllm") or "vllm"
        return [exe, "serve", spec.model, "--port", str(spec.port), "--disable-log-requests", *_argv_from_config(spec.config)]

    def version(self) -> str:
        try:
            return subprocess.run(["python", "-c", "import vllm;print(vllm.__version__)"], capture_output=True, text=True, timeout=30).stdout.strip() or "unknown"
        except Exception:  # noqa: BLE001
            return "unknown"


class SglangBackend(Backend):
    name = "sglang"

    def command(self, spec: LaunchSpec) -> list[str]:
        return ["python", "-m", "sglang.launch_server", "--model-path", spec.model, "--port", str(spec.port), *_argv_from_config(spec.config)]

    def version(self) -> str:
        try:
            return subprocess.run(["python", "-c", "import sglang;print(sglang.__version__)"], capture_output=True, text=True, timeout=30).stdout.strip() or "unknown"
        except Exception:  # noqa: BLE001
            return "unknown"


class MockEngine(Backend):
    """Synthetic roofline: decode is HBM-bound on weight reads; batching amortizes it up to a
    compute cap; TP splits weights but adds sync overhead; fp8 KV halves cache traffic.
    Deterministic per (config, repeat) with mild noise so `noisy` detection has something to see.
    """

    name = "mock"

    def command(self, spec: LaunchSpec) -> list[str]:
        return ["mock-engine", spec.model, *_argv_from_config(spec.config)]

    def version(self) -> str:
        return "mock-1"

    def launch(self, spec: LaunchSpec, timeout_s: int, log_dir: str) -> Handle:
        tp = int(spec.config.get("tensor-parallel-size", spec.config.get("tp", 1)) or 1)
        return Handle("mock", "mock-1", "mock://", None, self.command(spec), None, gpu_count=tp, mock={"spec": spec})

    def measure(self, h: Handle, sku: str, workload: dict[str, Any], repeat: int) -> dict[str, float]:
        spec: LaunchSpec = h.mock["spec"]
        cfg = spec.config
        bw, sms, tflops = _SKU.get(sku, (3350, 132, 989))
        params_b = _model_size_b(spec.model)
        dtype = str(cfg.get("dtype", "bf16")).lower()
        bytes_per_param = 1.0 if "fp8" in dtype or str(cfg.get("quantization", "")).lower() in ("fp8", "nvfp4", "int8", "awq", "gptq") else 2.0
        tp = max(1, int(cfg.get("tensor-parallel-size", cfg.get("tp", 1)) or 1))
        max_seqs = int(cfg.get("max-num-seqs", 256) or 256)
        conc = min(int(workload.get("concurrency", 32)), max_seqs)
        kv_fp8 = "fp8" in str(cfg.get("kv-cache-dtype", "auto")).lower()
        # Time per decode step: read all weights once per step (split across TP), plus KV traffic.
        weight_bytes = params_b * 1e9 * bytes_per_param / tp
        kv_bytes = conc * 2 * 32 * 4096 * (1 if kv_fp8 else 2) * 0.5  # crude per-step KV read
        step_s = (weight_bytes + kv_bytes) / (bw * 1e9) * (1 + 0.08 * (tp - 1))
        compute_cap_tok_s = tflops * 1e12 / (2 * params_b * 1e9 / tp) * 0.35
        tok_s = min(conc / step_s, compute_cap_tok_s)
        seed = int(hashlib.sha256(f"{json.dumps(cfg, sort_keys=True)}|{sku}|{repeat}".encode()).hexdigest()[:8], 16)
        rng = random.Random(seed)
        noise = 1 + rng.gauss(0, 0.03) + (0.12 * rng.random() if cfg.get("_thermal_throttle") else 0)
        tok_s *= noise
        tpot_ms = 1000.0 / (tok_s / conc)
        prefill_tok = int(workload.get("prompt_tokens", 512)) * conc
        ttft_ms = prefill_tok * 2 * params_b * 1e9 / tp / (tflops * 1e12 * 0.5) * 1000 * (1 + rng.random() * 0.2) + 15
        gpu_util = min(0.98, 0.35 + 0.6 * min(1.0, conc / 128))
        mem_bw_util = min(0.95, step_s * (bw * 1e9) / (weight_bytes + kv_bytes) * 0.85)
        return {
            "tok_s": tok_s,
            "ttft_p50_ms": ttft_ms,
            "ttft_p99_ms": ttft_ms * (1.6 + rng.random() * 0.4),
            "tpot_ms": tpot_ms,
            "gpu_util": gpu_util,
            "mem_bw_util": mem_bw_util,
            "duration_s": 0.0,
        }


BACKENDS: dict[str, Backend] = {"vllm": VllmBackend(), "sglang": SglangBackend(), "mock": MockEngine()}


def get_backend(name: str) -> Backend:
    try:
        return BACKENDS[name]
    except KeyError:
        raise ValueError(f"unknown engine `{name}`; known: {', '.join(BACKENDS)}") from None


def mock_cost_per_1m_tok(sku: str, tok_s: float, gpu_count: int) -> float:
    usd_h = {"a100": 2.0, "h100": 3.5, "h200": 4.5, "b200": 7.0, "gb200": 7.0, "b300": 9.0}
    rate = next((v for k, v in usd_h.items() if sku.startswith(k)), 3.0) * gpu_count
    return rate / (tok_s * 3600) * 1e6 if tok_s > 0 else math.inf

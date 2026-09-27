"""Hardware probe. Measured values override datasheet priors (hw_query)."""
from __future__ import annotations

import platform
import shutil
import subprocess
from typing import Any

_FIELDS = [
    "index",
    "name",
    "uuid",
    "driver_version",
    "memory.total",
    "memory.used",
    "clocks.max.sm",
    "clocks.max.memory",
    "power.limit",
    "compute_cap",
    "pcie.link.gen.max",
    "pcie.link.width.max",
]


def probe() -> dict[str, Any]:
    out: dict[str, Any] = {"host": platform.node(), "os": platform.platform(), "gpus": [], "source": "measured"}
    smi = shutil.which("nvidia-smi")
    if not smi:
        out["note"] = "nvidia-smi not found; no NVIDIA GPUs visible on this host"
        return out
    try:
        res = subprocess.run(
            [smi, f"--query-gpu={','.join(_FIELDS)}", "--format=csv,noheader,nounits"],
            capture_output=True,
            text=True,
            timeout=20,
            check=True,
        )
    except Exception as e:  # noqa: BLE001
        out["note"] = f"nvidia-smi failed: {e}"
        return out
    for line in res.stdout.strip().splitlines():
        parts = [p.strip() for p in line.split(",")]
        gpu = dict(zip(_FIELDS, parts))
        for k in ("memory.total", "memory.used", "clocks.max.sm", "clocks.max.memory", "pcie.link.gen.max", "pcie.link.width.max"):
            try:
                gpu[k] = int(float(gpu[k]))
            except (KeyError, ValueError):
                pass
        try:
            gpu["power.limit"] = float(gpu["power.limit"])
        except (KeyError, ValueError):
            pass
        out["gpus"].append(gpu)
    # NVLink topology is cheap to capture and matters for TP/EP upper bounds.
    try:
        topo = subprocess.run([smi, "topo", "-m"], capture_output=True, text=True, timeout=20)
        if topo.returncode == 0:
            out["topology"] = topo.stdout[:4000]
    except Exception:  # noqa: BLE001
        pass
    return out

"""Load generator against an OpenAI-compatible `/v1/completions` endpoint (stdlib only).
Streams each response to measure TTFT and TPOT; N concurrent workers; returns one sample.
"""
from __future__ import annotations

import http.client
import json
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from typing import Any
from urllib.parse import urlparse

_PROMPT_WORD = "inference "


def _one_request(base_url: str, model: str, prompt_tokens: int, output_tokens: int) -> dict[str, float]:
    u = urlparse(base_url)
    conn = http.client.HTTPConnection(u.hostname, u.port or 80, timeout=600)
    body = json.dumps({
        "model": model,
        "prompt": _PROMPT_WORD * prompt_tokens,
        "max_tokens": output_tokens,
        "temperature": 0,
        "stream": True,
        "ignore_eos": True,
    })
    t0 = time.perf_counter()
    conn.request("POST", "/v1/completions", body=body, headers={"Content-Type": "application/json"})
    resp = conn.getresponse()
    if resp.status != 200:
        raise RuntimeError(f"HTTP {resp.status}: {resp.read()[:200]!r}")
    first = None
    tokens = 0
    buf = b""
    while True:
        chunk = resp.read1(65536) if hasattr(resp, "read1") else resp.read(65536)
        if not chunk:
            break
        buf += chunk
        while b"\n\n" in buf:
            frame, buf = buf.split(b"\n\n", 1)
            for line in frame.split(b"\n"):
                if line.startswith(b"data: ") and line != b"data: [DONE]":
                    if first is None:
                        first = time.perf_counter()
                    tokens += 1
    t1 = time.perf_counter()
    conn.close()
    ttft = (first or t1) - t0
    return {"ttft_s": ttft, "tokens": tokens, "gen_s": t1 - (first or t1), "total_s": t1 - t0}


def run_load(base_url: str, model: str, workload: dict[str, Any]) -> dict[str, float]:
    conc = int(workload.get("concurrency", 32))
    n = int(workload.get("requests", 128))
    ptoks = int(workload.get("prompt_tokens", 512))
    otoks = int(workload.get("output_tokens", 128))
    results: list[dict[str, float]] = []
    lock = threading.Lock()
    t0 = time.perf_counter()
    with ThreadPoolExecutor(max_workers=conc) as ex:
        for f in [ex.submit(_one_request, base_url, model, ptoks, otoks) for _ in range(n)]:
            r = f.result()
            with lock:
                results.append(r)
    wall = time.perf_counter() - t0
    ttfts = sorted(r["ttft_s"] for r in results)
    total_tokens = sum(r["tokens"] for r in results)
    per_tok = [r["gen_s"] / max(1, r["tokens"] - 1) for r in results if r["tokens"] > 1]
    return {
        "tok_s": total_tokens / wall if wall > 0 else 0.0,
        "ttft_p50_ms": ttfts[len(ttfts) // 2] * 1000,
        "ttft_p99_ms": ttfts[min(len(ttfts) - 1, int(len(ttfts) * 0.99))] * 1000,
        "tpot_ms": (sum(per_tok) / len(per_tok) * 1000) if per_tok else 0.0,
        "gpu_util": 0.0,  # filled by an nvidia-smi sampler in a later increment
        "mem_bw_util": 0.0,
        "duration_s": wall,
    }

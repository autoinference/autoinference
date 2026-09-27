"""Frame loop: `[u32 BE length][CBOR request]` on stdin → `[u32 BE length][CBOR response]` on stdout.

Requests: {"id": n, "op": "...", ...}. Responses: {"id": n, "ok": bool, "result": any, "error": str|None}.
stderr is free for logging. stdout carries frames only.
"""
from __future__ import annotations

import os
import struct
import sys
import traceback
from typing import Any

import cbor2

from . import PROTOCOL_VERSION
from .hw import probe as hw_probe
from .kb import KnobRegistry
from .engines import LaunchSpec, get_backend
from .trial import run_trial

MAX_FRAME = 16 * 1024 * 1024


def _read_frame(stream) -> dict[str, Any] | None:
    hdr = stream.read(4)
    if len(hdr) < 4:
        return None
    (n,) = struct.unpack(">I", hdr)
    if n > MAX_FRAME:
        raise ValueError(f"frame too large: {n}")
    body = b""
    while len(body) < n:
        chunk = stream.read(n - len(body))
        if not chunk:
            return None
        body += chunk
    return cbor2.loads(body)


def _write_frame(stream, obj: dict[str, Any]) -> None:
    body = cbor2.dumps(obj)
    stream.write(struct.pack(">I", len(body)))
    stream.write(body)
    stream.flush()


class Server:
    def __init__(self) -> None:
        kb_dir = os.environ.get("AUTOINFERENCE_KB_DIR")
        self.kb = KnobRegistry(kb_dir) if kb_dir else None

    def handle(self, req: dict[str, Any]) -> Any:
        op = req.get("op")
        if op == "hello":
            return {
                "protocol_version": PROTOCOL_VERSION,
                "python": sys.version.split()[0],
                "kb": self.kb.summary() if self.kb else None,
            }
        if op == "ping":
            return "pong"
        if op == "shutdown":
            raise SystemExit(0)
        if op == "hw_probe":
            return hw_probe()
        if op == "trial_run":
            return run_trial(
                req["engine"], req["model"], req["sku"], req.get("config") or {}, req.get("workload") or {},
                int(req.get("repeats") or 3), int(req.get("timeout_s") or 900),
            )
        if op == "engine_command":
            return get_backend(req["engine"]).command(LaunchSpec(req["engine"], req["model"], req.get("config") or {}))
        if op.startswith("kb_"):
            if self.kb is None:
                raise RuntimeError("no knob registry: AUTOINFERENCE_KB_DIR not set or invalid")
            if op == "kb_summary":
                return self.kb.summary()
            if op == "kb_search":
                return self.kb.search(req.get("query", ""), engine=req.get("engine"), limit=int(req.get("limit") or 10))
            if op == "kb_knob":
                return self.kb.knob(req["engine"], req["name"])
            if op == "kb_constraints":
                return self.kb.constraints(req["engine"], list(req.get("terms") or []), limit=int(req.get("limit") or 15))
            if op == "kb_attention_backends":
                return self.kb.attention_backends(req.get("engine") or "vllm", req.get("compute_capability"))
        raise ValueError(f"unknown op: {op}")


def main() -> None:
    stdin = sys.stdin.buffer
    stdout = sys.stdout.buffer
    server = Server()
    while True:
        try:
            req = _read_frame(stdin)
        except Exception as e:  # noqa: BLE001
            print(f"[sidecar] bad frame: {e}", file=sys.stderr)
            return
        if req is None:
            return
        rid = req.get("id", 0)
        try:
            result = server.handle(req)
            _write_frame(stdout, {"id": rid, "ok": True, "result": result, "error": None})
        except SystemExit:
            _write_frame(stdout, {"id": rid, "ok": True, "result": "bye", "error": None})
            return
        except Exception as e:  # noqa: BLE001
            traceback.print_exc(file=sys.stderr)
            _write_frame(stdout, {"id": rid, "ok": False, "result": None, "error": f"{type(e).__name__}: {e}"})

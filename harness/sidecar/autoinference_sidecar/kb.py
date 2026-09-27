"""Knob registry over inference-engine-KB/knobs/*.json.

The JSON files are machine-extracted from engine source at pinned commits (see
inference-engine-KB/INDEX.md). Each knob carries name/kind/type/default/choices/help and a
`source` of `file:line`. Nothing here is hand-maintained; we only index and search.
"""
from __future__ import annotations

import glob
import json
import os
import re
from typing import Any

ENGINE_ALIASES = {
    "vllm": "vllm",
    "sglang": "sglang",
    "trtllm": "trtllm",
    "tensorrt-llm": "trtllm",
    "tensorrt_llm": "trtllm",
    "llamacpp": "llamacpp",
    "llama.cpp": "llamacpp",
    "lmdeploy": "lmdeploy",
    "lmcache": "lmcache",
    "modelopt": "modelopt",
    "llmcompressor": "llmcompressor",
    "llm-compressor": "llmcompressor",
}


def _norm(s: str) -> str:
    return re.sub(r"[^a-z0-9]+", " ", s.lower()).strip()


def _tokens(s: str) -> list[str]:
    return [t for t in _norm(s).split() if t]


class KnobRegistry:
    def __init__(self, kb_dir: str) -> None:
        self.kb_dir = kb_dir
        self.knobs_dir = os.path.join(kb_dir, "knobs")
        if not os.path.isdir(self.knobs_dir):
            raise FileNotFoundError(f"no knobs/ under {kb_dir}")
        self.files: dict[str, str] = {}
        self.data: dict[str, dict[str, Any]] = {}
        self.constraint_files: dict[str, dict[str, Any]] = {}
        for path in sorted(glob.glob(os.path.join(self.knobs_dir, "*.json"))):
            base = os.path.basename(path)
            engine = base.split("-")[0]
            if base.startswith("vllm-constraints"):
                with open(path) as f:
                    self.constraint_files["vllm"] = json.load(f)
                continue
            with open(path) as f:
                self.data[engine] = json.load(f)
            self.files[engine] = base
        # Flatten knobs into a searchable list per engine.
        self.index: dict[str, list[dict[str, Any]]] = {}
        for engine, d in self.data.items():
            rows: list[dict[str, Any]] = []
            for k in d.get("knobs", []) or []:
                rows.append(self._row(engine, k))
            # ModelOpt presets / llm-compressor modifiers are also useful search targets.
            for k in d.get("presets", []) or []:
                rows.append(self._row(engine, {**k, "kind": "preset"}))
            for k in d.get("modifiers", []) or []:
                rows.append(self._row(engine, {**k, "kind": "modifier"}))
            self.index[engine] = rows

    @staticmethod
    def _row(engine: str, k: dict[str, Any]) -> dict[str, Any]:
        name = k.get("name") or k.get("flag") or k.get("key") or ""
        text = " ".join(
            str(x)
            for x in [
                name,
                " ".join(k.get("flags") or []),
                k.get("help") or k.get("description") or "",
                k.get("group") or "",
                k.get("env") or "",
            ]
        )
        return {"engine": engine, "_text": _norm(text), **k}

    # ---- queries -----------------------------------------------------------------------------

    def summary(self) -> dict[str, Any]:
        out = {}
        for engine, d in self.data.items():
            out[engine] = {
                "version": d.get("version"),
                "commit": (d.get("commit") or "")[:12],
                "counts": d.get("counts"),
                "file": self.files[engine],
                "constraints": len(d.get("constraints") or []) if isinstance(d.get("constraints"), list) else None,
            }
        if "vllm" in self.constraint_files:
            c = self.constraint_files["vllm"]
            out["vllm"]["constraints"] = c.get("counts")
            out["vllm"]["taxonomy"] = c.get("taxonomy_counts")
        return out

    def _engines(self, engine: str | None) -> list[str]:
        if not engine:
            return list(self.index.keys())
        e = ENGINE_ALIASES.get(engine.lower(), engine.lower())
        return [e] if e in self.index else []

    def search(self, query: str, engine: str | None = None, limit: int = 10) -> dict[str, Any]:
        q = _norm(query)
        toks = _tokens(query)
        hits: list[tuple[float, dict[str, Any]]] = []
        for e in self._engines(engine):
            for row in self.index[e]:
                name = _norm(str(row.get("name", "")))
                score = 0.0
                if q and q == name:
                    score += 100
                elif q and q in name:
                    score += 40
                for t in toks:
                    if t in name:
                        score += 8
                    elif t in row["_text"]:
                        score += 2
                if score > 0:
                    hits.append((score, row))
        hits.sort(key=lambda x: (-x[0], str(x[1].get("name"))))
        return {
            "query": query,
            "engines": self._engines(engine),
            "total": len(hits),
            "results": [self._public(r) for _, r in hits[:limit]],
        }

    @staticmethod
    def _public(row: dict[str, Any]) -> dict[str, Any]:
        keep = ["engine", "name", "flags", "kind", "type", "default", "choices", "help", "group", "env", "source", "stability", "action"]
        out = {k: row[k] for k in keep if k in row and row[k] not in (None, "", [])}
        if "help" in out and isinstance(out["help"], str) and len(out["help"]) > 400:
            out["help"] = out["help"][:400] + "…"
        return out

    def knob(self, engine: str, name: str) -> dict[str, Any] | None:
        n = _norm(name)
        for e in self._engines(engine):
            for row in self.index[e]:
                if _norm(str(row.get("name", ""))) == n or n in [_norm(f) for f in (row.get("flags") or [])]:
                    return {k: v for k, v in row.items() if k != "_text"}
        return None

    def constraints(self, engine: str, terms: list[str], limit: int = 15) -> dict[str, Any]:
        e = ENGINE_ALIASES.get(engine.lower(), engine.lower())
        toks = [_norm(t) for t in terms if t]
        rows: list[dict[str, Any]] = []
        if e == "vllm" and "vllm" in self.constraint_files:
            rows = list(self.constraint_files["vllm"].get("raise_sites") or [])
        else:
            c = self.data.get(e, {}).get("constraints")
            if isinstance(c, list):
                rows = c
            elif isinstance(c, dict):
                for v in c.values():
                    if isinstance(v, list):
                        rows.extend(v)
        scored = []
        for r in rows:
            text = _norm(json.dumps(r, default=str))
            score = sum(1 for t in toks if t and t in text)
            if score:
                scored.append((score, r))
        scored.sort(key=lambda x: -x[0])
        return {"engine": e, "terms": terms, "total": len(scored), "results": [r for _, r in scored[:limit]]}

    def attention_backends(self, engine: str, compute_capability: str | None) -> dict[str, Any]:
        e = ENGINE_ALIASES.get(engine.lower(), engine.lower())
        if e != "vllm" or "vllm" not in self.constraint_files:
            return {"engine": e, "note": "attention backend matrix only extracted for vLLM in this KB revision"}
        m = self.constraint_files["vllm"].get("attention_backend_matrix") or {}
        out: dict[str, Any] = {
            "engine": "vllm",
            "version": self.constraint_files["vllm"].get("version"),
            "backends": m.get("backends"),
            "selection_priority": m.get("selection_priority"),
            "mla_prefill_backends": m.get("mla_prefill_backends"),
        }
        if compute_capability:
            cc = compute_capability.strip()
            major = cc.split(".")[0]
            pri = m.get("selection_priority") or {}
            key = "standard_sm100" if major == "10" else "standard_default"
            out["selection_for_cc"] = {"compute_capability": cc, "priority_key": key, "order": pri.get(key)}
            if major == "10":
                out["selection_for_cc"]["mla_order"] = pri.get("mla_sm100")
        return out

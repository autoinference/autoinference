# autoinference — Architecture Blueprint

The committed architecture, distilled from a survey of 24 open-source agentic harnesses
(the `other-harnesses/` survey corpus (outside this repo)) and the autoinference product premise: a CLI on customer infra that
turns raw weights into a peak-performance deployment through four agents — **Engine Selection →
Config Tuning → Kernel Synthesis → Validation** — with a wandb-style dashboard and an MCP surface.
Numbered decisions live in [DECISIONS.md](DECISIONS.md); this is the shape.

## 1. Stack

| Layer | Choice | Modeled on |
|---|---|---|
| Core | Rust, single static binary | `codex/codex-rs` |
| Sidecar | Python, out-of-process, `[u32 BE len][CBOR]` | `pi/packages/protocol` |
| Dashboard | TypeScript from generated types (`autoinference schema`) | `codex/codex-rs/app-server-protocol`, `opencode/packages/{sdk,stats}` |
| TUI | ratatui | `codex/codex-rs/tui`, `grok-build xai-grok-pager` |
| Store | SQLite (events, messages, domain ledger) + JSONL transcript + content-addressed artifacts | `codex/codex-rs/rollout`, `crush/internal/db` (ideas) |

The deciding constraint is deployment onto a customer's GPU login node: no Node, and the Python env
there is the thing under test. Everything Python-native (vLLM, SGLang, Triton, nsys/ncu parsers,
Optuna/BoTorch, torch verification) lives in the sidecar *inside the customer's engine venv*.

## 2. Agent loop

```
turn ─► [budget_guard] ─► [tool_calling] ─► [micro_compaction] ─► [inference] ─► [end_turn]
             │                  │                    │                  │
        steps/turn cap   execute tool_uses   prune old tool     stream deltas;    yield when
        turns/session    → user(tool_results) results (no LLM)   append assistant  no tool_use
```

* `StateMachine { steps: Vec<Operation> }` — first applicable op runs, returns `Effect`s
  (`AppendMessage`, `ReplaceMessages`, `AddUsage`, `Emit`, `EndTurn`, `Fail`).
* Runtime **persists effects before publishing** them — the durable log is authoritative, subscribers
  are hints (pi `harness.md` §3.4 atomic transition rule).
* Domain ops slot into the same Vec: `ops_trial` (compile→run→profile→measure as one durable op),
  `ops_numerical_gate` (transition condition, not an LLM choice), `ops_bayes_propose` (LLM ≤ 20% of
  trials), `ops_budget_guard` (GPU-hours, not just tokens).

## 3. Tool space (guarded, typed)

| Tool | Returns | Skeleton |
|---|---|---|
| `bash` | text (head+tail bounded) | ✅ read-only allowlist, deny-list (GPU reset, service control) |
| `read_file` / `write_file` / `edit_file` / `list_dir` | — | ✅ replacer ladder exact→line-trimmed→ws-normalized |
| `hw_query` | `Sku[]` with provenance | ✅ compiled-in TOML, 9 SKUs |
| `hw_probe` | measured GPUs (nvidia-smi + topology) | ✅ via sidecar |
| `kb_search` / `kb_constraints` / `kb_attention_backends` | registry rows with `file:line` | ✅ via sidecar over inference-engine-KB |
| `engine.launch(engine, config)` | `ServerHandle` | next |
| `bench.run(handle, workload)` | `BenchResult` (typed; median+IQR; `noisy` flag; `gpu_hours`) | next |
| `profile.capture` / `profile.query` | artifact **handle** / ≤2 KB slice | next |
| `kernel.compile(src, arch)` | `KernelCompileResult` | next |
| `verify.numeric(ref, cand, mode)` | `VerifyResult` — **not LLM-invokable** | next |
| `search.propose` / `search.tell` | candidates / ack | next |
| `codemode.run(program)` | structured — **sweeps only** | next |

Guard seam (`ToolRegistry::execute`): blast-radius check → approval → execute → output bounding.

## 4. Events and dashboard

* Wire: `Envelope { protocol_version, session_id, seq, ts, ..Event }`; `Event` is `#[serde(tag="type")]`.
* Harness layer verbatim from codex (`thread.* turn.* item.* error`, `+ item.delta`); domain layer
  `run.* stage.* candidate.* job.* bench.sample profile.captured kernel.compiled verify.result
  pareto.updated deploy.*`.
* **Two tiers**: `Lossy` (`item.delta`, `bench.sample`, `job.running`) vs `MustDeliver` (everything
  terminal). Drop counters are metrics.
* **Invariant**: snapshots (`SessionSnapshot`) are authoritative; events animate. A dashboard that
  loses every event over a customer VPN is still correct.
* Pipeline (opencode reference, in order of build): in-process bus ✅ → local SSE/WS endpoint →
  coalescing content-keyed upload queue (`part/{item_id}` semantics) with debounce/rate-limit/backoff
  (prime-agent `agent-traces.ts`) → snapshot endpoints → warehouse. Self-hosted customers get
  local JSONL/OTLP instead.
* Privacy: telemetry fields are constrained scalars (openhands `telemetry/models.py`); content-free by
  default; upload opt-in per workspace.

## 5. Context management

1. Profiler/log bytes never enter context — handles + deterministic digests. ✅ head+tail bounding
   today; artifact store next.
2. Model-free micro-compaction of old tool results first. ✅
3. Tombstone compaction with cache-preserving prefix replay at safe turn boundaries. next
4. Pinned results ledger regenerated per turn as a DB projection (never summarized). next

## 6. Security posture

`BlastRadius { mode: Observe|Tune|Deploy, gpus, nodes, may_touch_prod }` per session ✅ →
hash-pinned TOML policy (gemini-cli) → Landlock/seccomp profiles with a separately-approved
`profiling` escalation for `ncu` counters (grok-build named profiles) → exec-policy DSL (codex).
Canary precondition: no canary without a pinned, health-checked rollback target.

## 7. Domain layer

* **Four stages** = a declarative Recipe (goose) of durable Operations with typed handoffs;
  **arenas** (qwen-code) inside Stage 1 (engine head-to-head) and Stage 3 (N competing kernels)
  where the judge is objective and cheap. Not mailbox teams — sequential contracts belong in an
  audit trail.
* **SKU knowledge** = compiled-in facts exposed as `hw_query` ✅; probes override datasheets ✅
  (nvidia-smi; STREAM/NCCL next). Procedures (how to pick TP/PP for a MoE) are skills/recipes, not facts.
* **Knobs** come from `inference-engine-KB` (external) only ✅ (3,275 knobs, 482 constraints, `file:line`).
* **Trial** = detachable (handle persisted, survives agent death), idempotent by
  `hash(kernel + config + engine_version + sku + workload)`, variance-aware (N repeats, median+IQR,
  significance is a machine guard), emits the right tier per event.
* **Bayes × LLM**: LLM defines the space and diagnoses plateaus; optimizer owns the search;
  `CandidateSource` tracked so eval can prove LLM value; multi-objective (qEHVI/NSGA-II) → pareto front.
* **Numerical gate**: tiered `bitwise / ulp≤N / distributional`; `VerifyResult.proof_id` referenced
  by every deployment record.

## 8. Evaluation — `autobench`

`score = verified ? speedup_vs_expert_baseline : 0`. Geomean normalized speedup, p99-under-SLO,
$/1M tok; `pass@k`, `pass^k`, flakiness (cline `metrics.ts`); infra-vs-model failure tiers
(swe-bench `infra_failure.py`); harbor-style cost table **plus `gpu_h`**; four baselines every run:
vendor default, expert config, mini-swe-agent-style bash-only, and **pure Bayesian with no LLM**.

## 9. Repo map

```
crates/protocol   wire types; `autoinference schema` exports JSON Schema
crates/core       bus · session · llm · tools · hardware · sidecar · config
crates/agent      machine · ops · runtime · prompt
crates/tui        ratatui subscriber
crates/cli        `autoinference` binary (#![deny(clippy::print_stdout)])
sidecar/          server.py (frames) · kb.py (registry) · hw.py (probe)
```

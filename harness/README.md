# autoinference

Agentic CLI that turns raw model weights into a peak-performance LLM deployment on NVIDIA GPUs
(Ampere · Hopper · Blackwell): pick the engine (vLLM / SGLang / TensorRT-LLM / tokenspeed / LMDeploy),
tune its configuration against real benchmarks, synthesize kernels where it pays, and verify every
change numerically before it can ship.

**Status: walking skeleton (v0.0.1).** Every layer exists and is wired end-to-end; each is thin.

```
┌──────────────────────── Rust core (single static binary) ────────────────────────┐
│  cli ─┬─ chat / tui / exec --json                                                  │
│       └─ hw · kb · sessions · schema · doctor                                      │
│  agent  composable state machine: budget_guard → tool_calling → micro_compaction   │
│         → inference → end_turn   (effects persisted BEFORE they are observable)    │
│  core   two-tier event bus · SQLite sessions + JSONL transcript · tools (bash, fs, │
│         hw_query, kb_*) · Anthropic streaming provider · compiled-in SKU KB        │
│  protocol  one Rust type = JSON Schema = dashboard TS types (thread.*/turn.*/item.*│
│         + run.*/stage.*/candidate.*/job.*/bench.sample/verify.result/pareto/deploy)│
└──────────────┬────────────────────────────────────────────────────────────────────┘
               │ [u32 BE len][CBOR]  (out-of-process on purpose: lives in the engine venv,
               ▼                      a segfaulting kernel kills the sidecar, not the agent)
┌──── Python sidecar ────┐      ┌──── inference-engine-KB (AUTOINFERENCE_KB_DIR)    ────┐
│ kb.py  knob registry   │◄─────│ 3,275 knobs + 482 constraints, machine-extracted from │
│ hw.py  nvidia-smi probe│      │ vLLM/SGLang/TRT-LLM/… source with file:line provenance │
└────────────────────────┘      └───────────────────────────────────────────────────────┘
```

## Quick start

```bash
# 1. build
cargo build --release

# 2. sidecar (once) — plain venv, only needs cbor2
python3 -m venv sidecar/.venv && sidecar/.venv/bin/pip install cbor2
cat > autoinference.toml <<EOF
python = "$PWD/sidecar/.venv/bin/python"
EOF

# 3. point at the knowledge base (auto-discovered if it is a sibling directory)
export AUTOINFERENCE_KB_DIR=/path/to/inference-engine-KB

# 4. sanity
./target/release/autoinference doctor
./target/release/autoinference kb search "kv cache dtype" --engine vllm
./target/release/autoinference kb backends --cc 10.0        # what vLLM auto-picks on Blackwell
./target/release/autoinference hw query h100-sxm

# 5. talk to it
export ANTHROPIC_API_KEY=...
./target/release/autoinference tui                          # full-screen
./target/release/autoinference chat                         # line REPL
./target/release/autoinference exec --json "Which attention backend will vLLM pick on B200 with fp8 KV cache, and why?" \
  | jq -r '.type'                                           # JSONL event stream for dashboards

# no key? the mock provider drives the whole loop offline
./target/release/autoinference --provider mock exec --json "kb: max num batched tokens"
```

## Commands

| Command | What |
|---|---|
| `chat` / `tui` / `exec [--json] PROMPT` | run the agent (line REPL / ratatui / headless JSONL) |
| `--access observe\|tune\|deploy` | blast-radius mode; hardware-touching tools refuse below `tune` |
| `-y` | auto-approve mutating tools (headless); otherwise you are asked |
| `kb search\|knob\|constraints\|backends\|summary` | query the machine-extracted knob registry |
| `hw list\|query SKU\|probe` | datasheet priors / measured GPUs |
| `sessions list\|events ID\|stats ID` | the durable event log (this is what the dashboard reads) |
| `trial --engine mock\|vllm\|sglang --sku … --config JSON` | one headless benchmark trial, recorded on the ledger |
| `runs list\|show RUN\|pareto RUN` | candidates, results and the pareto front of a tuning run |
| `schema` | JSON Schema of the event envelope → generate dashboard types from it |

## The TUI

`autoinference tui` — a full-screen ratatui interface designed to feel like a product, not a debug view:

* **animated intro** — gradient logo sweep, brand cyan→violet accents throughout
* **streaming markdown** — headings, lists, tables, fenced code with a language tag and light token colouring; blinking caret while the model streams
* **tool cards** — braille spinner while running, ✓/✗ on completion, elapsed time, 3-line output preview with +/− diff colouring, `Ctrl+E` to expand
* **trial cards** — verdict badge (improved / regressed / no_change / failed) and the typed result line
* **bench panel** (`Tab`) — live `bench.sample` sparkline, pareto scatter (tok/s vs p99 TTFT, front highlighted), last candidates, best-vs-latest meter
* **timeline panel** — every bus event with its seq and delivery tier (● must-deliver · lossy)
* **approval modal** — every mutating/hardware tool call asks with the full input, current access mode and prod flag; `y` / `n`
* **command palette** — type `/` for `/help /bench /timeline /expand /clear /session /quit` with live filtering
* header: model · session · access badge (green observe / amber tune / red deploy) · shimmering "thinking" state; footer: eased token counters, cost, seq, bus health
* mouse wheel + PgUp/PgDn scrolling with a scrollbar, prompt history (↑/↓), multi-line input (Alt+Enter or Shift+Enter), bracketed paste, toasts
* keys: `Enter` send · `Tab` cycle side panel · `Shift+Tab` hide it · `Ctrl+B` bench panel · `Ctrl+E` expand tool output · `Ctrl+L` clear · `Esc` cancel turn / close palette · `Ctrl+C` quit · in the approval modal `y` approves and `n`/`Esc` declines (`Enter` deliberately does not approve; keys typed before the modal opened are ignored)

Regression-tested through a PTY harness — `scripts/tui-snapshot.py` drives the real binary in a pseudo-terminal and prints emulated screens.

## Load-test tiers (incremental vs robust)

Every trial picks a load generator; all tiers return the same `BenchResult` shape so runs are comparable.

| tier | tool | use it for |
|---|---|---|
| `quick` (default) | built-in stdlib generator, streaming TTFT/TPOT | **every commit / every candidate** — seconds |
| `engine` | `vllm bench serve` / `python -m sglang.bench_serving` | numbers comparable to the engine's own reference benchmarks |
| `aiperf` | [NVIDIA AIPerf](https://developer.nvidia.com/blog/benchmarking-llm-inference-at-scale-with-aiperf/) (`aiperf profile`) | **robust stress runs, used sparingly** — full percentiles, poisson/gamma arrivals, request-rate shaping, GPU power/utilization telemetry, multi-node |

```bash
scripts/deploy-check.sh                      # quick tier after each deployment change (exit 2 if the result is noisy)
STRESS=1 scripts/deploy-check.sh             # AIPerf, 512 poisson-arrival requests
LOADGEN=engine scripts/deploy-check.sh       # engine-native bench tool
autoinference trial --engine vllm --loadgen aiperf --workload '{"concurrency":64,"arrival":"poisson","request_rate":40}' …
```

AIPerf needs `uv tool install aiperf` on the GPU host; when a tier's tool is missing the trial fails with a
one-line install hint instead of silently downgrading.

## Design provenance

Every subsystem is modeled on the best-in-class implementation found in a survey of 24 open-source
harnesses (kept outside this repo under `autoinference-harness/other-harnesses/`). See [docs/DECISIONS.md](docs/DECISIONS.md) for the numbered
decisions and [docs/BLUEPRINT.md](docs/BLUEPRINT.md) for the full architecture. Short version:

| Subsystem | Modeled on |
|---|---|
| Event taxonomy, `#![deny(print_stdout)]` JSONL discipline | `codex/codex-rs/exec/src/exec_events.rs` |
| One Rust type → JSON Schema → TS | `codex/codex-rs/app-server-protocol` |
| Composable `Operation` state machine | `goose/crates/goose-agent/src/machine.rs` + `ops_*.rs` |
| Durability semantics (persist before observable) | `pi/packages/agent/docs/harness.md` |
| Snapshot-authoritative / events-are-hints invariant | `pi/packages/protocol/README.md` |
| Two-tier bus with exported drop counters | `crush/internal/pubsub/broker.go` (semantics only; FSL) |
| Tool guard seam (blast radius → approval → execute → bound) | `deepseek-harness/docs/tool-execution-pipeline.md` |
| Edit replacer ladder | `opencode/packages/opencode/src/tool/edit.ts` |
| Model-free micro-compaction first | `deepseek-harness compaction-tool-result-pruner`, `qwen-code microcompaction` |
| Prompt-cache breakpoints on stable prefixes | `aider/aider/coders/chat_chunks.py` |
| SKU facts as a tool, not prompt text; probes override datasheets | (autoinference) |
| Knobs never guessed — registry with `file:line` provenance | `inference-engine-KB` (external; set `AUTOINFERENCE_KB_DIR`) |

## Layout

```
crates/protocol   events.rs snapshot.rs sidecar.rs      wire types (+ `schema` export)
crates/core       bus session llm/{anthropic,mock} tools/{bash,fs,hw,kb} hardware sidecar config
crates/agent      machine.rs (Operation/Effect/StateMachine) ops.rs runtime.rs prompt.rs
crates/tui        ratatui front-end — just another bus subscriber
crates/cli        the `autoinference` binary
sidecar/          Python: server.py (CBOR frames) kb.py (registry) hw.py (probe)
docs/             DECISIONS.md BLUEPRINT.md
```

## Roadmap (next milestones)

1. ~~**Trial primitive**~~ ✅ v0.0.3 — `trial_run` tool + `autoinference trial`: launch→warm→measure N repeats,
   content-hash cache, median+IQR `noisy` flag, ledger rows, pareto front, `candidate.*/job.*/bench.sample` events.
   Mock engine (synthetic roofline) runs anywhere; vLLM/SGLang backends launch real servers on GPU hosts.
   Still to do: `profile.capture/query`, detach/re-attach of long jobs, nvidia-smi util sampling during runs.
2. **Numerical gate** — `ops_numerical_gate.rs` + `verify.numeric` in the sidecar (bitwise / ulp≤N / distributional).
3. **Bayesian search** — Optuna/BoTorch in the sidecar behind `search.propose/tell`; LLM capped at ~20% of trials.
4. **Uploader + dashboard** — coalescing content-keyed queue (opencode `share-next.ts`) → `dashboard.autoinference.org`.
5. **Sandboxing** — Landlock/seccomp profiles with a separately-approved `profiling` escalation for `ncu`.
6. **MCP server** — `autoinference_{analyze,tune,status,pareto,verify,deploy}`; profiles exposed as MCP resources by URI.
7. **autobench** — the eval rig: `score = verified ? speedup_vs_expert_baseline : 0`, `pass^k`, `gpu_h` column, pure-Bayes baseline.

MIT licensed.

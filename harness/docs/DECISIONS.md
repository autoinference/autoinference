# Architecture Decision Records

Numbered, append-only. Each names the harness(es) it is modeled on so provenance is checkable
against the `other-harnesses/` survey corpus (outside this repo).

## 1. Rust core, Python sidecar (out-of-process), TypeScript dashboard from generated types
**Decision.** The agent loop, session store, event bus, tools, sandbox, TUI and MCP server are Rust
and ship as one static binary. Engine adapters, profiler parsers, the Bayesian optimizer and numerical
verification run in a Python sidecar over `[u32 BE len][CBOR]` frames. The dashboard consumes a
generated SDK. **Why.** Deployment target is a customer GPU login node: no Node, and the Python env on
that box is *the thing being tuned* — installing into it version-conflicts with vLLM's torch pin.
Only Rust gives Landlock/seccomp/Seatbelt sandboxing without FFI. PyO3 embedding rejected: the sidecar
must live in the customer's engine venv, and a segfaulting kernel must kill the sidecar, not durable
state. **Modeled on** `codex/codex-rs/` (single binary + sandboxing), `pi/packages/protocol` (framing),
`codex/codex-rs/app-server-protocol` (derive Serialize+JsonSchema+TS, pinned in CI).
**Cost.** Three CI lanes.

## 2. Agent loop = pi's durability semantics implemented as goose's composable steps
**Decision.** `StateMachine { steps: Vec<Operation> }`; the first applicable operation runs and returns
`Effect`s; the runtime persists effects *before* publishing them. State is total, never relative.
**Modeled on** `goose/crates/goose-agent/src/machine.rs` + `goose/.../state_machine/ops_*.rs`
(structure), `pi/packages/agent/docs/harness.md` §3.2/§3.4/§0.5 (semantics: durable program counter,
crash-mid-tool recovery). Goose's own 6,113-line legacy `agent.rs` is the argument against a monolith.
**Skeleton ops.** `budget_guard → tool_calling → micro_compaction → inference → end_turn`.
Domain ops (`trial`, `numerical_gate`, `bayes_propose`) slot into the same Vec.

## 3. Event protocol: codex taxonomy + domain events; snapshots authoritative
**Decision.** `thread.* / turn.* / item.* / error` verbatim from codex; `item.delta` added for
streaming; domain layer `run.* stage.* candidate.* job.* bench.sample profile.captured kernel.compiled
verify.result pareto.updated deploy.*`. `ThreadItem {id, ..flattened details}` so `item.updated` is an
upsert. `ErrorInfo.will_retry` so transient OOM-and-retry does not render as failure. Invariant:
*snapshots are authoritative; progress events are transient UI hints.* **Modeled on**
`codex/codex-rs/exec/src/exec_events.rs`, `pi/packages/protocol/README.md:10`.
**Enforced by** `#![deny(clippy::print_stdout)]` in the CLI crate.

## 4. Two delivery tiers with exported drop counters
**Decision.** `Tier::Lossy` (`item.delta`, `bench.sample`, `job.running`) uses `try_send` and counts
drops; `Tier::MustDeliver` (everything terminal/authoritative) is bounded-blocking with a 50 ms
per-subscriber timeout and counts timeouts as a *bug metric*. Both counters are shown in the TUI footer
and will be exported to the dashboard. **Modeled on** `crush/internal/pubsub/broker.go` — semantics
reimplemented in Rust; crush is FSL-1.1-MIT so no code is copied.

## 5. Numerical verification is a tiered hard gate
**Decision.** `VerifyMode::{Bitwise, UlpBounded, Distributional}` per change class: bitwise for kernel
rewrites with identical math; ULP≤N for reassociation/fusion/reduction-order changes; distributional
(KL + task-metric delta) for quantization/dtype. The gate is a state-machine transition condition,
not an LLM-callable step, and every deployment record references a `VerifyResult.proof_id`.
**Why.** "0-ulp" is unachievable across TP degrees or under fp8/fp4 — most of what a tuner does.
The dashboard must display which tier a deployment was verified at.

## 6. Tools are typed and guarded; not bash-only
**Decision.** Small tool set behind one guard seam: blast-radius check → approval → execute → output
bounding. Domain tools return structs (`BenchResult`, `KernelCompileResult`), never prose. **Why.**
mini-swe-agent's bash-only minimalism wins on SWE-bench where a tool call costs seconds; here a call
can burn $400 of H100-hours or degrade production serving. **Modeled on**
`deepseek-harness/docs/tool-execution-pipeline.md`, `gemini-cli/packages/core/src/scheduler/`.
mini-swe-agent ships as the *eval baseline* instead.

## 7. Profiler bytes never enter the context window
**Decision.** `profile.capture` returns an artifact handle + deterministic digest; `profile.query`
returns ≤2 KB structured slices (nsys→SQLite, ncu→CSV). Compile logs get rule-based extraction
(first error, spill count, occupancy) before any LLM summary. Compaction: tombstones on an
append-only log + cache-preserving prefix replay; model-free micro-compaction runs first. A pinned
results ledger is regenerated each turn as a DB projection, never summarized from the transcript.
**Modeled on** `openhands-sdk/.../context/condenser/README.md`, `deepseek-harness/packages/compaction/
compaction-basic`, `opencode/packages/core/src/session/context-epoch.ts`, `deepseek-harness
session-stats/src/projection.ts`. **Skeleton.** `MicroCompaction` op + head/tail output bounding.

## 8. Three stores: transcript, domain ledger, artifacts
**Decision.** SQLite `events` (seq-ordered, name-indexed) + per-session JSONL transcript; domain
tables `runs/candidates/bench_results/verifications/deployments` in the same DB so the dashboard reads
one file; content-addressed artifacts with GC. Fork-awareness is first-class (`parent_session_id`).
Canary precondition: refuse to start unless a rollback target is pinned and health-checked.
**Modeled on** `codex/codex-rs/rollout/` (zstd JSONL + SQLite index + fork-aware names),
`pi/packages/agent/docs/harness.md` §0.3.

## 9. Blast radius is a first-class object; default observe-only
**Decision.** `BlastRadius { mode: Observe|Tune|Deploy, gpus, nodes, may_touch_prod }` is declared per
session, checked on every tool call, and rendered in the TUI header. `Observe` is read-only; `Tune`
may spawn test engines on reserved GPUs; `Deploy` may shift traffic and needs a separate approval.
GPU resets / `dcgmi config` / service control are denied outright. **Why.** This runs on customer
infra; the SRE has to be willing to run it at all. **Modeled on** `gemini-cli/packages/core/src/
policy/` (hash-pinned policy — next), `codex/codex-rs/execpolicy/` (command DSL — next).

## 10. Knobs are never guessed
**Decision.** Every engine flag the agent proposes must come from `kb_search`/`kb_constraints`, backed
by `inference-engine-KB` (external) (3,275 knobs, 482 constraints, machine-extracted from engine source at
pinned commits with `file:line` provenance). The system prompt says so; the tools exist to make it
cheap. **Why.** Flags churn weekly and the binding constraints live in validator code, not docs.

## 11. Hardware facts are a tool, not prompt text; probes override datasheets
**Decision.** `crates/core/data/skus.toml` is compiled in with `source_url` + `verified_date` per row
and exposed as `hw_query`, so every fact the agent relied on is in the event log. `hw_probe`
(nvidia-smi, later STREAM/NCCL) records measured values that take precedence. **Why.** Real clusters
are power-capped, throttled and mis-cabled; the datasheet is a prior.

## 12. Prompt-cache breakpoints on the two stable prefixes
**Decision.** System prompt and tool list carry `cache_control: ephemeral`; messages do not.
**Modeled on** `aider/aider/coders/chat_chunks.py` (breakpoints at stable boundaries).

## 13. The TUI is just another subscriber
**Decision.** ratatui; renders `item.delta` for streaming and `item.completed` for tool cards; reads the
authoritative `SessionSnapshot` for header/footer; never owns agent state. **Modeled on**
`codex/codex-rs/tui/`, `grok-build/.../xai-grok-pager/src/views/dashboard/` (multi-pane board — next).

## 14. Mock provider for offline end-to-end
**Decision.** `--provider mock` drives the full loop (tool calls included) with no network, so CI and
the dashboard can be developed without spend. **Modeled on** `pi/packages/coding-agent/test/suite/harness.ts`.

## 15. The Trial is machine policy, not prompt policy
**Decision.** `trial_run` is the only way the model obtains a benchmark number. It launches the engine
(`vllm serve` / `sglang.launch_server` rendered from registry-named knobs, or a synthetic-roofline mock),
warms up, runs the workload N times, and returns a typed `BenchResult` (median + IQR, `noisy` when
IQR/median > 10%, `gpu_hours`, $/1M tok). Identical specs are served from the ledger by content hash
(engine version folded in). Every trial writes `candidates` + `bench_results` rows, emits
`candidate.proposed → job.submitted → bench.sample*(lossy) → job.finished → candidate.evaluated →
pareto.updated`, and recomputes the pareto front (max tok/s, min p99, min cost). **Why.** An agent that
trusts single measurements chases thermal noise forever; a Bayesian loop re-proposes near-duplicates
constantly. Warm-up, repeats, significance and provenance are invariants, so they live in the tool, not
the prompt. **Modeled on** the Trial design in BLUEPRINT §7; `CandidateSource` is tracked so eval can
prove LLM value against pure Bayesian search. Mock numbers are labelled `[mock-1]` and never presented as real.

## 16. Load tests are tiered: quick per commit, AIPerf for robust stress
**Decision.** `WorkloadSpec.loadgen ∈ {quick, engine, aiperf}`. `quick` (built-in streaming generator,
seconds) runs after every commit / for every candidate; `engine` uses the engine's own bench tool
(`vllm bench serve`, `sglang.bench_serving`) when numbers must match the engine's reference benchmarks;
`aiperf` (NVIDIA AIPerf) is the robust tier used sparingly — percentiles, poisson/gamma arrivals,
request-rate shaping, GPU power/util telemetry. The tier is part of the trial's content hash, so a quick
and an AIPerf measurement of the same config are distinct ledger rows. A missing tool fails loudly with
an install hint; there is no silent downgrade. `scripts/deploy-check.sh` encodes the policy
(`STRESS=1` → AIPerf). **Why.** Per-commit checks must be fast enough to run always; the robust tier
must be trustworthy enough to gate a deployment. Flags and metric tags were verified against the
mirrored sources in inference-engine-KB (`repos/aiperf`, `repos/vllm`, `repos/sglang`).

#!/usr/bin/env bash
# Incremental deployment check — run after every change to an inference deployment.
#
#   scripts/deploy-check.sh                       # quick tier: built-in load generator, seconds
#   STRESS=1 scripts/deploy-check.sh              # robust tier: NVIDIA AIPerf (percentiles, poisson arrivals, GPU power)
#   LOADGEN=engine scripts/deploy-check.sh        # the engine's own bench tool (vllm bench serve / sglang.bench_serving)
#
# Env: ENGINE (vllm|sglang|mock), MODEL, SKU, CONFIG (JSON), CONC, REPEATS, RUN (ledger run id).
# Exit non-zero if the trial fails or the result is flagged noisy (so CI treats flaky measurements as failures).
set -euo pipefail
BIN="${BIN:-$(dirname "$0")/../target/release/autoinference}"
[ -x "$BIN" ] || BIN="$(dirname "$0")/../target/debug/autoinference"
ENGINE="${ENGINE:-mock}"; MODEL="${MODEL:-meta-llama/Llama-3.1-8B-Instruct}"; SKU="${SKU:-h100-sxm}"
[ -n "${CONFIG:-}" ] || CONFIG="{}"; CONC="${CONC:-32}"; REPEATS="${REPEATS:-3}"; RUN="${RUN:-deploy-check}"
if [ "${STRESS:-0}" = "1" ]; then TIER=aiperf; REQ="${REQ:-512}"; ARR='"arrival":"poisson",'; else TIER="${LOADGEN:-quick}"; REQ="${REQ:-128}"; ARR=''; fi
ACCESS=observe; [ "$ENGINE" != "mock" ] && ACCESS=tune
echo "deploy-check: engine=$ENGINE sku=$SKU tier=$TIER conc=$CONC requests=$REQ repeats=$REPEATS run=$RUN" >&2
OUT=$("$BIN" --access "$ACCESS" trial --engine "$ENGINE" --model "$MODEL" --sku "$SKU" --config "$CONFIG" \
      --workload "{${ARR}\"concurrency\":$CONC,\"requests\":$REQ,\"loadgen\":\"$TIER\"}" --repeats "$REPEATS" --run "$RUN" --json 2>"${TMPDIR:-/tmp}/deploy-check.err" | tail -1)
[ -n "$OUT" ] || { echo "deploy-check: trial produced no result:" >&2; tail -5 "${TMPDIR:-/tmp}/deploy-check.err" >&2; exit 1; }
OUT="$OUT" python3 - <<'PY'
import os, sys, json
d = json.loads(os.environ["OUT"]); b = d.get("bench") or {}
print("tok/s %.1f  ttft p50/p99 %.0f/%.0f ms  tpot %.2f ms  noisy=%s  verdict=%s  cached=%s  loadgen=%s" % (
    b.get("tok_s", 0), b.get("ttft_p50_ms", 0), b.get("ttft_p99_ms", 0), b.get("tpot_ms", 0),
    b.get("noisy"), d.get("verdict"), d.get("cached"), d.get("loadgen")))
sys.exit(2 if b.get("noisy") else 0)
PY

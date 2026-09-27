# autoinference

Agentic LLM inference optimization: turn raw model weights into a peak-performance deployment on
NVIDIA GPUs (Ampere · Hopper · Blackwell) — engine selection → config tuning → kernel synthesis →
validation — with a wandb-style dashboard and an MCP surface.

**Status:** alpha `v0.0.1`. The agentic CLI harness in [`harness/`](harness/) is a working walking
skeleton (Rust core + Python sidecar, every layer wired end-to-end). The package-registry entries
below are name reservations that will be pointed at the harness as it stabilizes.

## The harness

```bash
cd harness
cargo build --release
python3 -m venv sidecar/.venv && sidecar/.venv/bin/pip install cbor2
export AUTOINFERENCE_KB_DIR=/path/to/inference-engine-KB     # machine-extracted knob registry
./target/release/autoinference doctor
./target/release/autoinference kb backends --cc 10.0          # what vLLM auto-picks on Blackwell
./target/release/autoinference --provider mock exec --json "hw: b200"   # offline, JSONL events
export ANTHROPIC_API_KEY=... && ./target/release/autoinference tui
```

See [harness/README.md](harness/README.md), [harness/docs/BLUEPRINT.md](harness/docs/BLUEPRINT.md)
and [harness/docs/DECISIONS.md](harness/docs/DECISIONS.md) — every subsystem names the open-source
harness it is modeled on (codex, pi, goose, opencode, deepseek-harness, gemini-cli, crush, …).

## Install (registry placeholders)

| Ecosystem | Command |
| --- | --- |
| Python (pip) | `pip install autoinference` |
| Python (uv) | `uv add autoinference` |
| Node.js (npm) | `npm install autoinference` |
| Node.js (npx) | `npx autoinference` |
| Rust (cargo) | `cargo install autoinference` |
| Go | `go get github.com/autoinference/autoinference` |

## Repo layout

```
.
├── harness/                   # THE PRODUCT: Rust workspace (protocol, core, agent, tui, cli) + Python sidecar
├── go.mod, autoinference.go   # Go module (root, so `go get` works)
├── python/                    # PyPI package (pip + uv)
├── node/                      # npm package (npm + npx)
├── rust/                      # crates.io crate
├── mcp-skill/                 # @autoinference/skill — MCP server (placeholder)
└── helm-chart/                # Helm chart (placeholder)
```

See [PUBLISH.md](PUBLISH.md) for the publishing checklist.

## License

MIT — see [LICENSE](LICENSE).

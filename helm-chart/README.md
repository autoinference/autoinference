# Auto Inference — Helm chart

Helm chart for **[Auto Inference](https://autoinference.org)** — agentic CLI + MCP skill for production LLM inference optimization.

> ⚠️ **v0.0.1-alpha placeholder.** This chart reserves the `autoinference` name on Artifact Hub + your cluster while the real agentic deployment pipeline ships with the design-partner beta. Today it deploys a single no-op pod and exposes the `values.yaml` shape the real chart will honour, so any Helm values you template against this skeleton stay forward-compatible.

## Install

Chart repository is live at **`https://autoinference.org/charts`** (published from [`autoinference/charts`](https://github.com/autoinference/charts) via GitHub Pages):

```bash
helm repo add autoinference https://autoinference.org/charts
helm repo update
helm install autoinference autoinference/autoinference \
  --version 0.0.1-alpha.0 \
  --namespace autoinference --create-namespace
```

`helm search repo autoinference --devel` (pre-release versions require `--devel`).

## What it deploys today

| Resource | Purpose |
| --- | --- |
| `Deployment` (1 replica) | Idle alpine pod that prints a placeholder notice and sleeps |
| `Service` (ClusterIP, port 8080) | Reserves the service name |
| `ServiceAccount` | Dedicated, non-default, no auto-mount |

## What it will deploy in the beta

The `values.yaml` already exposes the configuration surface the real chart will honour:

```yaml
autoinference:
  model: "deepseek-ai/DeepSeek-V4-Pro"
  hardware: "8x B300"
  engine: auto                # sglang | tensorrt-llm | tokenspeed | vllm | lmdeploy | auto
  quantize: auto
  slo:
    p99LatencyMs: 50
    minQps: 120
    maxCostPerMillionTokens: 0.5
  dashboard:
    enabled: true
    project: kimi-prod
```

When `dashboard.enabled: true`, the Deployment annotation `autoinference.org/dashboard` is populated with a wandb-style URL `dashboard.autoinference.org/<namespace>/<project>/<release>`.

## Links

- Website: [autoinference.org](https://autoinference.org)
- Source: [github.com/autoinference/autoinference](https://github.com/autoinference/autoinference)
- npm CLI: [`autoinference`](https://www.npmjs.com/package/autoinference)
- npm MCP skill: [`@autoinference/skill`](https://www.npmjs.com/package/@autoinference/skill)
- PyPI: [`autoinference`](https://pypi.org/project/autoinference/)
- Contact: hello@autoinference.org

## License

MIT

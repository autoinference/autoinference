# @autoinference/skill

MCP server skill for **[autoinference](https://autoinference.org)** — peak inference performance, zero ML engineering.

Drop in any frontier LLM (DeepSeek V4 Pro, Kimi 2.6, GLM 5.1, ...) on any hardware (8× B300, 256× H200 multi-node, AMD MI355X, ...) and the autoinference agentic pipeline picks the optimal inference engine, tunes every config, synthesises custom kernels, and stress-tests the result.

This package exposes that pipeline as an **MCP server** so any MCP-aware coding agent — Claude Code, Codex, Cursor, Antigravity — can call into it directly from a chat.

> ⚠️ **v0.0.1-alpha · placeholder release.** The MCP surface installs and connects, but the underlying agent pipeline ships in a later release. Today's tool calls return friendly placeholder responses describing what the call would do.

## Install

### Universal (any MCP client)

```bash
npx skill add @autoinference/skill
```

Or with the canonical npm `npx`:

```bash
npx -y @autoinference/skill
```

### Per-editor

| Editor       | Command                                                       |
| ------------ | ------------------------------------------------------------- |
| Claude Code  | `claude mcp add autoinference`                                |
| Codex        | `codex mcp add autoinference`                                 |
| Cursor       | `cursor mcp add autoinference`                                |
| Antigravity  | `antigravity mcp install autoinference`                       |

For Cursor's `~/.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "autoinference": {
      "command": "npx",
      "args": ["-y", "@autoinference/skill"]
    }
  }
}
```

## Tools exposed

| Tool                       | Description                                                                       |
| -------------------------- | --------------------------------------------------------------------------------- |
| `autoinference.deploy`     | Deploy an LLM optimally on user-owned hardware. Returns a placeholder in alpha.   |
| `autoinference.benchmark`  | Run a load/latency/cost benchmark against a deployment. Placeholder in alpha.     |
| `autoinference.summary_url`| Return the wandb-style dashboard URL for a deployment. Placeholder in alpha.      |

## Status

- npm package: [`@autoinference/skill`](https://www.npmjs.com/package/@autoinference/skill)
- Source: [github.com/autoinference/autoinference](https://github.com/autoinference/autoinference)
- Website: [autoinference.org](https://autoinference.org)

## License

MIT

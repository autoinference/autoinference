#!/usr/bin/env node
// autoinference — MCP skill server (placeholder release).
//
// Speaks MCP over stdio. Once the autoinference CLI / agent pipeline ships,
// these tool stubs become thin wrappers around `autoinference deploy`,
// `autoinference benchmark`, and the dashboard URL resolver. Today they
// return friendly "in alpha" responses so MCP-aware clients (Claude Code,
// Codex, Cursor, Antigravity, ...) can install + connect to the skill and
// see the tool surface that's coming.

import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
} from "@modelcontextprotocol/sdk/types.js";

const PRODUCT_URL = "https://autoinference.org";
const ALPHA_NOTE = `autoinference is in pre-release (v0.0.1-alpha). The MCP skill is installed and connected, but the underlying agent pipeline is not yet shipped — this tool returned a placeholder response. Track progress: ${PRODUCT_URL}`;

const TOOLS = [
  {
    name: "deploy",
    description:
      "Deploy an LLM optimally on user-owned hardware. The autoinference agentic pipeline selects the best inference engine (SGLang / TensorRT-LLM / tokenspeed / vLLM / LMDeploy), tunes config, synthesises custom kernels, and stress-tests the result. Currently returns a placeholder while the pipeline is in alpha.",
    inputSchema: {
      type: "object",
      properties: {
        model: {
          type: "string",
          description:
            "Model identifier on Hugging Face or a local path (e.g. 'kimi-2.6', 'zai-org/GLM-5.1', 'deepseek-ai/DeepSeek-V4-Pro').",
        },
        hardware: {
          type: "string",
          description:
            "Target hardware string (e.g. '8x B300', '32x 8x H200', 'AMD MI355X', 'TPU v6e').",
        },
        slo: {
          type: "object",
          description:
            "Service-level objectives. Common keys: p99_ms (number), min_qps (number), max_cost_per_million_tokens (number).",
          additionalProperties: true,
        },
      },
      required: ["model", "hardware"],
    },
  },
  {
    name: "benchmark",
    description:
      "Benchmark a deployed model against your workload (throughput, p50/p95/p99 latency, GPU utilisation, cost per million tokens). Currently returns a placeholder while the pipeline is in alpha.",
    inputSchema: {
      type: "object",
      properties: {
        deployment_id: {
          type: "string",
          description: "Deployment hash returned by autoinference.deploy.",
        },
        concurrency: {
          type: "number",
          description: "Peak concurrent users to simulate.",
        },
      },
      required: ["deployment_id"],
    },
  },
  {
    name: "summary_url",
    description:
      "Return the wandb-style dashboard summary URL for a deployment so the agent can link the human to live metrics, agent run logs, and re-tune controls.",
    inputSchema: {
      type: "object",
      properties: {
        deployment_id: {
          type: "string",
          description: "Deployment hash returned by autoinference.deploy.",
        },
      },
      required: ["deployment_id"],
    },
  },
];

const server = new Server(
  { name: "autoinference", version: "0.0.1-alpha.0" },
  { capabilities: { tools: {} } }
);

server.setRequestHandler(ListToolsRequestSchema, async () => ({ tools: TOOLS }));

server.setRequestHandler(CallToolRequestSchema, async (req) => {
  const { name, arguments: args = {} } = req.params;
  return {
    content: [
      {
        type: "text",
        text: [
          `[autoinference.${name}] placeholder response`,
          "",
          `args: ${JSON.stringify(args, null, 2)}`,
          "",
          ALPHA_NOTE,
        ].join("\n"),
      },
    ],
  };
});

await server.connect(new StdioServerTransport());

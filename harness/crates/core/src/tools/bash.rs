use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

use super::{Risk, Tool, ToolContext, ToolOutput};

/// Commands that are read-only by construction; everything else needs approval.
/// (codex `execpolicy` in spirit — a real Starlark DSL lands later.)
const READ_ONLY_PREFIXES: &[&str] = &[
    "ls",
    "cat ",
    "head ",
    "tail ",
    "grep ",
    "rg ",
    "find ",
    "wc ",
    "pwd",
    "echo ",
    "which ",
    "env",
    "printenv",
    "nvidia-smi",
    "git status",
    "git log",
    "git diff",
    "git show",
    "du ",
    "df ",
    "stat ",
    "file ",
    "python3 --version",
    "python --version",
    "uname",
    "lscpu",
    "free",
    "nproc",
];

/// Commands that are never allowed regardless of mode (GPU resets, service control, prod).
const DENIED: &[&str] = &[
    "nvidia-smi -r",
    "nvidia-smi --gpu-reset",
    "dcgmi config",
    "systemctl",
    "shutdown",
    "reboot",
    "mkfs",
    "rm -rf /",
];

pub struct Bash;

#[async_trait]
impl Tool for Bash {
    fn name(&self) -> &'static str {
        "bash"
    }
    fn description(&self) -> String {
        "Run a shell command in the workspace. Output is truncated head+tail; prefer narrow commands. \
         Read-only commands run without approval; mutating ones require approval. GPU resets and service control are denied."
            .into()
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "Command to run with bash -c"},
                "timeout_s": {"type": "integer", "description": "Seconds before the command is killed (default 120)"}
            },
            "required": ["command"]
        })
    }
    fn risk(&self, input: &Value) -> Risk {
        let cmd = input["command"].as_str().unwrap_or("").trim();
        if READ_ONLY_PREFIXES.iter().any(|p| cmd.starts_with(p))
            && !cmd.contains('>')
            && !cmd.contains("| tee")
        {
            Risk::ReadOnly
        } else {
            Risk::Write
        }
    }
    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let cmd = input["command"].as_str().unwrap_or("").to_string();
        if cmd.trim().is_empty() {
            return Ok(ToolOutput::err("empty command"));
        }
        if DENIED.iter().any(|d| cmd.contains(d)) {
            return Ok(ToolOutput::err(format!("denied by policy: `{cmd}`")));
        }
        let timeout = Duration::from_secs(input["timeout_s"].as_u64().unwrap_or(120));
        let child = tokio::process::Command::new("bash")
            .arg("-lc")
            .arg(&cmd)
            .current_dir(&ctx.cwd)
            .stdin(std::process::Stdio::null())
            .output();
        let out = match tokio::time::timeout(timeout, child).await {
            Ok(r) => r?,
            Err(_) => {
                return Ok(ToolOutput::err(format!(
                    "timed out after {}s",
                    timeout.as_secs()
                )))
            }
        };
        let code = out.status.code().unwrap_or(-1);
        let mut text = String::from_utf8_lossy(&out.stdout).to_string();
        let err = String::from_utf8_lossy(&out.stderr);
        if !err.trim().is_empty() {
            text.push_str("\n[stderr]\n");
            text.push_str(&err);
        }
        if text.trim().is_empty() {
            text = format!("(no output, exit {code})");
        }
        let o = ToolOutput {
            content: text,
            is_error: code != 0,
            data: Some(json!({"exit_code": code, "command": cmd})),
        };
        Ok(o)
    }
}

//! `autoinference` binary. In `exec --json` mode stdout is a JSONL event stream and
//! nothing else may print there (codex discipline) — human output goes to stderr.
#![deny(clippy::print_stdout)]

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use autoinference_agent::runtime::AgentBuilder;
use autoinference_agent::Agent;
use autoinference_core::config::Config;
use autoinference_core::hardware::HardwareKb;
use autoinference_core::llm::make_provider;
use autoinference_core::protocol::sidecar::SidecarOp;
use autoinference_core::protocol::{AccessMode, Envelope};
use autoinference_core::session::Store;
use autoinference_core::sidecar::Sidecar;

#[derive(Parser)]
#[command(
    name = "autoinference",
    version,
    about = "Agentic inference-optimization CLI: engine selection, config tuning, kernel synthesis, validation."
)]
struct Cli {
    /// LLM provider: anthropic | mock
    #[arg(long, global = true, env = "AUTOINFERENCE_PROVIDER")]
    provider: Option<String>,
    #[arg(long, global = true, env = "AUTOINFERENCE_MODEL")]
    model: Option<String>,
    /// Blast-radius access mode for this session.
    #[arg(long, global = true, value_enum, default_value = "observe")]
    access: Access,
    /// Approve every tool call automatically (headless/CI).
    #[arg(short = 'y', long, global = true)]
    yes: bool,
    /// Disable the Python sidecar (kb_* / hw_probe tools become unavailable).
    #[arg(long, global = true)]
    no_sidecar: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum Access {
    Observe,
    Tune,
    Deploy,
}

#[derive(Subcommand)]
enum Cmd {
    /// Interactive plain-text chat (line REPL).
    Chat {
        #[arg(long)]
        session: Option<String>,
    },
    /// Full-screen terminal UI.
    Tui {
        #[arg(long)]
        session: Option<String>,
    },
    /// Headless: run one prompt; with --json, emit the JSONL event stream on stdout.
    Exec {
        prompt: String,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        session: Option<String>,
    },
    /// Hardware SKU knowledge base.
    Hw {
        #[command(subcommand)]
        cmd: HwCmd,
    },
    /// Inference-engine knob registry (via sidecar).
    Kb {
        #[command(subcommand)]
        cmd: KbCmd,
    },
    /// Sessions and their durable event logs.
    Sessions {
        #[command(subcommand)]
        cmd: SessCmd,
    },
    /// Print the JSON Schema of the event envelope (dashboard/TS codegen input).
    Schema,
    /// Sidecar diagnostics.
    Sidecar {
        #[command(subcommand)]
        cmd: SidecarCmd,
    },
    /// Show resolved configuration and discovered paths.
    Doctor,
}

#[derive(Subcommand)]
enum HwCmd {
    List,
    Query { sku: String },
    Probe,
}

#[derive(Subcommand)]
enum KbCmd {
    Summary,
    Search {
        query: String,
        #[arg(long)]
        engine: Option<String>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    Knob {
        engine: String,
        name: String,
    },
    Constraints {
        engine: String,
        terms: Vec<String>,
        #[arg(long, default_value_t = 15)]
        limit: usize,
    },
    Backends {
        #[arg(long, default_value = "vllm")]
        engine: String,
        #[arg(long)]
        cc: Option<String>,
    },
}

#[derive(Subcommand)]
enum SessCmd {
    List,
    Events {
        id: String,
        #[arg(long, default_value_t = 0)]
        after: u64,
    },
    Stats {
        id: String,
    },
}

#[derive(Subcommand)]
enum SidecarCmd {
    Ping,
}

fn eprintln_json(v: &serde_json::Value) {
    eprintln!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let mut cfg = Config::load()?;
    if let Some(p) = cli.provider {
        cfg.provider = p;
    }
    if let Some(m) = cli.model {
        cfg.model = m;
    }
    cfg.blast_radius.mode = match cli.access {
        Access::Observe => AccessMode::Observe,
        Access::Tune => AccessMode::Tune,
        Access::Deploy => AccessMode::Deploy,
    };
    cfg.auto_approve = cli.yes || matches!(cli.cmd, Cmd::Exec { .. });
    std::fs::create_dir_all(&cfg.data_dir)?;

    match cli.cmd {
        Cmd::Doctor => {
            eprintln!("provider     : {}", cfg.provider);
            eprintln!("model        : {}", cfg.model);
            eprintln!("data_dir     : {}", cfg.data_dir.display());
            eprintln!(
                "kb_dir       : {}",
                cfg.kb_dir
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or("(not found — set AUTOINFERENCE_KB_DIR)".into())
            );
            eprintln!(
                "sidecar_dir  : {}",
                cfg.sidecar_dir
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or("(not found)".into())
            );
            eprintln!("python       : {}", cfg.python);
            eprintln!(
                "anthropic key: {}",
                if std::env::var("ANTHROPIC_API_KEY").is_ok() {
                    "set"
                } else {
                    "unset"
                }
            );
            let hw = HardwareKb::load()?;
            eprintln!(
                "hardware kb  : {} SKUs (schema v{})",
                hw.list().len(),
                hw.schema_version
            );
            Ok(())
        }
        Cmd::Schema => {
            let schema = schemars::schema_for!(Envelope);
            let mut out = std::io::stdout().lock();
            writeln!(out, "{}", serde_json::to_string_pretty(&schema)?)?;
            Ok(())
        }
        Cmd::Hw { cmd } => {
            let hw = HardwareKb::load()?;
            match cmd {
                HwCmd::List => {
                    for s in hw.list() {
                        eprintln!("{:<24} {:<10} cc {:<5} {:>3} SMs  {:>3} GB @ {:>4} GB/s  fp8={} fp4={}", s.id, s.architecture, s.compute_capability, s.sm_count, s.hbm_gb, s.hbm_bandwidth_gbs, s.fp8, s.fp4);
                    }
                }
                HwCmd::Query { sku } => eprintln_json(&serde_json::json!(hw.find(&sku))),
                HwCmd::Probe => {
                    let sc = spawn_sidecar(&cfg).await?;
                    eprintln_json(&sc.call(SidecarOp::HwProbe, Duration::from_secs(30)).await?);
                }
            }
            Ok(())
        }
        Cmd::Kb { cmd } => {
            let sc = spawn_sidecar(&cfg).await?;
            let op = match cmd {
                KbCmd::Summary => SidecarOp::KbSummary,
                KbCmd::Search {
                    query,
                    engine,
                    limit,
                } => SidecarOp::KbSearch {
                    engine,
                    query,
                    limit,
                },
                KbCmd::Knob { engine, name } => SidecarOp::KbKnob { engine, name },
                KbCmd::Constraints {
                    engine,
                    terms,
                    limit,
                } => SidecarOp::KbConstraints {
                    engine,
                    terms,
                    limit,
                },
                KbCmd::Backends { engine, cc } => SidecarOp::KbAttentionBackends {
                    engine,
                    compute_capability: cc,
                },
            };
            eprintln_json(&sc.call(op, Duration::from_secs(60)).await?);
            Ok(())
        }
        Cmd::Sidecar {
            cmd: SidecarCmd::Ping,
        } => {
            let sc = spawn_sidecar(&cfg).await?;
            let t = std::time::Instant::now();
            let v = sc.call(SidecarOp::Ping, Duration::from_secs(10)).await?;
            eprintln!("pong in {:?}: {v}", t.elapsed());
            Ok(())
        }
        Cmd::Sessions { cmd } => {
            let store = Store::open(&cfg.db_path(), &cfg.transcripts_dir())?;
            match cmd {
                SessCmd::List => {
                    for s in store.list_sessions(50)? {
                        eprintln!(
                            "{}  {}  {}",
                            s.id,
                            s.created_at.format("%Y-%m-%d %H:%M"),
                            s.title.unwrap_or_default()
                        );
                    }
                }
                SessCmd::Events { id, after } => {
                    let mut out = std::io::stdout().lock();
                    for e in store.events_since(&id, after, 10_000)? {
                        writeln!(out, "{}", serde_json::to_string(&e)?)?;
                    }
                }
                SessCmd::Stats { id } => {
                    let (u, cost) = store.usage(&id)?;
                    eprintln!(
                        "usage: in {} cached {} out {}  cost ${cost:.4}",
                        u.input_tokens, u.cached_input_tokens, u.output_tokens
                    );
                    for (n, c) in store.event_counts(&id)? {
                        eprintln!("{c:>6}  {n}");
                    }
                }
            }
            Ok(())
        }
        Cmd::Exec {
            prompt,
            json,
            session,
        } => {
            let agent = build_agent(&cfg, session, !cli.no_sidecar).await?;
            if json {
                let mut rx = agent.rt.bus.subscribe();
                let printer = tokio::spawn(async move {
                    while let Some(env) = rx.recv().await {
                        let line = serde_json::to_string(&*env).unwrap_or_default();
                        let mut out = std::io::stdout().lock();
                        let _ = writeln!(out, "{line}");
                        let _ = out.flush();
                    }
                });
                let r = agent.run_turn(&prompt).await;
                // let the printer drain
                tokio::time::sleep(Duration::from_millis(50)).await;
                printer.abort();
                r.map(|_| ())
            } else {
                let mut rx = agent.rt.bus.subscribe();
                let streamer = tokio::spawn(async move {
                    while let Some(env) = rx.recv().await {
                        if let autoinference_core::protocol::Event::ItemDelta { delta, .. } =
                            &env.event
                        {
                            eprint!("{delta}");
                        }
                    }
                });
                let text = agent.run_turn(&prompt).await?;
                streamer.abort();
                eprintln!();
                let mut out = std::io::stdout().lock();
                writeln!(out, "{text}")?;
                Ok(())
            }
        }
        Cmd::Chat { session } => {
            let agent = build_agent(&cfg, session, !cli.no_sidecar).await?;
            eprintln!(
                "autoinference chat — session {} — model {} — access {:?}. Ctrl-D to exit.",
                agent.session_id(),
                cfg.model,
                cfg.blast_radius.mode
            );
            let mut rx = agent.rt.bus.subscribe();
            tokio::spawn(async move {
                while let Some(env) = rx.recv().await {
                    match &env.event {
                        autoinference_core::protocol::Event::ItemDelta { delta, .. } => {
                            eprint!("{delta}")
                        }
                        autoinference_core::protocol::Event::ItemStarted { item } => {
                            if let autoinference_core::protocol::ThreadItemDetails::ToolCall {
                                tool,
                                input,
                                ..
                            } = &item.details
                            {
                                eprintln!(
                                    "\n  ⚙ {tool} {}",
                                    serde_json::to_string(input).unwrap_or_default()
                                );
                            }
                        }
                        autoinference_core::protocol::Event::ItemCompleted { item } => {
                            if let autoinference_core::protocol::ThreadItemDetails::ToolCall {
                                tool,
                                status,
                                ..
                            } = &item.details
                            {
                                eprintln!(
                                    "  {} {tool}",
                                    if *status
                                        == autoinference_core::protocol::ItemStatus::Completed
                                    {
                                        "✓"
                                    } else {
                                        "✗"
                                    }
                                );
                            }
                        }
                        _ => {}
                    }
                }
            });
            let stdin = std::io::stdin();
            loop {
                eprint!("\nyou ▸ ");
                let mut line = String::new();
                if stdin.read_line(&mut line)? == 0 {
                    break;
                }
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                eprint!("ai  ▸ ");
                if let Err(e) = agent.run_turn(line).await {
                    eprintln!("\n[error] {e:#}");
                }
                eprintln!();
            }
            Ok(())
        }
        Cmd::Tui { session } => {
            let agent = build_agent(&cfg, session, !cli.no_sidecar).await?;
            autoinference_tui::run(agent).await
        }
    }
}

async fn spawn_sidecar(cfg: &Config) -> Result<Arc<Sidecar>> {
    let dir = cfg
        .sidecar_dir
        .clone()
        .context("sidecar dir not found (set AUTOINFERENCE_SIDECAR_DIR)")?;
    Sidecar::spawn(&cfg.python, &dir, cfg.kb_dir.as_deref()).await
}

async fn build_agent(
    cfg: &Config,
    session: Option<String>,
    want_sidecar: bool,
) -> Result<Arc<Agent>> {
    let provider = make_provider(&cfg.provider)?;
    let store = Store::open(&cfg.db_path(), &cfg.transcripts_dir())?;
    let hardware = Arc::new(HardwareKb::load()?);
    let sidecar = if want_sidecar && cfg.sidecar_dir.is_some() {
        match spawn_sidecar(cfg).await {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!(error = %e, "sidecar unavailable; continuing without kb_* tools");
                None
            }
        }
    } else {
        None
    };
    let mut tools = autoinference_core::tools::ToolRegistry::standard(sidecar.is_some());
    if !cfg.auto_approve {
        tools.set_approver(Arc::new(|name: &str, input: &serde_json::Value| {
            eprint!(
                "\n[approve] {name} {} ? [y/N] ",
                serde_json::to_string(input).unwrap_or_default()
            );
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            matches!(line.trim(), "y" | "Y" | "yes")
        }));
    }
    Agent::build(AgentBuilder {
        config: cfg.clone(),
        provider,
        store,
        hardware,
        sidecar,
        tools: Some(tools),
        cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        resume: session,
    })
    .await
}

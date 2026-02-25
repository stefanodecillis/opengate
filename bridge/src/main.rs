use chrono::Local;
use clap::Parser;
use serde::Deserialize;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::time::{self, Duration};

// ── Logging ─────────────────────────────────────────────────────────

macro_rules! log {
    ($agent:expr, $($arg:tt)*) => {
        eprintln!("[{}] [{}] {}", Local::now().format("%Y-%m-%d %H:%M:%S"), $agent, format!($($arg)*))
    };
}

macro_rules! log_global {
    ($($arg:tt)*) => {
        eprintln!("[{}] {}", Local::now().format("%Y-%m-%d %H:%M:%S"), format!($($arg)*))
    };
}

// ── CLI ─────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name = "opengate-bridge",
    about = "Lightweight OpenGate agent polling daemon"
)]
struct Cli {
    /// Path to TOML config file
    #[arg(short, long, env = "OPENGATE_BRIDGE_CONFIG")]
    config: PathBuf,

    /// Run one poll cycle then exit (for cron)
    #[arg(long)]
    once: bool,

    /// Only process this agent (by name)
    #[arg(long)]
    agent: Option<String>,
}

// ── Config ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Config {
    server: ServerConfig,
    #[serde(default)]
    agents: Vec<AgentConfig>,
}

#[derive(Debug, Deserialize)]
struct ServerConfig {
    url: String,
    #[serde(default = "default_heartbeat_interval")]
    heartbeat_interval: u64,
    #[serde(default = "default_poll_interval")]
    poll_interval: u64,
}

fn default_heartbeat_interval() -> u64 {
    300
}
fn default_poll_interval() -> u64 {
    60
}

#[derive(Debug, Deserialize, Clone)]
struct AgentConfig {
    name: String,
    api_key_file: String,
    #[serde(default = "default_wake_mode")]
    wake_mode: WakeMode,
    /// For openclaw wake mode
    openclaw_id: Option<String>,
    /// For webhook wake mode
    webhook_url: Option<String>,
    /// For command wake mode
    command: Option<String>,
}

fn default_wake_mode() -> WakeMode {
    WakeMode::Stdout
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
enum WakeMode {
    Openclaw,
    Webhook,
    Command,
    Stdout,
}

// ── API types ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Notification {
    #[allow(dead_code)]
    id: serde_json::Value,
    #[serde(alias = "type", alias = "event_type")]
    notification_type: String,
    title: String,
    #[serde(alias = "message", alias = "body")]
    body: Option<String>,
}

// WebhookPayload and NotificationSummary removed — webhook mode now uses simple JSON

// ── Resolved agent (config with key loaded) ─────────────────────────

struct ResolvedAgent {
    name: String,
    api_url: String,
    api_key: String,
    poll_interval: Duration,
    heartbeat_interval: Duration,
    wake_mode: WakeMode,
    openclaw_id: Option<String>,
    webhook_url: Option<String>,
    command: Option<String>,
}

impl ResolvedAgent {
    fn from_config(cfg: &AgentConfig, server: &ServerConfig) -> Result<Self, String> {
        let api_key = std::fs::read_to_string(&cfg.api_key_file)
            .map_err(|e| {
                format!(
                    "agent '{}': can't read key file '{}': {}",
                    cfg.name, cfg.api_key_file, e
                )
            })?
            .trim()
            .to_string();

        if api_key.is_empty() {
            return Err(format!(
                "agent '{}': key file '{}' is empty",
                cfg.name, cfg.api_key_file
            ));
        }

        // Validate wake mode config
        match cfg.wake_mode {
            WakeMode::Openclaw if cfg.openclaw_id.is_none() => {
                return Err(format!(
                    "agent '{}': openclaw wake_mode requires openclaw_id",
                    cfg.name
                ));
            }
            WakeMode::Webhook if cfg.webhook_url.is_none() => {
                return Err(format!(
                    "agent '{}': webhook wake_mode requires webhook_url",
                    cfg.name
                ));
            }
            WakeMode::Command if cfg.command.is_none() => {
                return Err(format!(
                    "agent '{}': command wake_mode requires command",
                    cfg.name
                ));
            }
            _ => {}
        }

        Ok(Self {
            name: cfg.name.clone(),
            api_url: server.url.trim_end_matches('/').to_string(),
            api_key,
            poll_interval: Duration::from_secs(server.poll_interval),
            heartbeat_interval: Duration::from_secs(server.heartbeat_interval),
            wake_mode: cfg.wake_mode.clone(),
            openclaw_id: cfg.openclaw_id.clone(),
            webhook_url: cfg.webhook_url.clone(),
            command: cfg.command.clone(),
        })
    }
}

// ── Core logic ──────────────────────────────────────────────────────

async fn do_heartbeat(client: &reqwest::Client, agent: &ResolvedAgent) {
    let url = format!("{}/api/agents/heartbeat", agent.api_url);
    match client
        .post(&url)
        .header("Authorization", format!("Bearer {}", agent.api_key))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            log!(agent.name, "Heartbeat OK");
        }
        Ok(resp) => {
            log!(agent.name, "Heartbeat failed (HTTP {})", resp.status());
        }
        Err(e) => {
            log!(agent.name, "Heartbeat error: {}", e);
        }
    }
}

/// Track whether a wake process is already running for an agent.
/// Uses an Arc<AtomicBool> per agent to prevent double-waking.
async fn poll_and_wake(
    client: &reqwest::Client,
    agent: &ResolvedAgent,
    waking: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    // If a wake is already in progress, skip this poll cycle
    if waking.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }

    // Fetch unread notifications
    let url = format!("{}/api/agents/me/notifications?unread=true", agent.api_url);
    let resp = match client
        .get(&url)
        .header("Authorization", format!("Bearer {}", agent.api_key))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            log!(agent.name, "Poll error: {}", e);
            return;
        }
    };

    if !resp.status().is_success() {
        log!(agent.name, "Poll failed (HTTP {})", resp.status());
        return;
    }

    let notifications: Vec<Notification> = match resp.json().await {
        Ok(n) => n,
        Err(e) => {
            log!(agent.name, "Failed to parse notifications: {}", e);
            return;
        }
    };

    if notifications.is_empty() {
        return;
    }

    log!(
        agent.name,
        "{} notifications, waking via {:?} (fire-and-forget)",
        notifications.len(),
        agent.wake_mode
    );

    // Fire-and-forget: spawn the wake as a background task.
    // The AGENT is responsible for acking its own notifications after processing.
    // Bridge only detects and wakes — it does NOT ack.
    waking.store(true, std::sync::atomic::Ordering::Relaxed);
    let name = agent.name.clone();
    let wake_mode = agent.wake_mode.clone();
    let openclaw_id = agent.openclaw_id.clone();
    let webhook_url = agent.webhook_url.clone();
    let command = agent.command.clone();
    let summary = build_summary(&notifications);
    let waking_flag = waking.clone();

    tokio::spawn(async move {
        let ok = do_wake(&name, &wake_mode, &openclaw_id, &webhook_url, &command, &summary).await;
        if !ok {
            log!(name, "Wake failed");
        }
        waking_flag.store(false, std::sync::atomic::Ordering::Relaxed);
    });
}

/// Perform the actual wake call (extracted from wake_agent for spawned task use)
async fn do_wake(
    agent_name: &str,
    wake_mode: &WakeMode,
    openclaw_id: &Option<String>,
    webhook_url: &Option<String>,
    command: &Option<String>,
    summary: &str,
) -> bool {
    match wake_mode {
        WakeMode::Stdout => {
            eprintln!("[{}] Notifications:\n{}", agent_name, summary);
            true
        }
        WakeMode::Openclaw => {
            let oc_id = openclaw_id.as_ref().unwrap();
            let message = format!("OpenGate: {}", summary);

            match tokio::process::Command::new("openclaw")
                .args(["agent", "--agent", oc_id, "--message", &message])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .status()
                .await
            {
                Ok(s) if s.success() => {
                    log!(agent_name, "OpenClaw wake succeeded");
                    true
                }
                Ok(s) => {
                    log!(
                        agent_name,
                        "OpenClaw wake failed (exit {})",
                        s.code().unwrap_or(-1)
                    );
                    false
                }
                Err(e) => {
                    log!(agent_name, "OpenClaw exec error: {}", e);
                    false
                }
            }
        }
        WakeMode::Webhook => {
            let url = webhook_url.as_ref().unwrap();
            let payload = serde_json::json!({
                "agent": agent_name,
                "summary": summary,
            });

            match reqwest::Client::new().post(url).json(&payload).send().await {
                Ok(r) if r.status().is_success() => {
                    log!(agent_name, "Webhook wake succeeded");
                    true
                }
                Ok(r) => {
                    log!(agent_name, "Webhook wake failed (HTTP {})", r.status());
                    false
                }
                Err(e) => {
                    log!(agent_name, "Webhook error: {}", e);
                    false
                }
            }
        }
        WakeMode::Command => {
            let cmd = command.as_ref().unwrap();

            match tokio::process::Command::new("sh")
                .args(["-c", cmd])
                .env("OPENGATE_NOTIFICATIONS", summary)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .status()
                .await
            {
                Ok(s) if s.success() => {
                    log!(agent_name, "Command wake succeeded");
                    true
                }
                Ok(s) => {
                    log!(
                        agent_name,
                        "Command wake failed (exit {})",
                        s.code().unwrap_or(-1)
                    );
                    false
                }
                Err(e) => {
                    log!(agent_name, "Command exec error: {}", e);
                    false
                }
            }
        }
    }
}

fn build_summary(notifications: &[Notification]) -> String {
    notifications
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let msg = n.body.as_deref().unwrap_or("");
            if msg.is_empty() {
                format!("{}. [{}] {}", i + 1, n.notification_type, n.title)
            } else {
                format!("{}. [{}] {} — {}", i + 1, n.notification_type, n.title, msg)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// wake_agent removed — replaced by fire-and-forget do_wake in spawned task

// ── Agent loop ──────────────────────────────────────────────────────

async fn run_agent_loop(agent: ResolvedAgent) {
    let client = reqwest::Client::new();
    let waking = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut hb_interval = time::interval(agent.heartbeat_interval);
    let mut poll_interval = time::interval(agent.poll_interval);

    // First tick fires immediately
    do_heartbeat(&client, &agent).await;
    poll_and_wake(&client, &agent, &waking).await;

    loop {
        tokio::select! {
            _ = hb_interval.tick() => {
                do_heartbeat(&client, &agent).await;
            }
            _ = poll_interval.tick() => {
                poll_and_wake(&client, &agent, &waking).await;
            }
        }
    }
}

async fn run_once(agent: &ResolvedAgent) {
    let client = reqwest::Client::new();
    let waking = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    do_heartbeat(&client, agent).await;
    poll_and_wake(&client, agent, &waking).await;
    // In one-shot mode, wait a bit for the spawned wake to start
    time::sleep(Duration::from_secs(5)).await;
}

// ── Main ────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // Load config
    let text = match std::fs::read_to_string(&cli.config) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Error reading config '{}': {}", cli.config.display(), e);
            std::process::exit(1);
        }
    };

    let config: Config = match toml::from_str(&text) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error parsing config: {}", e);
            std::process::exit(1);
        }
    };

    if config.agents.is_empty() {
        eprintln!("No agents configured in config file");
        std::process::exit(1);
    }

    // Resolve agents (load keys, validate config)
    let mut agents: Vec<ResolvedAgent> = Vec::new();
    for cfg in &config.agents {
        // Filter by --agent if specified
        if let Some(ref filter) = cli.agent {
            if cfg.name != *filter {
                continue;
            }
        }

        match ResolvedAgent::from_config(cfg, &config.server) {
            Ok(a) => agents.push(a),
            Err(e) => {
                eprintln!("Config error: {}", e);
                std::process::exit(1);
            }
        }
    }

    if agents.is_empty() {
        if let Some(ref filter) = cli.agent {
            eprintln!("No agent named '{}' found in config", filter);
        } else {
            eprintln!("No agents configured");
        }
        std::process::exit(1);
    }

    log_global!("Starting bridge for {} agent(s)", agents.len());

    // --once mode: single poll cycle
    if cli.once {
        for agent in &agents {
            run_once(agent).await;
        }
        return;
    }

    // Daemon mode: spawn a task per agent with staggered starts
    let mut handles = Vec::new();
    for (i, agent) in agents.into_iter().enumerate() {
        // Stagger agent starts by 2 seconds each
        let delay = Duration::from_secs(i as u64 * 2);
        handles.push(tokio::spawn(async move {
            if !delay.is_zero() {
                log!(agent.name, "Starting in {}s (staggered)", delay.as_secs());
                time::sleep(delay).await;
            }
            run_agent_loop(agent).await;
        }));
    }

    // Wait for ctrl+c
    match tokio::signal::ctrl_c().await {
        Ok(()) => log_global!("Shutting down"),
        Err(e) => eprintln!("Signal handler error: {}", e),
    }
}

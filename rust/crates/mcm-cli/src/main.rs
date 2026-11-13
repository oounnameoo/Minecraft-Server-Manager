mod client;
mod config;

use std::io::{self, Write};

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use futures_util::StreamExt;
use reqwest::Method;
use serde_json::{Value, json};

use crate::client::{ApiClient, ServerSummary};
use crate::config::{default_config_path, load_config, save_config};

#[derive(Parser)]
#[command(
    name = "mcm-cli",
    about = "Proxmox-style CLI for Minecraft Container Manager (MCM)"
)]
struct Cli {
    /// MCM panel URL (default: saved config or https://localhost:8080)
    #[arg(long, global = true, env = "MCM_CLI_URL")]
    url: Option<String>,

    /// Skip TLS certificate verification (for self-signed certs)
    #[arg(short = 'k', long, global = true, env = "MCM_CLI_INSECURE")]
    insecure: bool,

    /// Output raw JSON
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Authenticate and save session locally
    Login {
        #[arg(long)]
        email: Option<String>,
        #[arg(long)]
        password: Option<String>,
        #[arg(long)]
        totp: Option<String>,
    },
    /// Log out and clear saved session
    Logout,
    /// Show current authenticated user
    Whoami,
    /// Show node status or a specific server's live status
    Status { server: Option<String> },
    /// List all servers (like qm list)
    #[command(alias = "ls")]
    List,
    /// Show server configuration (like qm config)
    Config { server: String },
    /// Update server configuration (like qm set)
    Set {
        server: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        version: Option<String>,
        #[arg(long)]
        ram: Option<i64>,
        #[arg(long)]
        port: Option<i64>,
        #[arg(long)]
        cpu_limit: Option<f64>,
        #[arg(long)]
        memory_limit: Option<i64>,
        #[arg(long)]
        java: Option<i64>,
        #[arg(long)]
        auto_start: Option<String>,
        #[arg(long)]
        start_cmd: Option<String>,
    },
    /// Create a new server
    Create {
        #[arg(long)]
        name: String,
        #[arg(long = "type", alias = "kind", default_value = "paper")]
        server_type: String,
        #[arg(long, default_value = "1.21.4")]
        version: String,
        #[arg(long, default_value_t = 2048)]
        ram: i64,
        #[arg(long, default_value_t = 0)]
        port: i64,
        #[arg(long)]
        auto_start: bool,
        #[arg(long, default_value = "")]
        start_cmd: String,
    },
    /// Delete a server
    Destroy {
        server: String,
        #[arg(long)]
        yes: bool,
    },
    /// Start a server
    Start { server: String },
    /// Stop a server gracefully
    #[command(alias = "shutdown")]
    Stop { server: String },
    /// Restart a server
    #[command(alias = "reboot")]
    Restart { server: String },
    /// Force kill a server
    Kill { server: String },
    /// Attach to live server console (Ctrl+C to detach)
    #[command(alias = "terminal")]
    Console { server: String },
    /// Send a single console command
    Exec {
        server: String,
        #[arg(required = true, trailing_var_arg = true)]
        command: Vec<String>,
    },
    /// Manage backups/snapshots
    #[command(alias = "snapshot", alias = "backups")]
    Backup {
        #[command(subcommand)]
        action: BackupCmd,
    },
    /// Browse and manage server files
    #[command(alias = "file")]
    Files {
        #[command(subcommand)]
        action: FilesCmd,
    },
    /// Manage panel users
    #[command(alias = "users")]
    User {
        #[command(subcommand)]
        action: UserCmd,
    },
    /// Call any MCM API endpoint directly (like pvesh)
    Api {
        method: String,
        path: String,
        body: Option<String>,
    },
    /// Show CLI build info
    Version,
}

#[derive(Subcommand)]
enum BackupCmd {
    #[command(alias = "ls")]
    List {
        server: String,
    },
    Create {
        server: String,
        #[arg(long, default_value = "cli-backup")]
        name: String,
        #[arg(long, default_value = "local")]
        storage: String,
    },
    Restore {
        server: String,
        backup_id: String,
        #[arg(long)]
        yes: bool,
    },
    #[command(alias = "rm")]
    Delete {
        backup_id: String,
        #[arg(long)]
        yes: bool,
    },
    Cancel {
        server: String,
    },
}

#[derive(Subcommand)]
enum FilesCmd {
    Ls {
        server: String,
        path: Option<String>,
    },
    Cat {
        server: String,
        path: String,
    },
    Rm {
        server: String,
        path: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum UserCmd {
    #[command(alias = "ls")]
    List,
    Create {
        #[arg(long)]
        email: String,
        #[arg(long)]
        password: String,
        #[arg(long, default_value = "viewer")]
        role: String,
    },
    #[command(alias = "rm")]
    Delete {
        user_id: String,
        #[arg(long)]
        yes: bool,
    },
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let cfg_path = default_config_path()?;
    let mut cfg = load_config(&cfg_path)?;

    let url = cli.url.unwrap_or_else(|| cfg.url.clone());
    let insecure = cli.insecure || cfg.insecure;
    let json_out = cli.json;

    let client = ApiClient::new(&url, insecure, cfg.cookies.clone())?;

    match cli.command {
        Commands::Version => {
            let info = mcm_core::buildinfo::current();
            if json_out {
                println!("{}", serde_json::to_string_pretty(&info)?);
            } else {
                println!(
                    "mcm-cli (rust) commit={} dirty={} built={}",
                    info.commit, info.dirty, info.built_at
                );
            }
        }
        Commands::Login {
            email,
            password,
            totp,
        } => {
            let email = match email {
                Some(e) if !e.trim().is_empty() => e.trim().to_string(),
                _ => {
                    eprint!("Email: ");
                    io::stderr().flush()?;
                    let mut line = String::new();
                    io::stdin().read_line(&mut line)?;
                    line.trim().to_string()
                }
            };
            let password = match password {
                Some(p) if !p.is_empty() => p,
                _ => rpassword::prompt_password("Password: ")?,
            };
            if email.is_empty() || password.is_empty() {
                bail!("email and password are required");
            }
            let login_client = ApiClient::new(&url, insecure, vec![])?;
            let mut body = json!({ "email": email, "password": password });
            if let Some(code) = totp.filter(|c| !c.is_empty()) {
                body["totp_code"] = Value::String(code);
            }
            let resp = login_client
                .request_json(Method::POST, "/api/auth/login", Some(body))
                .await?;
            cfg.url = login_client.base_url().to_string();
            cfg.insecure = insecure;
            cfg.cookies = login_client.cookies();
            save_config(&cfg_path, &cfg)?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&resp)?);
            } else {
                println!(
                    "Logged in to {} (session saved to {})",
                    cfg.url,
                    cfg_path.display()
                );
            }
        }
        Commands::Logout => {
            let _ = client
                .request_json(Method::POST, "/api/auth/logout", Some(json!({})))
                .await;
            cfg.cookies.clear();
            save_config(&cfg_path, &cfg)?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&json!({"ok": true}))?);
            } else {
                println!("Logged out.");
            }
        }
        Commands::Whoami => {
            let resp = client
                .request_json(Method::GET, "/api/auth/me", None)
                .await?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&resp)?);
            } else {
                let u = resp.get("user").unwrap_or(&resp);
                let email = u.get("email").and_then(|v| v.as_str()).unwrap_or("");
                let role = u.get("role").and_then(|v| v.as_str()).unwrap_or("");
                let id = u.get("id").and_then(|v| v.as_str()).unwrap_or("");
                println!("{email} ({role}) [{id}]");
            }
        }
        Commands::Status { server } => {
            if let Some(target) = server {
                let id = client.resolve_server_id(&target).await?;
                let st = client
                    .request_json(Method::GET, &format!("/api/servers/{id}/status"), None)
                    .await?;
                let stats = client
                    .request_json(Method::GET, &format!("/api/servers/{id}/stats"), None)
                    .await
                    .unwrap_or(Value::Null);
                if json_out {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&json!({"status": st, "stats": stats}))?
                    );
                } else {
                    let state = st
                        .get("state")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    println!("status: {state}");
                    if let Some(up) = st
                        .get("uptime_seconds")
                        .and_then(|v| v.as_i64())
                        .filter(|&u| u > 0)
                    {
                        println!("uptime: {up}s");
                    }
                    if let Some(cpu) = stats.get("cpu_percent").and_then(|v| v.as_f64()) {
                        let mem = stats
                            .get("memory_bytes")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0);
                        println!("cpu:    {cpu:.2}%");
                        println!("mem:    {:.1} MB", mem / (1024.0 * 1024.0));
                    }
                }
            } else {
                let cap = client
                    .request_json(Method::GET, "/api/system/capacity", None)
                    .await?;
                let srvs = client
                    .request_json(Method::GET, "/api/servers", None)
                    .await?;
                if json_out {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&json!({"capacity": cap, "servers": srvs}))?
                    );
                } else {
                    let runtime = cap
                        .get("runtime")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let cores = cap.get("cpu_cores").and_then(|v| v.as_i64()).unwrap_or(0);
                    let mem = cap
                        .get("memory_total_bytes")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let disk_total = cap
                        .get("disk")
                        .and_then(|d| d.get("total_bytes"))
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let disk_free = cap
                        .get("disk")
                        .and_then(|d| d.get("available_bytes"))
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let list: Vec<ServerSummary> = serde_json::from_value(
                        srvs.get("servers").cloned().unwrap_or(Value::Array(vec![])),
                    )?;
                    let running = list.iter().filter(|s| s.state == "running").count();
                    println!("Panel:    {}", client.base_url());
                    println!("Runtime:  {runtime}");
                    println!("CPU:      {cores} cores");
                    if mem > 0.0 {
                        println!("Memory:   {:.1} GB", mem / (1024.0 * 1024.0 * 1024.0));
                    }
                    if disk_total > 0.0 {
                        println!(
                            "Disk:     {:.1} GB free / {:.1} GB total",
                            disk_free / (1024.0 * 1024.0 * 1024.0),
                            disk_total / (1024.0 * 1024.0 * 1024.0)
                        );
                    }
                    println!("Servers:  {} total ({running} running)", list.len());
                }
            }
        }
        Commands::List => {
            let resp = client
                .request_json(Method::GET, "/api/servers", None)
                .await?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&resp)?);
            } else {
                let list: Vec<ServerSummary> = serde_json::from_value(
                    resp.get("servers").cloned().unwrap_or(Value::Array(vec![])),
                )?;
                if list.is_empty() {
                    println!("No servers found.");
                } else {
                    println!(
                        "{:<36}  {:<16}  {:<10}  {:<8}  {:<5}  {:<7}  {:<9}  STATE",
                        "ID", "NAME", "TYPE", "VERSION", "PORT", "RAM(MB)", "AUTOSTART"
                    );
                    for s in list {
                        let auto = if s.auto_start { "yes" } else { "no" };
                        println!(
                            "{:<36}  {:<16}  {:<10}  {:<8}  {:<5}  {:<7}  {:<9}  {}",
                            s.id,
                            s.name,
                            s.server_type,
                            s.version,
                            s.host_port,
                            s.ram_mb,
                            auto,
                            s.state
                        );
                    }
                }
            }
        }
        Commands::Config { server } => {
            let id = client.resolve_server_id(&server).await?;
            let resp = client
                .request_json(Method::GET, &format!("/api/servers/{id}"), None)
                .await?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&resp)?);
            } else {
                let s: ServerSummary = serde_json::from_value(resp)?;
                println!("id: {}", s.id);
                println!("name: {}", s.name);
                println!("server_type: {}", s.server_type);
                println!("version: {}", s.version);
                println!("ram_mb: {}", s.ram_mb);
                println!("host_port: {}", s.host_port);
                println!("cpu_limit: {:.2}", s.cpu_limit);
                println!("memory_limit_mb: {}", s.memory_limit_mb);
                println!("java_version: {}", s.java_version);
                println!("auto_start: {}", s.auto_start);
                println!("start_command: {}", s.start_command);
                println!("state: {}", s.state);
            }
        }
        Commands::Set {
            server,
            name,
            version,
            ram,
            port,
            cpu_limit,
            memory_limit,
            java,
            auto_start,
            start_cmd,
        } => {
            let mut map = serde_json::Map::new();
            if let Some(v) = name {
                map.insert("name".into(), Value::String(v));
            }
            if let Some(v) = version {
                map.insert("version".into(), Value::String(v));
            }
            if let Some(v) = ram {
                map.insert("ram_mb".into(), json!(v));
            }
            if let Some(v) = port {
                map.insert("host_port".into(), json!(v));
            }
            if let Some(v) = cpu_limit {
                map.insert("cpu_limit".into(), json!(v));
            }
            if let Some(v) = memory_limit {
                map.insert("memory_limit_mb".into(), json!(v));
            }
            if let Some(v) = java {
                map.insert("java_version".into(), json!(v));
            }
            if let Some(raw) = auto_start {
                let b = match raw.trim().to_ascii_lowercase().as_str() {
                    "true" | "1" | "yes" | "on" | "enable" | "enabled" => true,
                    "false" | "0" | "no" | "off" | "disable" | "disabled" => false,
                    _ => bail!("invalid --auto-start value {raw:?} (use true or false)"),
                };
                map.insert("auto_start".into(), Value::Bool(b));
            }
            if let Some(cmd) = start_cmd {
                map.insert("start_command".into(), Value::String(cmd));
            }
            if map.is_empty() {
                bail!("no fields specified to update");
            }
            let id = client.resolve_server_id(&server).await?;
            let resp = client
                .request_json(
                    Method::PATCH,
                    &format!("/api/servers/{id}"),
                    Some(Value::Object(map)),
                )
                .await?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&resp)?);
            } else {
                println!("Updated configuration for {id}");
            }
        }
        Commands::Create {
            name,
            server_type,
            version,
            ram,
            port,
            auto_start,
            start_cmd,
        } => {
            let mut body = json!({
                "name": name,
                "server_type": server_type,
                "version": version,
                "ram_mb": ram,
                "auto_start": auto_start,
                "start_command": start_cmd,
            });
            if port > 0 {
                body["host_port"] = json!(port);
            }
            let resp = client
                .request_json(Method::POST, "/api/servers", Some(body))
                .await?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&resp)?);
            } else {
                let s: ServerSummary = serde_json::from_value(resp)?;
                println!(
                    "Created server {} ({}) on port {}",
                    s.name, s.id, s.host_port
                );
            }
        }
        Commands::Destroy { server, yes } => {
            let id = client.resolve_server_id(&server).await?;
            if !yes && !confirm(&format!("Delete server {id}? [y/N]: "))? {
                bail!("aborted");
            }
            client
                .request_json(Method::DELETE, &format!("/api/servers/{id}"), None)
                .await?;
            if json_out {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({"ok": true, "id": id}))?
                );
            } else {
                println!("Deleted server {id}");
            }
        }
        Commands::Start { server } => power(&client, &server, "start", json_out).await?,
        Commands::Stop { server } => power(&client, &server, "stop", json_out).await?,
        Commands::Restart { server } => power(&client, &server, "restart", json_out).await?,
        Commands::Kill { server } => power(&client, &server, "kill", json_out).await?,
        Commands::Exec { server, command } => {
            let id = client.resolve_server_id(&server).await?;
            let cmd_str = command.join(" ");
            let resp = client
                .request_json(
                    Method::POST,
                    &format!("/api/servers/{id}/console/command"),
                    Some(json!({ "command": cmd_str })),
                )
                .await?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&resp)?);
            } else {
                println!("Sent command to {id}: {cmd_str}");
            }
        }
        Commands::Console { server } => {
            let id = client.resolve_server_id(&server).await?;
            let resp = client
                .open_stream(&format!("/api/servers/{id}/console"))
                .await?;
            eprintln!(
                "Attached to console for {id} (type commands and press Enter, Ctrl+C to detach)"
            );
            let cmd_client = client.clone();
            let cmd_id = id.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let trimmed = line.trim().to_string();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Err(err) = cmd_client
                        .request_json(
                            Method::POST,
                            &format!("/api/servers/{cmd_id}/console/command"),
                            Some(json!({ "command": trimmed })),
                        )
                        .await
                    {
                        eprintln!("command error: {err}");
                    }
                }
            });

            let mut stream = resp.bytes_stream();
            let mut buf = String::new();
            while let Some(chunk) = stream.next().await {
                let bytes = chunk?;
                buf.push_str(&String::from_utf8_lossy(&bytes));
                while let Some(pos) = buf.find('\n') {
                    let line = buf[..pos].trim_end_matches('\r').to_string();
                    buf.drain(..=pos);
                    if let Some(payload) = line.strip_prefix("data:") {
                        let payload = payload.trim();
                        if payload.is_empty() {
                            continue;
                        }
                        if let Ok(v) = serde_json::from_str::<Value>(payload) {
                            let msg = v
                                .get("message")
                                .or_else(|| v.get("line"))
                                .or_else(|| v.get("raw"))
                                .and_then(|m| m.as_str());
                            if let Some(m) = msg {
                                println!("{m}");
                            } else {
                                println!("{payload}");
                            }
                        } else {
                            println!("{payload}");
                        }
                    }
                }
            }
        }
        Commands::Backup { action } => match action {
            BackupCmd::List { server } => {
                let id = client.resolve_server_id(&server).await?;
                let resp = client
                    .request_json(Method::GET, &format!("/api/servers/{id}/backups"), None)
                    .await?;
                if json_out {
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                } else if let Some(arr) = resp.get("backups").and_then(|v| v.as_array()) {
                    if arr.is_empty() {
                        println!("No backups found.");
                    } else {
                        for b in arr {
                            println!(
                                "{}\t{}\t{}\t{}",
                                b.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                                b.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                                b.get("status").and_then(|v| v.as_str()).unwrap_or(""),
                                b.get("size_bytes").and_then(|v| v.as_i64()).unwrap_or(0),
                            );
                        }
                    }
                }
            }
            BackupCmd::Create {
                server,
                name,
                storage,
            } => {
                let id = client.resolve_server_id(&server).await?;
                let resp = client
                    .request_json(
                        Method::POST,
                        &format!("/api/servers/{id}/backup"),
                        Some(json!({ "name": name, "storage": storage })),
                    )
                    .await?;
                if json_out {
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                } else {
                    println!(
                        "Backup created: {}",
                        resp.get("id").and_then(|v| v.as_str()).unwrap_or("")
                    );
                }
            }
            BackupCmd::Restore {
                server,
                backup_id,
                yes,
            } => {
                let id = client.resolve_server_id(&server).await?;
                if !yes && !confirm(&format!("Restore backup {backup_id} onto {id}? [y/N]: "))? {
                    bail!("aborted");
                }
                let resp = client
                    .request_json(
                        Method::POST,
                        &format!(
                            "/api/servers/{id}/restore/{}",
                            urlencoding::encode(&backup_id)
                        ),
                        Some(json!({})),
                    )
                    .await?;
                if json_out {
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                } else {
                    println!("Restored backup {backup_id} onto {id}");
                }
            }
            BackupCmd::Delete { backup_id, yes } => {
                if !yes && !confirm(&format!("Delete backup {backup_id}? [y/N]: "))? {
                    bail!("aborted");
                }
                client
                    .request_json(
                        Method::DELETE,
                        &format!("/api/backups/{}", urlencoding::encode(&backup_id)),
                        None,
                    )
                    .await?;
                if json_out {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&json!({"ok": true, "id": backup_id}))?
                    );
                } else {
                    println!("Deleted backup {backup_id}");
                }
            }
            BackupCmd::Cancel { server } => {
                let id = client.resolve_server_id(&server).await?;
                let resp = client
                    .request_json(
                        Method::POST,
                        &format!("/api/servers/{id}/backups/cancel"),
                        Some(json!({})),
                    )
                    .await?;
                println!("{}", serde_json::to_string_pretty(&resp)?);
            }
        },
        Commands::Files { action } => match action {
            FilesCmd::Ls { server, path } => {
                let id = client.resolve_server_id(&server).await?;
                let q = urlencoding::encode(path.as_deref().unwrap_or(""));
                let resp = client
                    .request_json(
                        Method::GET,
                        &format!("/api/servers/{id}/files?path={q}"),
                        None,
                    )
                    .await?;
                if json_out {
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                } else if let Some(arr) = resp.get("entries").and_then(|v| v.as_array()) {
                    for e in arr {
                        let kind = if e.get("is_dir").and_then(|v| v.as_bool()).unwrap_or(false) {
                            "dir"
                        } else {
                            "file"
                        };
                        let size = e.get("size").and_then(|v| v.as_i64()).unwrap_or(0);
                        let name = e.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        println!("{kind}\t{size}\t{name}");
                    }
                }
            }
            FilesCmd::Cat { server, path } => {
                let id = client.resolve_server_id(&server).await?;
                let q = urlencoding::encode(&path);
                let resp = client
                    .request_json(
                        Method::GET,
                        &format!("/api/servers/{id}/files/content?path={q}"),
                        None,
                    )
                    .await?;
                if json_out {
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                } else {
                    print!(
                        "{}",
                        resp.get("content").and_then(|v| v.as_str()).unwrap_or("")
                    );
                }
            }
            FilesCmd::Rm { server, path, yes } => {
                let id = client.resolve_server_id(&server).await?;
                if !yes && !confirm(&format!("Delete {path} on {id}? [y/N]: "))? {
                    bail!("aborted");
                }
                let q = urlencoding::encode(&path);
                client
                    .request_json(
                        Method::DELETE,
                        &format!("/api/servers/{id}/files?path={q}"),
                        None,
                    )
                    .await?;
                println!("Deleted {path}");
            }
        },
        Commands::User { action } => match action {
            UserCmd::List => {
                let resp = client.request_json(Method::GET, "/api/users", None).await?;
                if json_out {
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                } else if let Some(arr) = resp.get("users").and_then(|v| v.as_array()) {
                    for u in arr {
                        println!(
                            "{}\t{}\t{}",
                            u.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                            u.get("email").and_then(|v| v.as_str()).unwrap_or(""),
                            u.get("role").and_then(|v| v.as_str()).unwrap_or(""),
                        );
                    }
                }
            }
            UserCmd::Create {
                email,
                password,
                role,
            } => {
                let resp = client
                    .request_json(
                        Method::POST,
                        "/api/users",
                        Some(json!({
                            "email": email,
                            "password": password,
                            "access": { "role": role }
                        })),
                    )
                    .await?;
                if json_out {
                    println!("{}", serde_json::to_string_pretty(&resp)?);
                } else {
                    println!("Created user {email} ({role})");
                }
            }
            UserCmd::Delete { user_id, yes } => {
                if !yes && !confirm(&format!("Delete user {user_id}? [y/N]: "))? {
                    bail!("aborted");
                }
                client
                    .request_json(
                        Method::DELETE,
                        &format!("/api/users/{}", urlencoding::encode(&user_id)),
                        None,
                    )
                    .await?;
                println!("Deleted user {user_id}");
            }
        },
        Commands::Api { method, path, body } => {
            let m = Method::from_bytes(method.trim().to_ascii_uppercase().as_bytes())?;
            let path = if path.starts_with('/') {
                path
            } else {
                format!("/{path}")
            };
            let json_body = match body {
                Some(raw) => Some(serde_json::from_str::<Value>(&raw)?),
                None => None,
            };
            let resp = client.request_json(m, &path, json_body).await?;
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
    }
    Ok(())
}

async fn power(client: &ApiClient, server: &str, action: &str, json_out: bool) -> Result<()> {
    let id = client.resolve_server_id(server).await?;
    let resp = client
        .request_json(
            Method::POST,
            &format!("/api/servers/{id}/{action}"),
            Some(json!({})),
        )
        .await?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&resp)?);
    } else {
        println!("Server {id}: {action}");
    }
    Ok(())
}

fn confirm(prompt: &str) -> Result<bool> {
    eprint!("{prompt}");
    io::stderr().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let ans = line.trim().to_ascii_lowercase();
    Ok(ans == "y" || ans == "yes")
}

use anyhow::{Context, Result, anyhow};
use rusqlite::{Connection, params};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex as AsyncMutex, broadcast};

const MAX_LOG_LINES: usize = 1000;

#[derive(Debug, Clone, Serialize)]
pub struct ProcessStats {
    pub cpu_percent: f64,
    pub memory_used_mb: f64,
    pub memory_limit_mb: f64,
    pub memory_percent: f64,
    pub network_rx_bytes: u64,
    pub network_tx_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct StartOpts {
    pub server_id: String,
    pub server_dir: PathBuf,
    pub ram_mb: i64,
    #[allow(dead_code)]
    pub memory_limit_mb: i64,
    pub java_version: i64,
    pub start_command: String,
    pub host_port: u16,
}

struct RunningProcess {
    pid: Option<u32>,
    child: Child,
    stdin: Option<ChildStdin>,
    logs: Arc<Mutex<VecDeque<String>>>,
    log_tx: broadcast::Sender<String>,
}

#[derive(Clone)]
pub struct NativeRuntime {
    processes: Arc<AsyncMutex<HashMap<String, RunningProcess>>>,
    db: Arc<Mutex<Connection>>,
}

impl NativeRuntime {
    pub fn new(db: Arc<Mutex<Connection>>) -> Self {
        Self {
            processes: Arc::new(AsyncMutex::new(HashMap::new())),
            db,
        }
    }

    pub async fn java_version_string(&self) -> Option<String> {
        let out = Command::new("java").arg("-version").output().await.ok()?;
        let text = String::from_utf8_lossy(&out.stderr).to_string()
            + &String::from_utf8_lossy(&out.stdout);
        text.lines().next().map(|s| s.trim().to_string())
    }

    pub fn resolve_java_bin(java_version: i64) -> String {
        if java_version > 0 {
            let candidates = [
                format!("/usr/lib/jvm/java-{java_version}-openjdk-amd64/bin/java"),
                format!("/usr/lib/jvm/java-{java_version}-openjdk-arm64/bin/java"),
                format!("/usr/lib/jvm/java-{java_version}-openjdk/bin/java"),
                format!("/usr/lib/jvm/temurin-{java_version}-jre/bin/java"),
            ];
            for c in candidates {
                if Path::new(&c).exists() {
                    return c;
                }
            }
        }
        "java".to_string()
    }

    pub async fn is_running(&self, server_id: &str) -> bool {
        let mut procs = self.processes.lock().await;
        if let Some(p) = procs.get_mut(server_id) {
            match p.child.try_wait() {
                Ok(None) => return true,
                _ => {
                    procs.remove(server_id);
                    return false;
                }
            }
        }
        false
    }

    pub async fn start(&self, opts: StartOpts) -> Result<u32> {
        if self.is_running(&opts.server_id).await {
            let procs = self.processes.lock().await;
            return Ok(procs.get(&opts.server_id).and_then(|p| p.pid).unwrap_or(0));
        }

        std::fs::create_dir_all(&opts.server_dir).context("create server dir")?;

        // Auto-accept EULA if missing
        let eula_path = opts.server_dir.join("eula.txt");
        if !eula_path.exists() {
            let _ = std::fs::write(&eula_path, "eula=true\n");
        }

        // Ensure server.properties has the configured port if missing
        let props_path = opts.server_dir.join("server.properties");
        if !props_path.exists() && opts.host_port > 0 {
            let _ = std::fs::write(&props_path, format!("server-port={}\n", opts.host_port));
        }

        let mut cmd = if !opts.start_command.trim().is_empty() {
            let mut c = Command::new("sh");
            c.arg("-c").arg(opts.start_command.trim());
            c
        } else {
            let java_bin = Self::resolve_java_bin(opts.java_version);
            let mut c = Command::new(java_bin);
            c.arg(format!("-Xms{}M", opts.ram_mb))
                .arg(format!("-Xmx{}M", opts.ram_mb))
                .arg("-XX:+UseG1GC")
                .arg("-XX:+ParallelRefProcEnabled")
                .arg("-XX:MaxGCPauseMillis=200");
            if opts.server_dir.join("server.jar").is_file() {
                c.arg("-jar").arg("server.jar").arg("nogui");
            } else if let Some(unix_args) = find_unix_args(&opts.server_dir) {
                c.arg(format!("@{unix_args}")).arg("nogui");
            } else {
                c.arg("-jar").arg("server.jar").arg("nogui");
            }
            c
        };

        cmd.current_dir(&opts.server_dir)
            .env("SERVER_RAM_MB", opts.ram_mb.to_string())
            .env("SERVER_PORT", opts.host_port.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().context("spawn minecraft server process")?;
        let pid = child.id();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let logs = Arc::new(Mutex::new(VecDeque::with_capacity(MAX_LOG_LINES)));
        let (log_tx, _) = broadcast::channel(256);

        if let Some(out) = stdout {
            let logs_clone = logs.clone();
            let tx_clone = log_tx.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(out).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    push_log(&logs_clone, &tx_clone, line);
                }
            });
        }

        if let Some(err) = stderr {
            let logs_clone = logs.clone();
            let tx_clone = log_tx.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(err).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    push_log(&logs_clone, &tx_clone, line);
                }
            });
        }

        let mut procs = self.processes.lock().await;
        procs.insert(
            opts.server_id.clone(),
            RunningProcess {
                pid,
                child,
                stdin,
                logs,
                log_tx,
            },
        );

        // Background watcher to update DB state when process exits
        let procs_ref = self.processes.clone();
        let db_ref = self.db.clone();
        let sid = opts.server_id;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let exited = {
                    let mut map = procs_ref.lock().await;
                    if let Some(p) = map.get_mut(&sid) {
                        matches!(p.child.try_wait(), Ok(Some(_)) | Err(_))
                    } else {
                        true
                    }
                };
                if exited {
                    let mut map = procs_ref.lock().await;
                    map.remove(&sid);
                    if let Ok(conn) = db_ref.lock() {
                        let now = chrono::Utc::now().to_rfc3339();
                        let _ = conn.execute(
                            "UPDATE servers SET state='stopped', updated_at=?1 WHERE id=?2",
                            params![now, sid],
                        );
                    }
                    break;
                }
            }
        });

        Ok(pid.unwrap_or(0))
    }

    pub async fn send_command(&self, server_id: &str, command: &str) -> Result<()> {
        let mut procs = self.processes.lock().await;
        let proc = procs
            .get_mut(server_id)
            .ok_or_else(|| anyhow!("server is not running"))?;
        let stdin = proc
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("server stdin closed"))?;
        let line = format!("{}\n", command.trim_end_matches(['\r', '\n']));
        stdin
            .write_all(line.as_bytes())
            .await
            .context("write command to stdin")?;
        stdin.flush().await.ok();
        Ok(())
    }

    pub async fn stop(&self, server_id: &str, timeout_secs: u64) -> Result<()> {
        let _ = self.send_command(server_id, "stop").await;
        for _ in 0..(timeout_secs * 2) {
            if !self.is_running(server_id).await {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        self.kill(server_id).await
    }

    pub async fn kill(&self, server_id: &str) -> Result<()> {
        let mut procs = self.processes.lock().await;
        if let Some(mut p) = procs.remove(server_id) {
            let _ = p.child.kill().await;
            let _ = p.child.wait().await;
        }
        Ok(())
    }

    pub async fn logs_tail(&self, server_id: &str, server_dir: &Path, tail: usize) -> Vec<String> {
        let procs = self.processes.lock().await;
        if let Some(p) = procs.get(server_id) {
            let guard = p.logs.lock().unwrap();
            let skip = guard.len().saturating_sub(tail);
            return guard.iter().skip(skip).cloned().collect();
        }
        drop(procs);

        let latest = server_dir.join("logs").join("latest.log");
        if let Ok(content) = tokio::fs::read_to_string(&latest).await {
            let all: Vec<String> = content.lines().map(String::from).collect();
            let skip = all.len().saturating_sub(tail);
            return all.into_iter().skip(skip).collect();
        }
        Vec::new()
    }

    pub async fn subscribe_logs(
        &self,
        server_id: &str,
    ) -> Option<(Vec<String>, broadcast::Receiver<String>)> {
        let procs = self.processes.lock().await;
        let p = procs.get(server_id)?;
        let snapshot: Vec<String> = p.logs.lock().unwrap().iter().cloned().collect();
        let rx = p.log_tx.subscribe();
        Some((snapshot, rx))
    }

    pub async fn stats(&self, server_id: &str, ram_mb: i64) -> ProcessStats {
        let pid = {
            let procs = self.processes.lock().await;
            procs.get(server_id).and_then(|p| p.pid)
        };
        let mut mem_used_mb = 0.0;
        if let Some(pid) = pid
            && let Ok(status) = tokio::fs::read_to_string(format!("/proc/{pid}/status")).await
        {
            for line in status.lines() {
                if let Some(rest) = line.strip_prefix("VmRSS:")
                    && let Some(kb_str) = rest.split_whitespace().next()
                    && let Ok(kb) = kb_str.parse::<f64>()
                {
                    mem_used_mb = kb / 1024.0;
                }
            }
        }
        let limit_mb = ram_mb.max(1) as f64;
        ProcessStats {
            cpu_percent: 0.0,
            memory_used_mb: mem_used_mb,
            memory_limit_mb: limit_mb,
            memory_percent: (mem_used_mb / limit_mb) * 100.0,
            network_rx_bytes: 0,
            network_tx_bytes: 0,
        }
    }
}

fn push_log(logs: &Arc<Mutex<VecDeque<String>>>, tx: &broadcast::Sender<String>, line: String) {
    {
        let mut guard = logs.lock().unwrap();
        if guard.len() >= MAX_LOG_LINES {
            guard.pop_front();
        }
        guard.push_back(line.clone());
    }
    let _ = tx.send(line);
}

fn find_unix_args(server_dir: &Path) -> Option<String> {
    let lib_dir = server_dir.join("libraries");
    if !lib_dir.is_dir() {
        return None;
    }
    let mut stack = vec![lib_dir];
    while let Some(dir) = stack.pop() {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.file_name().and_then(|n| n.to_str()) == Some("unix_args.txt")
                    && let Ok(rel) = path.strip_prefix(server_dir)
                {
                    return Some(rel.to_string_lossy().to_string());
                }
            }
        }
    }
    None
}

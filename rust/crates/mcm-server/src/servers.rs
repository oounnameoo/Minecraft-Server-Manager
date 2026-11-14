use crate::auth::User;
use crate::error::ApiError;
use crate::jars::download_server_jar;
use crate::ports::allocate_port;
use crate::runtime::{NativeRuntime, StartOpts};
use axum::{
    Extension, Json,
    extract::{
        Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::{
        IntoResponse,
        sse::{Event, KeepAlive, Sse},
    },
};
use chrono::Utc;
use mcm_core::config::Config;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Server {
    pub id: String,
    pub name: String,
    pub server_type: String,
    pub version: String,
    pub build: String,
    pub ram_mb: i64,
    pub cpu_limit: f64,
    pub memory_limit_mb: i64,
    pub host_port: u16,
    pub extra_ports: Vec<serde_json::Value>,
    pub container_id: String,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: String,
    pub container_config: Option<serde_json::Value>,
    pub java_version: i64,
    pub auto_start: bool,
    pub start_command: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateServerInput {
    pub name: String,
    #[serde(alias = "software")]
    pub server_type: Option<String>,
    pub version: String,
    pub ram_mb: i64,
    #[serde(default)]
    pub cpu_limit: f64,
    #[serde(default)]
    pub memory_limit_mb: i64,
    #[serde(default, alias = "port")]
    pub host_port: Option<u16>,
    #[serde(default = "default_java_version")]
    pub java_version: i64,
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default)]
    pub start_command: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub jvm_flags: String,
}

fn default_java_version() -> i64 {
    21
}

#[derive(Debug, Deserialize)]
pub struct UpdateServerInput {
    pub name: Option<String>,
    pub ram_mb: Option<i64>,
    pub cpu_limit: Option<f64>,
    pub memory_limit_mb: Option<i64>,
    #[serde(alias = "port")]
    pub host_port: Option<u16>,
    pub java_version: Option<i64>,
    pub auto_start: Option<bool>,
    pub start_command: Option<String>,
    pub version: Option<String>,
    pub server_type: Option<String>,
}

#[derive(Clone)]
pub struct ServerStore {
    pub db: Arc<Mutex<Connection>>,
    pub cfg: Arc<Config>,
    pub runtime: NativeRuntime,
    pub http: reqwest::Client,
    pub install_tx: broadcast::Sender<(String, String)>,
}

impl ServerStore {
    pub fn new(db: Arc<Mutex<Connection>>, cfg: Arc<Config>, runtime: NativeRuntime) -> Self {
        let (install_tx, _) = broadcast::channel(128);
        Self {
            db,
            cfg,
            runtime,
            http: reqwest::Client::new(),
            install_tx,
        }
    }

    pub fn server_dir(&self, id: &str) -> PathBuf {
        PathBuf::from(&self.cfg.data_dir).join("servers").join(id)
    }

    pub fn list(&self) -> Result<Vec<Server>, ApiError> {
        let conn = self.db.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, server_type, version, COALESCE(build,''), ram_mb, cpu_limit, memory_limit_mb,
                    host_port, COALESCE(extra_ports,'[]'), COALESCE(container_id,''), state, created_at,
                    updated_at, COALESCE(started_at,''), COALESCE(container_config,''),
                    COALESCE(java_version, 21), COALESCE(auto_start, 0), COALESCE(start_command, '')
             FROM servers ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], row_to_server)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn get(&self, id: &str) -> Result<Server, ApiError> {
        let conn = self.db.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, server_type, version, COALESCE(build,''), ram_mb, cpu_limit, memory_limit_mb,
                    host_port, COALESCE(extra_ports,'[]'), COALESCE(container_id,''), state, created_at,
                    updated_at, COALESCE(started_at,''), COALESCE(container_config,''),
                    COALESCE(java_version, 21), COALESCE(auto_start, 0), COALESCE(start_command, '')
             FROM servers WHERE id = ?1",
        )?;
        stmt.query_row(params![id], row_to_server)
            .optional()?
            .ok_or_else(|| ApiError::not_found("server not found"))
    }

    pub async fn create(&self, input: CreateServerInput) -> Result<Server, ApiError> {
        let name = input.name.trim().to_string();
        if name.is_empty() || input.version.trim().is_empty() || input.ram_mb <= 0 {
            return Err(ApiError::bad_request(
                "name, version, and ram_mb are required",
            ));
        }

        let server_type = input
            .server_type
            .unwrap_or_else(|| "paper".to_string())
            .trim()
            .to_lowercase();
        let host_port = allocate_port(&self.db, self.cfg.port_range, input.host_port)?;
        let cpu_limit = if input.cpu_limit > 0.0 {
            input.cpu_limit
        } else {
            self.cfg.default_cpu_limit
        };
        let memory_limit_mb = if input.memory_limit_mb > 0 {
            input.memory_limit_mb
        } else {
            self.cfg.default_memory_mb
        };

        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let dir = self.server_dir(&id);
        std::fs::create_dir_all(&dir).map_err(|e| ApiError::internal(e.to_string()))?;

        // Download server jar (non-custom)
        if server_type != "custom" {
            let _ = self.install_tx.send((
                id.clone(),
                format!("Downloading {server_type} {}...", input.version),
            ));
            if let Err(e) = download_server_jar(&self.http, &server_type, &input.version, &dir).await {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(ApiError::internal(format!("download jar: {e}")));
            }
        }

        {
            let conn = self.db.lock().unwrap();
            conn.execute(
                "INSERT INTO servers (id, name, server_type, version, build, ram_mb, cpu_limit, memory_limit_mb,
                                      host_port, extra_ports, container_id, state, created_at, updated_at,
                                      started_at, container_config, java_version, auto_start, start_command)
                 VALUES (?1, ?2, ?3, ?4, '', ?5, ?6, ?7, ?8, '[]', '', 'stopped', ?9, ?10, '', '', ?11, ?12, ?13)",
                params![
                    id,
                    name,
                    server_type,
                    input.version.trim(),
                    input.ram_mb,
                    cpu_limit,
                    memory_limit_mb,
                    host_port as i64,
                    now,
                    now,
                    input.java_version,
                    if input.auto_start { 1 } else { 0 },
                    input.start_command,
                ],
            )?;
        }

        self.get(&id)
    }

    pub fn update(&self, id: &str, input: UpdateServerInput) -> Result<Server, ApiError> {
        let mut srv = self.get(id)?;
        if let Some(n) = input.name {
            let trimmed = n.trim().to_string();
            if !trimmed.is_empty() {
                srv.name = trimmed;
            }
        }
        if let Some(r) = input.ram_mb
            && r > 0
        {
            srv.ram_mb = r;
        }
        if let Some(c) = input.cpu_limit {
            srv.cpu_limit = c;
        }
        if let Some(m) = input.memory_limit_mb {
            srv.memory_limit_mb = m;
        }
        if let Some(p) = input.host_port
            && p > 0
            && p != srv.host_port
        {
            let _ = allocate_port(&self.db, self.cfg.port_range, Some(p))?;
            srv.host_port = p;
        }
        if let Some(j) = input.java_version {
            srv.java_version = j;
        }
        if let Some(a) = input.auto_start {
            srv.auto_start = a;
        }
        if let Some(sc) = input.start_command {
            srv.start_command = sc;
        }
        if let Some(v) = input.version
            && !v.trim().is_empty()
        {
            srv.version = v.trim().to_string();
        }
        if let Some(st) = input.server_type
            && !st.trim().is_empty()
        {
            srv.server_type = st.trim().to_lowercase();
        }
        let now = Utc::now().to_rfc3339();
        let conn = self.db.lock().unwrap();
        conn.execute(
            "UPDATE servers SET name=?1, server_type=?2, version=?3, ram_mb=?4, cpu_limit=?5,
                                memory_limit_mb=?6, host_port=?7, java_version=?8, auto_start=?9,
                                start_command=?10, updated_at=?11 WHERE id=?12",
            params![
                srv.name,
                srv.server_type,
                srv.version,
                srv.ram_mb,
                srv.cpu_limit,
                srv.memory_limit_mb,
                srv.host_port as i64,
                srv.java_version,
                if srv.auto_start { 1 } else { 0 },
                srv.start_command,
                now,
                id,
            ],
        )?;
        drop(conn);
        self.get(id)
    }

    pub async fn delete(&self, id: &str) -> Result<(), ApiError> {
        let _ = self.get(id)?;
        let _ = self.runtime.kill(id).await;
        let dir = self.server_dir(id);
        let _ = tokio::fs::remove_dir_all(&dir).await;
        let conn = self.db.lock().unwrap();
        conn.execute("DELETE FROM servers WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub async fn start(&self, id: &str) -> Result<Server, ApiError> {
        let srv = self.get(id)?;
        let pid = self
            .runtime
            .start(StartOpts {
                server_id: srv.id.clone(),
                server_dir: self.server_dir(&srv.id),
                ram_mb: srv.ram_mb,
                memory_limit_mb: srv.memory_limit_mb,
                java_version: srv.java_version,
                start_command: srv.start_command.clone(),
                host_port: srv.host_port,
            })
            .await
            .map_err(|e| ApiError::internal(format!("start server process: {e}")))?;

        let now = Utc::now().to_rfc3339();
        {
            let conn = self.db.lock().unwrap();
            conn.execute(
                "UPDATE servers SET container_id=?1, state='running', started_at=?2, updated_at=?3 WHERE id=?4",
                params![format!("pid:{pid}"), now, now, id],
            )?;
        }
        self.get(id)
    }

    pub async fn stop(&self, id: &str) -> Result<Server, ApiError> {
        let _ = self.get(id)?;
        let _ = self.runtime.stop(id, 20).await;
        let now = Utc::now().to_rfc3339();
        {
            let conn = self.db.lock().unwrap();
            conn.execute(
                "UPDATE servers SET state='stopped', updated_at=?1 WHERE id=?2",
                params![now, id],
            )?;
        }
        self.get(id)
    }

    pub async fn kill(&self, id: &str) -> Result<Server, ApiError> {
        let _ = self.get(id)?;
        let _ = self.runtime.kill(id).await;
        let now = Utc::now().to_rfc3339();
        {
            let conn = self.db.lock().unwrap();
            conn.execute(
                "UPDATE servers SET state='stopped', updated_at=?1 WHERE id=?2",
                params![now, id],
            )?;
        }
        self.get(id)
    }

    pub async fn restart(&self, id: &str) -> Result<Server, ApiError> {
        let _ = self.stop(id).await?;
        self.start(id).await
    }

    pub async fn recreate(&self, id: &str) -> Result<Server, ApiError> {
        let _ = self.stop(id).await?;
        self.start(id).await
    }

    pub async fn auto_start_servers(&self) {
        let Ok(list) = self.list() else { return };
        for srv in list {
            if srv.auto_start {
                if let Err(err) = self.start(&srv.id).await {
                    eprintln!(
                        "auto-start failed for server {} ({}): {:?}",
                        srv.name, srv.id, err
                    );
                } else {
                    println!("auto-started server {} ({})", srv.name, srv.id);
                }
            }
        }
    }
}

fn row_to_server(row: &rusqlite::Row) -> rusqlite::Result<Server> {
    let extra_str: String = row.get(9)?;
    let cfg_str: String = row.get(15)?;
    let extra_ports = serde_json::from_str(&extra_str).unwrap_or_default();
    let container_config = if cfg_str.is_empty() {
        None
    } else {
        serde_json::from_str(&cfg_str).ok()
    };
    Ok(Server {
        id: row.get(0)?,
        name: row.get(1)?,
        server_type: row.get(2)?,
        version: row.get(3)?,
        build: row.get(4)?,
        ram_mb: row.get(5)?,
        cpu_limit: row.get(6)?,
        memory_limit_mb: row.get(7)?,
        host_port: row.get::<_, i64>(8)?.max(0) as u16,
        extra_ports,
        container_id: row.get(10)?,
        state: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
        started_at: row.get(14)?,
        container_config,
        java_version: row.get(16)?,
        auto_start: row.get::<_, i64>(17)? != 0,
        start_command: row.get(18)?,
    })
}

// --- HTTP Handlers ---

pub fn check_server_perm(user: &User, server_id: &str, perm: &str) -> Result<(), ApiError> {
    if !user.can_access_server(server_id) {
        return Err(ApiError::forbidden("no access to this server"));
    }
    if !user.has_permission(perm) {
        return Err(ApiError::forbidden(format!("missing permission {perm}")));
    }
    Ok(())
}

pub async fn handle_list_servers(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, ApiError> {
    if !user.has_permission("servers.view") {
        return Err(ApiError::forbidden("missing permission servers.view"));
    }
    let servers: Vec<Server> = store
        .list()?
        .into_iter()
        .filter(|s| user.can_access_server(&s.id))
        .collect();
    Ok(Json(serde_json::json!({ "servers": servers })))
}

pub async fn handle_create_server(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Json(input): Json<CreateServerInput>,
) -> Result<impl IntoResponse, ApiError> {
    if !user.has_permission("servers.create") {
        return Err(ApiError::forbidden("missing permission servers.create"));
    }
    let srv = store.create(input).await?;
    Ok((StatusCode::CREATED, Json(srv)))
}

pub async fn handle_get_server(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "servers.view")?;
    let mut srv = store.get(&id)?;
    srv.state = if store.runtime.is_running(&id).await {
        "running".to_string()
    } else {
        "stopped".to_string()
    };
    Ok(Json(srv))
}

pub async fn handle_update_server(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(input): Json<UpdateServerInput>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "servers.edit")?;
    let srv = store.update(&id, input)?;
    Ok(Json(srv))
}

pub async fn handle_delete_server(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "servers.delete")?;
    store.delete(&id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_start_server(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "servers.power")?;
    Ok(Json(store.start(&id).await?))
}

pub async fn handle_stop_server(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "servers.power")?;
    Ok(Json(store.stop(&id).await?))
}

pub async fn handle_kill_server(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "servers.power")?;
    Ok(Json(store.kill(&id).await?))
}

pub async fn handle_restart_server(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "servers.power")?;
    Ok(Json(store.restart(&id).await?))
}

pub async fn handle_recreate_server(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "servers.rebuild")?;
    Ok(Json(store.recreate(&id).await?))
}

pub async fn handle_server_status(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "servers.view")?;
    let srv = store.get(&id)?;
    let state = if store.runtime.is_running(&id).await {
        "running".to_string()
    } else {
        "stopped".to_string()
    };
    Ok(Json(serde_json::json!({
        "id": srv.id,
        "state": state,
        "host_port": srv.host_port,
        "container_id": srv.container_id,
    })))
}

pub async fn handle_server_stats(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "performance.read")?;
    let srv = store.get(&id)?;
    let stats = store.runtime.stats(&id, srv.ram_mb).await;
    Ok(Json(serde_json::json!(stats)))
}

#[derive(Deserialize)]
pub struct ConsoleCommandReq {
    pub command: String,
}

pub async fn handle_console_command(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(req): Json<ConsoleCommandReq>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "console.write")?;
    let _ = store.get(&id)?;
    store
        .runtime
        .send_command(&id, &req.command)
        .await
        .map_err(|e| ApiError::bad_request(format!("send command: {e}")))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct LogsQuery {
    #[serde(default = "default_tail")]
    pub tail: String,
}

fn default_tail() -> String {
    "200".to_string()
}

pub async fn handle_server_logs(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Query(q): Query<LogsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "logs.read")?;
    let _ = store.get(&id)?;
    let tail = q.tail.parse::<usize>().unwrap_or(200);
    let lines = store
        .runtime
        .logs_tail(&id, &store.server_dir(&id), tail)
        .await;
    Ok(Json(serde_json::json!({ "lines": lines })))
}

pub async fn handle_console_ws(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "console.read")?;
    let srv = store.get(&id)?;
    Ok(ws.on_upgrade(move |socket| console_ws_loop(socket, store, srv)))
}

async fn console_ws_loop(mut socket: WebSocket, store: ServerStore, srv: Server) {
    let Some((snapshot, mut rx)) = store.runtime.subscribe_logs(&srv.id).await else {
        let _ = socket.send(Message::Text("Server is stopped".into())).await;
        return;
    };
    for line in snapshot {
        if socket.send(Message::Text(line.into())).await.is_err() {
            return;
        }
    }

    loop {
        tokio::select! {
            next_log = rx.recv() => {
                match next_log {
                    Ok(line) => {
                        if socket.send(Message::Text(line.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
            next_ws = socket.recv() => {
                match next_ws {
                    Some(Ok(Message::Text(cmd))) => {
                        let _ = store.runtime.send_command(&srv.id, &cmd).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}

pub async fn handle_console_sse(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>>>, ApiError>
{
    check_server_perm(&user, &id, "console.read")?;
    let _ = store.get(&id)?;
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, std::convert::Infallible>>(64);

    if let Some((snapshot, mut log_rx)) = store.runtime.subscribe_logs(&id).await {
        tokio::spawn(async move {
            for line in snapshot {
                if tx.send(Ok(Event::default().data(line))).await.is_err() {
                    return;
                }
            }
            while let Ok(line) = log_rx.recv().await {
                if tx.send(Ok(Event::default().data(line))).await.is_err() {
                    break;
                }
            }
        });
    }

    Ok(Sse::new(tokio_stream::wrappers::ReceiverStream::new(rx)).keep_alive(KeepAlive::default()))
}

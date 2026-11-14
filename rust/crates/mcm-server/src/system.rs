use crate::auth::User;
use crate::error::ApiError;
use crate::jars::{list_builds, list_versions};
use crate::ports::{create_reservation, delete_reservation, list_reservations};
use crate::servers::ServerStore;
use axum::{
    Extension, Json,
    body::{Body, Bytes},
    extract::{Path, Query, RawQuery, State},
    http::{HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use mcm_core::buildinfo;
use serde::Deserialize;

pub async fn handle_runtime_status(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, ApiError> {
    if !user.has_permission("system.read") && !user.has_permission("servers.view") {
        return Err(ApiError::forbidden("missing permission"));
    }
    let java_ver = store.runtime.java_version_string().await;
    Ok(Json(serde_json::json!({
        "runtime": "native",
        "connected": java_ver.is_some(),
        "version": java_ver.unwrap_or_else(|| "java not found".to_string()),
        "backend": "rust",
        "panel_version": buildinfo::VERSION,
    })))
}

pub async fn handle_system_capacity(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, ApiError> {
    if !user.has_permission("system.read") && !user.has_permission("servers.view") {
        return Err(ApiError::forbidden("missing permission"));
    }
    let servers = store.list().unwrap_or_default();
    let allocated_ram_mb: i64 = servers.iter().map(|s| s.ram_mb).sum();
    let running_count = servers.iter().filter(|s| s.state == "running").count();
    Ok(Json(serde_json::json!({
        "server_count": servers.len(),
        "running_count": running_count,
        "allocated_ram_mb": allocated_ram_mb,
        "port_range_start": store.cfg.port_range.start,
        "port_range_end": store.cfg.port_range.end,
        "data_dir": store.cfg.data_dir,
        "backup_dir": store.cfg.backup_dir,
    })))
}

pub async fn handle_java_versions() -> impl IntoResponse {
    Json(serde_json::json!([
        { "version": 8, "is_lts": true, "name": "Java 8 (LTS)" },
        { "version": 11, "is_lts": true, "name": "Java 11 (LTS)" },
        { "version": 17, "is_lts": true, "name": "Java 17 (LTS)" },
        { "version": 21, "is_lts": true, "name": "Java 21 (LTS)" },
        { "version": 22, "is_lts": false, "name": "Java 22" }
    ]))
}

pub async fn handle_list_software() -> impl IntoResponse {
    Json(serde_json::json!({
        "software": [
            { "id": "paper", "name": "Paper" },
            { "id": "vanilla", "name": "Vanilla" },
            { "id": "purpur", "name": "Purpur" },
            { "id": "fabric", "name": "Fabric" },
            { "id": "velocity", "name": "Velocity" },
            { "id": "waterfall", "name": "Waterfall" },
            { "id": "custom", "name": "Custom JAR / Command" }
        ]
    }))
}

pub async fn handle_jar_versions(
    State(store): State<ServerStore>,
    Path(software): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let versions = list_versions(&store.http, &software)
        .await
        .map_err(|e| ApiError::internal(format!("fetch versions: {e}")))?;
    let metas: Vec<serde_json::Value> = versions
        .into_iter()
        .map(|name| serde_json::json!({ "name": name }))
        .collect();
    Ok(Json(metas))
}

pub async fn handle_jar_builds(
    State(store): State<ServerStore>,
    Path((software, version)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let builds = list_builds(&store.http, &software, &version)
        .await
        .map_err(|e| ApiError::internal(format!("fetch builds: {e}")))?;
    Ok(Json(builds))
}

#[derive(Deserialize)]
pub struct AvailablePortsQuery {
    pub server_id: Option<String>,
}

pub async fn handle_available_ports(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Query(query): Query<AvailablePortsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    if !user.has_permission("servers.view") {
        return Err(ApiError::forbidden("missing permission servers.view"));
    }
    let servers = store.list().unwrap_or_default();
    let reservations = list_reservations(&store.db).unwrap_or_default();

    let mut used_ports = std::collections::HashSet::new();
    let mut used_details = Vec::new();
    for s in &servers {
        if query.server_id.as_deref() == Some(s.id.as_str()) {
            continue;
        }
        if s.host_port > 0 {
            used_ports.insert(s.host_port);
            used_details.push(serde_json::json!({
                "port": s.host_port,
                "server_id": s.id,
                "server_name": s.name,
                "description": "Primary game port",
            }));
        }
        for ep in &s.extra_ports {
            if let Some(ep_port) = ep.get("host_port").and_then(|v| v.as_u64()).and_then(|p| u16::try_from(p).ok())
                && ep_port > 0
            {
                used_ports.insert(ep_port);
                let desc = ep.get("description").and_then(|v| v.as_str()).unwrap_or("");
                used_details.push(serde_json::json!({
                    "port": ep_port,
                    "server_id": s.id,
                    "server_name": s.name,
                    "description": desc,
                }));
            }
        }
    }
    for r in &reservations {
        if let Some(r_port) = r.get("port").and_then(|v| v.as_u64()).and_then(|p| u16::try_from(p).ok()) {
            used_ports.insert(r_port);
            let label = r.get("label").and_then(|v| v.as_str()).unwrap_or("");
            used_details.push(serde_json::json!({
                "port": r_port,
                "server_id": "",
                "server_name": "Reserved",
                "description": label,
            }));
        }
    }

    let pool: Vec<u16> = (store.cfg.port_range.start..=store.cfg.port_range.end).collect();
    let available: Vec<u16> = pool
        .iter()
        .copied()
        .filter(|p| !used_ports.contains(p))
        .collect();

    Ok(Json(serde_json::json!({
        "available": available,
        "pool": pool,
        "used": used_details,
    })))
}

#[derive(Deserialize)]
pub struct CheckPortQuery {
    pub port: u16,
    pub server_id: Option<String>,
}

pub async fn handle_check_port(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Query(query): Query<CheckPortQuery>,
) -> Result<impl IntoResponse, ApiError> {
    if !user.has_permission("servers.view") {
        return Err(ApiError::forbidden("missing permission servers.view"));
    }
    let servers = store.list().unwrap_or_default();
    for s in &servers {
        if query.server_id.as_deref() == Some(s.id.as_str()) {
            continue;
        }
        if s.host_port == query.port
            || s.extra_ports.iter().any(|ep| {
                ep.get("host_port")
                    .and_then(|v| v.as_u64())
                    .is_some_and(|p| p == u64::from(query.port))
            })
        {
            return Ok(Json(serde_json::json!({
                "port": query.port,
                "in_use": true,
                "message": format!("Port {} is already used by server {}", query.port, s.name),
            })));
        }
    }
    let reservations = list_reservations(&store.db).unwrap_or_default();
    if let Some(r) = reservations.into_iter().find(|r| {
        r.get("port")
            .and_then(|v| v.as_u64())
            .is_some_and(|p| p == u64::from(query.port))
    }) {
        let label = r.get("label").and_then(|v| v.as_str()).unwrap_or("");
        return Ok(Json(serde_json::json!({
            "port": query.port,
            "in_use": true,
            "message": format!("Port {} is reserved ({})", query.port, label),
        })));
    }
    Ok(Json(serde_json::json!({
        "port": query.port,
        "in_use": false,
        "message": format!("Port {} is available", query.port),
    })))
}

pub async fn handle_list_port_reservations(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, ApiError> {
    if !user.has_permission("servers.view") {
        return Err(ApiError::forbidden("missing permission servers.view"));
    }
    let list = list_reservations(&store.db)?;
    Ok(Json(serde_json::json!({ "reservations": list })))
}

#[derive(Deserialize)]
pub struct CreateReservationReq {
    pub port: u16,
    #[serde(default)]
    pub label: String,
    #[serde(default = "default_tcp")]
    pub protocol: String,
}

fn default_tcp() -> String {
    "tcp".to_string()
}

pub async fn handle_create_port_reservation(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Json(req): Json<CreateReservationReq>,
) -> Result<impl IntoResponse, ApiError> {
    if !user.has_permission("settings.manage") {
        return Err(ApiError::forbidden("missing permission settings.manage"));
    }
    let res = create_reservation(&store.db, req.port, &req.label, &req.protocol)?;
    Ok((StatusCode::CREATED, Json(res)))
}

pub async fn handle_delete_port_reservation(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    if !user.has_permission("settings.manage") {
        return Err(ApiError::forbidden("missing permission settings.manage"));
    }
    delete_reservation(&store.db, &id)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_get_settings(
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, ApiError> {
    if !user.has_permission("settings.manage") {
        return Err(ApiError::forbidden("missing permission settings.manage"));
    }
    Ok(Json(serde_json::json!({ "settings": {} })))
}

fn with_db<T>(
    db: &std::sync::Mutex<rusqlite::Connection>,
    f: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>,
) -> Result<T, ApiError> {
    let conn = db
        .lock()
        .map_err(|_| ApiError::internal("database lock poisoned"))?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS kv_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        [],
    )
    .map_err(|e| ApiError::internal(format!("db error: {e}")))?;
    f(&conn).map_err(|e| ApiError::internal(format!("db error: {e}")))
}

fn load_curseforge_key(
    db: &std::sync::Mutex<rusqlite::Connection>,
) -> (Option<String>, &'static str) {
    if let Ok(val) = std::env::var("MCM_CURSEFORGE_API_KEY")
        && !val.trim().is_empty()
    {
        return (Some(val.trim().to_string()), "env");
    }
    let stored = with_db(db, |conn| {
        let mut stmt = conn.prepare("SELECT value FROM kv_settings WHERE key = 'curseforge_api_key'")?;
        let mut rows = stmt.query([])?;
        if let Some(row) = rows.next()? {
            let v: String = row.get(0)?;
            Ok(Some(v))
        } else {
            Ok(None)
        }
    });
    if let Ok(Some(val)) = stored
        && !val.trim().is_empty()
    {
        return (Some(val.trim().to_string()), "database");
    }
    (None, "")
}

pub async fn handle_get_curseforge_settings(
    State(store): State<ServerStore>,
) -> impl IntoResponse {
    let (key, source) = load_curseforge_key(&store.db);
    Json(serde_json::json!({
        "configured": key.is_some(),
        "source": source,
    }))
}

#[derive(Deserialize)]
pub struct CurseForgeSettingsReq {
    #[serde(default)]
    pub api_key: String,
}

pub async fn handle_put_curseforge_settings(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Json(req): Json<CurseForgeSettingsReq>,
) -> Result<impl IntoResponse, ApiError> {
    if !user.has_permission("settings.manage") {
        return Err(ApiError::forbidden("missing permission settings.manage"));
    }
    let trimmed = req.api_key.trim().to_string();
    with_db(&store.db, |conn| {
        if trimmed.is_empty() {
            conn.execute("DELETE FROM kv_settings WHERE key = 'curseforge_api_key'", [])?;
        } else {
            conn.execute(
                "INSERT INTO kv_settings (key, value) VALUES ('curseforge_api_key', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [&trimmed],
            )?;
        }
        Ok(())
    })?;
    let (key, source) = load_curseforge_key(&store.db);
    Ok(Json(serde_json::json!({
        "configured": key.is_some(),
        "source": source,
    })))
}

pub async fn handle_delete_curseforge_settings(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
) -> Result<impl IntoResponse, ApiError> {
    if !user.has_permission("settings.manage") {
        return Err(ApiError::forbidden("missing permission settings.manage"));
    }
    with_db(&store.db, |conn| {
        conn.execute("DELETE FROM kv_settings WHERE key = 'curseforge_api_key'", [])?;
        Ok(())
    })?;
    let (key, source) = load_curseforge_key(&store.db);
    Ok(Json(serde_json::json!({
        "configured": key.is_some(),
        "source": source,
    })))
}

#[derive(Deserialize)]
pub struct ProxyQuery {
    pub url: String,
}

pub async fn handle_api_proxy(
    State(store): State<ServerStore>,
    Query(query): Query<ProxyQuery>,
) -> Result<Response, ApiError> {
    let parsed = reqwest::Url::parse(&query.url)
        .map_err(|_| ApiError::bad_request("invalid proxy url"))?;
    if parsed.scheme() != "https" {
        return Err(ApiError::bad_request("proxy url must use https"));
    }
    let host = parsed.host_str().unwrap_or("");
    let allowed = matches!(
        host,
        "api.modrinth.com" | "hangar.papermc.io" | "api.spiget.org" | "api.curseforge.com"
    );
    if !allowed {
        return Err(ApiError::forbidden(format!("host not allowed: {host}")));
    }

    let mut req = store
        .http
        .get(parsed.as_str())
        .header("User-Agent", "mcm/0.1.0");
    if host == "api.curseforge.com"
        && let (Some(key), _) = load_curseforge_key(&store.db)
    {
        req = req.header("x-api-key", key);
    }

    forward_upstream(req).await
}

pub async fn handle_curseforge_proxy(
    State(store): State<ServerStore>,
    method: Method,
    Path(path): Path<String>,
    RawQuery(raw_query): RawQuery,
    body: Bytes,
) -> Result<Response, ApiError> {
    let (Some(key), _) = load_curseforge_key(&store.db) else {
        return Err(ApiError::bad_request(
            "CurseForge API key is not configured",
        ));
    };
    let clean_path = path.trim_start_matches('/');
    let api_path = if clean_path.starts_with("v1/") {
        clean_path.to_string()
    } else {
        format!("v1/{clean_path}")
    };
    let url = match raw_query {
        Some(q) if !q.is_empty() => format!("https://api.curseforge.com/{api_path}?{q}"),
        _ => format!("https://api.curseforge.com/{api_path}"),
    };
    let req_method = match method {
        Method::POST => reqwest::Method::POST,
        _ => reqwest::Method::GET,
    };
    let mut req = store
        .http
        .request(req_method, &url)
        .header("User-Agent", "mcm/0.1.0")
        .header("Accept", "application/json")
        .header("x-api-key", key);
    if !body.is_empty() {
        req = req
            .header("Content-Type", "application/json")
            .body(body);
    }
    forward_upstream(req).await
}

async fn forward_upstream(req: reqwest::RequestBuilder) -> Result<Response, ApiError> {
    let resp = req
        .send()
        .await
        .map_err(|e| ApiError::internal(format!("upstream request failed: {e}")))?;
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| ApiError::internal(format!("read upstream body: {e}")))?;

    let mut builder = Response::builder().status(status);
    if let Ok(val) = HeaderValue::from_str(&content_type) {
        builder = builder.header(header::CONTENT_TYPE, val);
    }
    builder
        .body(Body::from(bytes))
        .map_err(|e| ApiError::internal(format!("build proxy response: {e}")))
}

fn mods_folder_for_type(server_type: &str) -> &'static str {
    match server_type.trim().to_lowercase().as_str() {
        "fabric" | "forge" | "neoforge" | "quilt" => "mods",
        _ => "plugins",
    }
}

fn sanitize_jar_filename(name: &str) -> Result<String, ApiError> {
    let base = std::path::Path::new(name.trim())
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if base.is_empty() || base.contains("..") || base.contains('/') || base.contains('\\') {
        return Err(ApiError::bad_request("invalid filename"));
    }
    Ok(base.to_string())
}

pub async fn handle_list_mods(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    if !user.has_permission("servers.view") {
        return Err(ApiError::forbidden("missing permission servers.view"));
    }
    let srv = store.get(&id)?;
    let kind = mods_folder_for_type(&srv.server_type);
    let dir = store.server_dir(&id).join(kind);
    let mut items = Vec::new();
    if dir.is_dir()
        && let Ok(entries) = std::fs::read_dir(&dir)
    {
        for entry in entries.flatten() {
            let fname = entry.file_name().to_string_lossy().to_string();
            if fname.ends_with(".jar") || fname.ends_with(".jar.disabled") {
                let enabled = !fname.ends_with(".disabled");
                let name = fname
                    .strip_suffix(".jar.disabled")
                    .or_else(|| fname.strip_suffix(".jar"))
                    .unwrap_or(&fname)
                    .to_string();
                items.push(serde_json::json!({
                    "name": name,
                    "file": fname,
                    "enabled": enabled,
                }));
            }
        }
    }
    items.sort_by(|a, b| {
        a.get("name")
            .and_then(|v| v.as_str())
            .cmp(&b.get("name").and_then(|v| v.as_str()))
    });
    Ok(Json(serde_json::json!({
        "type": kind,
        "items": items,
    })))
}

#[derive(Deserialize)]
pub struct DownloadModReq {
    pub url: String,
    pub filename: String,
    pub delete_old_name: Option<String>,
    pub project_id: Option<String>,
    pub project_slug: Option<String>,
    pub provider: Option<String>,
}

pub async fn handle_download_mod(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(req): Json<DownloadModReq>,
) -> Result<impl IntoResponse, ApiError> {
    if !user.has_permission("servers.files") && !user.has_permission("servers.manage") {
        return Err(ApiError::forbidden("missing permission"));
    }
    let srv = store.get(&id)?;
    let kind = mods_folder_for_type(&srv.server_type);
    let dir = store.server_dir(&id).join(kind);
    std::fs::create_dir_all(&dir)
        .map_err(|e| ApiError::internal(format!("create {kind} dir: {e}")))?;

    let filename = sanitize_jar_filename(&req.filename)?;
    let resp = store
        .http
        .get(&req.url)
        .header("User-Agent", "mcm/0.1.0")
        .send()
        .await
        .map_err(|e| ApiError::internal(format!("download mod failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(ApiError::internal(format!(
            "mod download returned HTTP {}",
            resp.status()
        )));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| ApiError::internal(format!("read mod bytes: {e}")))?;
    let dest = dir.join(&filename);
    tokio::fs::write(&dest, &bytes)
        .await
        .map_err(|e| ApiError::internal(format!("write mod file: {e}")))?;

    if let Some(old) = req.delete_old_name
        && let Ok(old_safe) = sanitize_jar_filename(&old)
        && old_safe != filename
    {
        let _ = std::fs::remove_file(dir.join(&old_safe));
    }

    let name = filename
        .strip_suffix(".jar")
        .unwrap_or(&filename)
        .to_string();
    Ok(Json(serde_json::json!({
        "name": name,
        "file": filename,
        "enabled": true,
        "project_id": req.project_id,
        "project_slug": req.project_slug,
        "provider": req.provider,
    })))
}

pub async fn handle_delete_mod(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path((id, name)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    if !user.has_permission("servers.files") && !user.has_permission("servers.manage") {
        return Err(ApiError::forbidden("missing permission"));
    }
    let srv = store.get(&id)?;
    let kind = mods_folder_for_type(&srv.server_type);
    let dir = store.server_dir(&id).join(kind);
    let safe = sanitize_jar_filename(&name)?;
    for candidate in [
        safe.clone(),
        format!("{safe}.jar"),
        format!("{safe}.jar.disabled"),
    ] {
        let p = dir.join(&candidate);
        if p.is_file() {
            let _ = std::fs::remove_file(p);
        }
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

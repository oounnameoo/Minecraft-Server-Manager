use crate::auth::User;
use crate::error::ApiError;
use crate::jars::list_versions;
use crate::ports::{create_reservation, delete_reservation, list_reservations};
use crate::servers::ServerStore;
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
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
    let (connected, version) = match &store.docker {
        Some(docker) => match docker.ping().await {
            Ok(v) => (true, v),
            Err(_) => (false, String::new()),
        },
        None => (false, String::new()),
    };
    Ok(Json(serde_json::json!({
        "runtime": store.cfg.container_runtime,
        "connected": connected,
        "version": version,
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
    Json(serde_json::json!({
        "versions": [8, 11, 17, 21, 22]
    }))
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
    Ok(Json(
        serde_json::json!({ "software": software, "versions": versions }),
    ))
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

use crate::auth::User;
use crate::error::ApiError;
use crate::servers::{ServerStore, check_server_perm};
use axum::{
    Extension, Json,
    extract::{Path, State},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpEntry {
    pub uuid: String,
    pub name: String,
    #[serde(default = "default_op_level")]
    pub level: i32,
    #[serde(default)]
    pub bounces_player_limit: bool,
}

fn default_op_level() -> i32 {
    4
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhitelistEntry {
    pub uuid: String,
    pub name: String,
}

pub async fn handle_list_ops(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "players.read")?;
    let _ = store.get(&id)?;
    let path = store.server_dir(&id).join("ops.json");
    let ops: Vec<OpEntry> = tokio::fs::read_to_string(&path)
        .await
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    Ok(Json(serde_json::json!({ "ops": ops })))
}

#[derive(Deserialize)]
pub struct PlayerNameReq {
    pub name: String,
}

pub async fn handle_add_op(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(req): Json<PlayerNameReq>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "ops.manage")?;
    let _ = store.get(&id)?;
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("player name is required"));
    }
    let _ = store.runtime.send_command(&id, &format!("op {name}")).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_remove_op(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path((id, name)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "ops.manage")?;
    let _ = store.get(&id)?;
    let _ = store
        .runtime
        .send_command(&id, &format!("deop {name}"))
        .await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_list_whitelist(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "players.read")?;
    let _ = store.get(&id)?;
    let path = store.server_dir(&id).join("whitelist.json");
    let entries: Vec<WhitelistEntry> = tokio::fs::read_to_string(&path)
        .await
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    Ok(Json(serde_json::json!({ "whitelist": entries })))
}

pub async fn handle_add_whitelist(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(req): Json<PlayerNameReq>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "whitelist.manage")?;
    let _ = store.get(&id)?;
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("player name is required"));
    }
    let _ = store
        .runtime
        .send_command(&id, &format!("whitelist add {name}"))
        .await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_remove_whitelist(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path((id, name)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "whitelist.manage")?;
    let _ = store.get(&id)?;
    let _ = store
        .runtime
        .send_command(&id, &format!("whitelist remove {name}"))
        .await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

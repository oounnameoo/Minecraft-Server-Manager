use crate::auth::User;
use crate::error::ApiError;
use crate::servers::{ServerStore, check_server_perm};
use axum::{
    Extension, Json,
    extract::{Path, State},
    response::IntoResponse,
};
use serde::Serialize;

#[derive(Serialize)]
pub struct WorldInfo {
    pub name: String,
    pub size_bytes: u64,
    pub has_level_dat: bool,
}

fn dir_size(path: &std::path::Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(path) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in rd.flatten() {
        if let Ok(meta) = entry.metadata() {
            if meta.is_dir() {
                total += dir_size(&entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    total
}

pub async fn handle_list_worlds(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "worlds.read")?;
    let _ = store.get(&id)?;
    let root = store.server_dir(&id);
    let mut worlds = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&root) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join("level.dat").exists() {
                worlds.push(WorldInfo {
                    name: entry.file_name().to_string_lossy().to_string(),
                    size_bytes: dir_size(&path),
                    has_level_dat: true,
                });
            }
        }
    }
    Ok(Json(serde_json::json!({ "worlds": worlds })))
}

pub async fn handle_delete_world(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path((id, name)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "worlds.manage")?;
    let _ = store.get(&id)?;
    if name.contains('/') || name.contains("..") {
        return Err(ApiError::bad_request("invalid world name"));
    }
    let path = store.server_dir(&id).join(&name);
    if !path.join("level.dat").exists() {
        return Err(ApiError::not_found("world not found"));
    }
    tokio::fs::remove_dir_all(&path)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

use crate::auth::User;
use crate::error::ApiError;
use crate::files::safe_resolve;
use crate::servers::{ServerStore, check_server_perm};
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::path::Path as StdPath;

#[derive(Debug, Serialize)]
pub struct DatapackEntry {
    pub name: String,
    pub file: String,
    pub size_bytes: u64,
    pub enabled: bool,
    pub is_dir: bool,
    pub modified_at: String,
}

#[derive(Debug, Deserialize)]
pub struct WorldQuery {
    #[serde(default)]
    pub world: String,
}

#[derive(Debug, Deserialize)]
pub struct DownloadDatapackReq {
    #[serde(default)]
    pub world: String,
    pub url: String,
    pub filename: String,
}

#[derive(Debug, Deserialize)]
pub struct ToggleDatapackReq {
    #[serde(default)]
    pub world: String,
    pub file: String,
}

fn read_active_world(root: &StdPath) -> String {
    if let Ok(props) = std::fs::read_to_string(root.join("server.properties")) {
        for line in props.lines() {
            if let Some(val) = line.strip_prefix("level-name=") {
                let trimmed = val.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_string();
                }
            }
        }
    }
    "world".to_string()
}

fn resolve_world_datapacks_dir(
    root: &StdPath,
    world: &str,
) -> Result<(String, std::path::PathBuf), ApiError> {
    let world_name = if world.trim().is_empty() {
        read_active_world(root)
    } else {
        world.trim().to_string()
    };
    let world_dir = safe_resolve(root, &world_name)?;
    Ok((world_name, world_dir.join("datapacks")))
}

fn sanitize_zip_filename(raw: &str) -> Result<String, ApiError> {
    let fname = StdPath::new(raw.trim())
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| ApiError::bad_request("invalid datapack filename"))?;
    if fname.ends_with(".zip") {
        Ok(fname.to_string())
    } else {
        Ok(format!("{fname}.zip"))
    }
}

fn entry_from_path(path: &StdPath) -> Option<DatapackEntry> {
    let fname = path.file_name()?.to_str()?.to_string();
    let meta = std::fs::metadata(path).ok()?;
    let is_dir = meta.is_dir();
    if is_dir {
        if !path.join("pack.mcmeta").is_file() {
            return None;
        }
    } else if !fname.ends_with(".zip") && !fname.ends_with(".zip.disabled") {
        return None;
    }
    let enabled = !fname.ends_with(".disabled");
    let name = fname
        .strip_suffix(".zip.disabled")
        .or_else(|| fname.strip_suffix(".zip"))
        .unwrap_or(&fname)
        .to_string();
    let modified_at = meta
        .modified()
        .ok()
        .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
        .unwrap_or_default();
    Some(DatapackEntry {
        name,
        file: fname,
        size_bytes: meta.len(),
        enabled,
        is_dir,
        modified_at,
    })
}

pub async fn handle_list_datapacks(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Query(q): Query<WorldQuery>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "worlds.read")
        .or_else(|_| check_server_perm(&user, &id, "files.read"))?;
    let _ = store.get(&id)?;
    let root = store.server_dir(&id);
    let (world, dir) = resolve_world_datapacks_dir(&root, &q.world)?;
    let mut datapacks = Vec::new();
    if dir.is_dir()
        && let Ok(entries) = std::fs::read_dir(&dir)
    {
        for entry in entries.flatten() {
            if let Some(dp) = entry_from_path(&entry.path()) {
                datapacks.push(dp);
            }
        }
    }
    datapacks.sort_by_key(|a| a.name.to_lowercase());
    Ok(Json(serde_json::json!({
        "world": world,
        "datapacks": datapacks,
    })))
}

pub async fn handle_download_datapack(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(req): Json<DownloadDatapackReq>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "worlds.manage")
        .or_else(|_| check_server_perm(&user, &id, "files.write"))?;
    let _ = store.get(&id)?;
    let url = req.url.trim();
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err(ApiError::bad_request(
            "url must start with https:// or http://",
        ));
    }
    let root = store.server_dir(&id);
    let (world, dir) = resolve_world_datapacks_dir(&root, &req.world)?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| ApiError::internal(format!("create datapacks dir: {e}")))?;

    let filename = sanitize_zip_filename(&req.filename)?;
    let resp = store
        .http
        .get(url)
        .header("User-Agent", "mcm/0.1.0")
        .send()
        .await
        .map_err(|e| ApiError::bad_request(format!("download datapack failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(ApiError::bad_request(format!(
            "datapack download returned HTTP {}",
            resp.status()
        )));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| ApiError::bad_request(format!("read datapack bytes: {e}")))?;
    let dest = dir.join(&filename);
    tokio::fs::write(&dest, &bytes)
        .await
        .map_err(|e| ApiError::internal(format!("write datapack file: {e}")))?;

    let datapack = entry_from_path(&dest)
        .ok_or_else(|| ApiError::internal("failed to read saved datapack metadata"))?;
    Ok(Json(serde_json::json!({
        "ok": true,
        "world": world,
        "datapack": datapack,
    })))
}

pub async fn handle_toggle_datapack(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(req): Json<ToggleDatapackReq>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "worlds.manage")
        .or_else(|_| check_server_perm(&user, &id, "files.write"))?;
    let _ = store.get(&id)?;
    let root = store.server_dir(&id);
    let (_world, dir) = resolve_world_datapacks_dir(&root, &req.world)?;
    let safe_name = StdPath::new(req.file.trim())
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| ApiError::bad_request("invalid datapack filename"))?
        .to_string();
    let src = dir.join(&safe_name);
    if !src.exists() {
        return Err(ApiError::not_found("datapack not found"));
    }
    let (new_name, enabled) = if let Some(base) = safe_name.strip_suffix(".disabled") {
        (base.to_string(), true)
    } else {
        (format!("{safe_name}.disabled"), false)
    };
    tokio::fs::rename(&src, dir.join(&new_name))
        .await
        .map_err(|e| ApiError::internal(format!("toggle datapack failed: {e}")))?;
    Ok(Json(serde_json::json!({
        "ok": true,
        "file": new_name,
        "enabled": enabled,
    })))
}

pub async fn handle_delete_datapack(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path((id, file)): Path<(String, String)>,
    Query(q): Query<WorldQuery>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "worlds.manage")
        .or_else(|_| check_server_perm(&user, &id, "files.delete"))?;
    let _ = store.get(&id)?;
    let root = store.server_dir(&id);
    let (_world, dir) = resolve_world_datapacks_dir(&root, &q.world)?;
    let safe_name = StdPath::new(file.trim())
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| ApiError::bad_request("invalid datapack filename"))?;
    let target = dir.join(safe_name);
    if !target.exists() {
        return Err(ApiError::not_found("datapack not found"));
    }
    if target.is_dir() {
        tokio::fs::remove_dir_all(&target)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    } else {
        tokio::fs::remove_file(&target)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

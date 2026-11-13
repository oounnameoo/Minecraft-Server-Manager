use crate::auth::User;
use crate::error::ApiError;
use crate::servers::{ServerStore, check_server_perm};
use axum::{
    Extension, Json,
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path as StdPath, PathBuf};

#[derive(Debug, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified_at: String,
}

#[derive(Debug, Deserialize)]
pub struct PathQuery {
    #[serde(default)]
    pub path: String,
}

pub fn safe_resolve(root: &StdPath, rel: &str) -> Result<PathBuf, ApiError> {
    let mut resolved = root.to_path_buf();
    let trimmed = rel.trim_start_matches('/');
    for comp in StdPath::new(trimmed).components() {
        match comp {
            Component::Normal(p) => resolved.push(p),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ApiError::bad_request("invalid file path"));
            }
        }
    }
    Ok(resolved)
}

pub async fn handle_list_files(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Query(q): Query<PathQuery>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "files.read")?;
    let _ = store.get(&id)?;
    let root = store.server_dir(&id);
    std::fs::create_dir_all(&root).ok();
    let target = safe_resolve(&root, &q.path)?;
    let mut entries = Vec::new();
    let rd = std::fs::read_dir(&target).map_err(|_| ApiError::not_found("directory not found"))?;
    for entry in rd.flatten() {
        let meta = entry.metadata().ok();
        let name = entry.file_name().to_string_lossy().to_string();
        let rel_path = if q.path.trim_matches('/').is_empty() {
            name.clone()
        } else {
            format!("{}/{}", q.path.trim_matches('/'), name)
        };
        let modified_at = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
            .unwrap_or_default();
        entries.push(FileEntry {
            name,
            path: rel_path,
            is_dir: meta.as_ref().map(|m| m.is_dir()).unwrap_or(false),
            size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
            modified_at,
        });
    }
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
    Ok(Json(
        serde_json::json!({ "entries": entries, "path": q.path }),
    ))
}

pub async fn handle_read_file(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Query(q): Query<PathQuery>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "files.read")?;
    let _ = store.get(&id)?;
    let target = safe_resolve(&store.server_dir(&id), &q.path)?;
    let content = tokio::fs::read_to_string(&target)
        .await
        .map_err(|_| ApiError::not_found("file not found or not UTF-8"))?;
    Ok(Json(
        serde_json::json!({ "path": q.path, "content": content }),
    ))
}

#[derive(Deserialize)]
pub struct WriteFileReq {
    pub path: String,
    pub content: String,
}

pub async fn handle_write_file(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(req): Json<WriteFileReq>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "files.write")?;
    let _ = store.get(&id)?;
    let target = safe_resolve(&store.server_dir(&id), &req.path)?;
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }
    tokio::fs::write(&target, req.content.as_bytes())
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct MkdirReq {
    pub path: String,
}

pub async fn handle_mkdir(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(req): Json<MkdirReq>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "files.write")?;
    let _ = store.get(&id)?;
    let target = safe_resolve(&store.server_dir(&id), &req.path)?;
    tokio::fs::create_dir_all(&target)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({ "ok": true }))))
}

#[derive(Deserialize)]
pub struct RenameReq {
    pub from: String,
    pub to: String,
}

pub async fn handle_rename_file(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(req): Json<RenameReq>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "files.write")?;
    let _ = store.get(&id)?;
    let root = store.server_dir(&id);
    let src = safe_resolve(&root, &req.from)?;
    let dst = safe_resolve(&root, &req.to)?;
    tokio::fs::rename(&src, &dst)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_delete_file(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Query(q): Query<PathQuery>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "files.delete")?;
    let _ = store.get(&id)?;
    let root = store.server_dir(&id);
    let target = safe_resolve(&root, &q.path)?;
    if target == root {
        return Err(ApiError::bad_request("cannot delete server root"));
    }
    let meta = tokio::fs::metadata(&target)
        .await
        .map_err(|_| ApiError::not_found("file not found"))?;
    if meta.is_dir() {
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

pub async fn handle_download_file(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Query(q): Query<PathQuery>,
) -> Result<Response, ApiError> {
    check_server_perm(&user, &id, "files.read")?;
    let _ = store.get(&id)?;
    let target = safe_resolve(&store.server_dir(&id), &q.path)?;
    let data = tokio::fs::read(&target)
        .await
        .map_err(|_| ApiError::not_found("file not found"))?;
    let fname = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("download");
    let resp = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{fname}\""),
        )
        .body(Body::from(data))
        .unwrap();
    Ok(resp)
}

pub async fn handle_upload_file(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Query(q): Query<PathQuery>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "files.write")?;
    let _ = store.get(&id)?;
    let dir = safe_resolve(&store.server_dir(&id), &q.path)?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?
    {
        let Some(fname) = field.file_name().map(String::from) else {
            continue;
        };
        let safe_name = StdPath::new(&fname)
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| ApiError::bad_request("invalid filename"))?;
        let dest = dir.join(safe_name);
        let bytes = field
            .bytes()
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        tokio::fs::write(&dest, &bytes)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }
    Ok((StatusCode::CREATED, Json(serde_json::json!({ "ok": true }))))
}

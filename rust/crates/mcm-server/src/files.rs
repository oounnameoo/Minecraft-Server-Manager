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

#[derive(Debug, Clone, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_directory: bool,
    pub size: u64,
    pub size_bytes: u64,
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
        let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        entries.push(FileEntry {
            name,
            path: rel_path,
            is_dir,
            is_directory: is_dir,
            size,
            size_bytes: size,
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

fn build_file_entry(root: &StdPath, target: &StdPath) -> Result<FileEntry, ApiError> {
    let meta = std::fs::metadata(target).map_err(|e| ApiError::internal(e.to_string()))?;
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    let rel = target
        .strip_prefix(root)
        .unwrap_or(target)
        .to_string_lossy()
        .replace('\\', "/");
    let modified_at = meta
        .modified()
        .ok()
        .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
        .unwrap_or_default();
    let is_dir = meta.is_dir();
    let size = meta.len();
    Ok(FileEntry {
        name,
        path: rel,
        is_dir,
        is_directory: is_dir,
        size,
        size_bytes: size,
        modified_at,
    })
}

fn copy_recursive(src: &StdPath, dst: &StdPath) -> std::io::Result<()> {
    let meta = std::fs::metadata(src)?;
    if meta.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &dst.join(entry.file_name()))?;
        }
    } else {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, dst)?;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct FromUrlReq {
    pub url: String,
    #[serde(default)]
    pub dir: String,
    #[serde(default)]
    pub name: String,
}

pub async fn handle_download_from_url(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(req): Json<FromUrlReq>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "files.write")?;
    let _ = store.get(&id)?;
    let url = req.url.trim();
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(ApiError::bad_request(
            "url must start with http:// or https://",
        ));
    }
    let root = store.server_dir(&id);
    let dir = safe_resolve(&root, &req.dir)?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| ApiError::bad_request(format!("download failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(ApiError::bad_request(format!(
            "remote server returned HTTP {}",
            resp.status()
        )));
    }

    let raw_name = if !req.name.trim().is_empty() {
        req.name.trim().to_string()
    } else {
        let from_url = url
            .split('?')
            .next()
            .unwrap_or(url)
            .rsplit('/')
            .next()
            .unwrap_or("download");
        if from_url.is_empty() {
            "download".to_string()
        } else {
            urlencoding::decode(from_url)
                .map(|c| c.into_owned())
                .unwrap_or_else(|_| from_url.to_string())
        }
    };
    let safe_name = StdPath::new(&raw_name)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| ApiError::bad_request("invalid filename"))?
        .to_string();

    let dest = dir.join(&safe_name);
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| ApiError::bad_request(format!("failed reading response body: {e}")))?;
    tokio::fs::write(&dest, &bytes)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let entry = build_file_entry(&root, &dest)?;
    Ok((StatusCode::CREATED, Json(entry)))
}

#[derive(Debug, Deserialize)]
pub struct CopyMoveReq {
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub sources: Option<Vec<String>>,
    pub destination: String,
}

fn collect_sources(req: &CopyMoveReq) -> Result<(Vec<String>, bool), ApiError> {
    if let Some(srcs) = &req.sources {
        if srcs.is_empty() {
            return Err(ApiError::bad_request("sources must not be empty"));
        }
        return Ok((srcs.clone(), false));
    }
    if let Some(src) = &req.source {
        return Ok((vec![src.clone()], true));
    }
    Err(ApiError::bad_request("source or sources is required"))
}

pub async fn handle_copy_files(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(req): Json<CopyMoveReq>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "files.write")?;
    let _ = store.get(&id)?;
    let root = store.server_dir(&id);
    let (sources, single) = collect_sources(&req)?;
    let dest_base = safe_resolve(&root, &req.destination)?;
    let dest_is_dir = dest_base.is_dir() || !single;
    if dest_is_dir {
        tokio::fs::create_dir_all(&dest_base)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }

    let mut entries = Vec::new();
    for s in sources {
        let src = safe_resolve(&root, &s)?;
        if !src.exists() {
            return Err(ApiError::not_found(format!("source not found: {s}")));
        }
        let target = if dest_is_dir {
            let fname = src
                .file_name()
                .ok_or_else(|| ApiError::bad_request("invalid source path"))?;
            dest_base.join(fname)
        } else {
            dest_base.clone()
        };
        if target.starts_with(&src) && src.is_dir() {
            return Err(ApiError::bad_request("cannot copy a directory into itself"));
        }
        let src_c = src.clone();
        let target_c = target.clone();
        tokio::task::spawn_blocking(move || copy_recursive(&src_c, &target_c))
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?
            .map_err(|e| ApiError::internal(e.to_string()))?;
        entries.push(build_file_entry(&root, &target)?);
    }

    if single {
        let entry = entries.first().cloned();
        Ok(Json(
            serde_json::json!({ "entry": entry, "entries": entries }),
        ))
    } else {
        let count = entries.len();
        Ok(Json(
            serde_json::json!({ "entries": entries, "count": count }),
        ))
    }
}

pub async fn handle_move_files(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(req): Json<CopyMoveReq>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "files.write")?;
    let _ = store.get(&id)?;
    let root = store.server_dir(&id);
    let (sources, single) = collect_sources(&req)?;
    let dest_base = safe_resolve(&root, &req.destination)?;
    let dest_is_dir = dest_base.is_dir() || !single;
    if dest_is_dir {
        tokio::fs::create_dir_all(&dest_base)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }

    let mut entries = Vec::new();
    for s in sources {
        let src = safe_resolve(&root, &s)?;
        if !src.exists() {
            return Err(ApiError::not_found(format!("source not found: {s}")));
        }
        let target = if dest_is_dir {
            let fname = src
                .file_name()
                .ok_or_else(|| ApiError::bad_request("invalid source path"))?;
            dest_base.join(fname)
        } else {
            dest_base.clone()
        };
        if target.starts_with(&src) && src.is_dir() {
            return Err(ApiError::bad_request("cannot move a directory into itself"));
        }
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ApiError::internal(e.to_string()))?;
        }
        tokio::fs::rename(&src, &target)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        entries.push(build_file_entry(&root, &target)?);
    }

    if single {
        let entry = entries.first().cloned();
        Ok(Json(
            serde_json::json!({ "entry": entry, "entries": entries }),
        ))
    } else {
        let count = entries.len();
        Ok(Json(
            serde_json::json!({ "entries": entries, "count": count }),
        ))
    }
}

#[derive(Debug, Deserialize)]
pub struct ArchiveReq {
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub sources: Option<Vec<String>>,
    #[serde(default)]
    pub dir: String,
    #[serde(default)]
    pub name: String,
}

pub async fn handle_archive_files(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(req): Json<ArchiveReq>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "files.write")?;
    let _ = store.get(&id)?;
    let root = store.server_dir(&id);
    let srcs: Vec<String> = if let Some(s) = req.sources.clone() {
        s
    } else if let Some(s) = req.source.clone() {
        vec![s]
    } else {
        return Err(ApiError::bad_request("source or sources is required"));
    };
    if srcs.is_empty() {
        return Err(ApiError::bad_request("sources must not be empty"));
    }

    let mut resolved_srcs = Vec::with_capacity(srcs.len());
    for s in &srcs {
        let p = safe_resolve(&root, s)?;
        if !p.exists() {
            return Err(ApiError::not_found(format!("source not found: {s}")));
        }
        resolved_srcs.push(p);
    }

    let out_dir = safe_resolve(&root, &req.dir)?;
    tokio::fs::create_dir_all(&out_dir)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let raw_name = if !req.name.trim().is_empty() {
        req.name.trim().to_string()
    } else if resolved_srcs.len() == 1 {
        let base = resolved_srcs[0]
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("archive");
        format!("{base}.zip")
    } else {
        "archive.zip".to_string()
    };
    let safe_name = StdPath::new(&raw_name)
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| ApiError::bad_request("invalid archive name"))?
        .to_string();
    let archive_path = out_dir.join(&safe_name);

    let archive_c = archive_path.clone();
    let root_c = root.clone();
    tokio::task::spawn_blocking(move || -> Result<(), ApiError> {
        let script = r#"
import os, sys, zipfile
out = sys.argv[1]
base = sys.argv[2]
items = sys.argv[3:]
with zipfile.ZipFile(out, 'w', zipfile.ZIP_DEFLATED) as zf:
    for item in items:
        if os.path.isdir(item):
            for dirpath, _, filenames in os.walk(item):
                for fn in filenames:
                    full = os.path.join(dirpath, fn)
                    zf.write(full, os.path.relpath(full, base))
        else:
            zf.write(item, os.path.relpath(item, base))
"#;
        let mut cmd = std::process::Command::new("python3");
        cmd.arg("-c").arg(script).arg(&archive_c).arg(&root_c);
        for p in &resolved_srcs {
            cmd.arg(p);
        }
        let status = cmd
            .status()
            .map_err(|e| ApiError::internal(format!("archive failed: {e}")))?;
        if !status.success() {
            return Err(ApiError::internal("archive creation failed"));
        }
        Ok(())
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))??;

    let entry = build_file_entry(&root, &archive_path)?;
    Ok((StatusCode::CREATED, Json(entry)))
}

#[derive(Debug, Deserialize)]
pub struct UnzipReq {
    pub archive: String,
    #[serde(default)]
    pub dest: String,
}

pub async fn handle_unzip_file(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(req): Json<UnzipReq>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "files.write")?;
    let _ = store.get(&id)?;
    let root = store.server_dir(&id);
    let archive_path = safe_resolve(&root, &req.archive)?;
    if !archive_path.is_file() {
        return Err(ApiError::not_found("archive not found"));
    }
    let dest_dir = safe_resolve(&root, &req.dest)?;
    tokio::fs::create_dir_all(&dest_dir)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let count = tokio::task::spawn_blocking(move || -> Result<usize, ApiError> {
        let script = r#"
import os, sys, zipfile
arch = sys.argv[1]
dest = os.path.realpath(sys.argv[2])
count = 0
with zipfile.ZipFile(arch, 'r') as zf:
    for info in zf.infolist():
        target = os.path.realpath(os.path.join(dest, info.filename))
        if not (target == dest or target.startswith(dest + os.sep)):
            raise ValueError('unsafe path in archive')
        if info.is_dir():
            os.makedirs(target, exist_ok=True)
        else:
            os.makedirs(os.path.dirname(target), exist_ok=True)
            with zf.open(info) as src, open(target, 'wb') as dst:
                dst.write(src.read())
            count += 1
print(count)
"#;
        let out = std::process::Command::new("python3")
            .arg("-c")
            .arg(script)
            .arg(&archive_path)
            .arg(&dest_dir)
            .output()
            .map_err(|e| ApiError::internal(format!("unzip failed: {e}")))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(ApiError::bad_request(format!("unzip failed: {err}")));
        }
        let n = String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse::<usize>()
            .unwrap_or(0);
        Ok(n)
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))??;

    Ok(Json(serde_json::json!({ "ok": true, "count": count })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_resolve_blocks_parent_traversal_and_resolves_relative_paths() {
        let root = StdPath::new("/tmp/mcm-server-root");
        assert_eq!(
            safe_resolve(root, "/plugins/config.yml").unwrap(),
            root.join("plugins").join("config.yml")
        );
        assert_eq!(safe_resolve(root, "./world").unwrap(), root.join("world"));
        assert!(safe_resolve(root, "../etc/passwd").is_err());
        assert!(safe_resolve(root, "plugins/../../secret").is_err());
    }

    #[test]
    fn copy_recursive_copies_nested_directories_and_files() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src_dir");
        let dst = tmp.path().join("dst_dir");
        std::fs::create_dir_all(src.join("nested")).unwrap();
        std::fs::write(src.join("a.txt"), "hello").unwrap();
        std::fs::write(src.join("nested").join("b.txt"), "world").unwrap();

        copy_recursive(&src, &dst).unwrap();

        assert_eq!(std::fs::read_to_string(dst.join("a.txt")).unwrap(), "hello");
        assert_eq!(
            std::fs::read_to_string(dst.join("nested").join("b.txt")).unwrap(),
            "world"
        );
    }
}

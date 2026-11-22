use crate::auth::User;
use crate::error::ApiError;
use crate::servers::{ServerStore, check_server_perm};
use axum::{
    Extension, Json,
    body::Body,
    extract::{Multipart, Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::PathBuf;
use tar::{Archive, Builder};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct Backup {
    pub id: String,
    pub server_id: String,
    pub name: String,
    pub size_bytes: i64,
    pub location: String,
    pub status: String,
    pub protected: bool,
    pub deleted_at: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateBackupReq {
    #[serde(default)]
    pub name: String,
}

fn backup_dir(store: &ServerStore, server_id: &str) -> PathBuf {
    PathBuf::from(&store.cfg.backup_dir).join(server_id)
}

fn resolve_backup_path(location: &str) -> PathBuf {
    PathBuf::from(location.strip_prefix("local:").unwrap_or(location))
}

fn format_local_location(location: String) -> String {
    if location.starts_with("local:") || location.starts_with("s3:") {
        location
    } else {
        format!("local:{location}")
    }
}

pub async fn handle_list_backups(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "backups.read")?;
    let _ = store.get(&id)?;
    let conn = store.db.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT id, COALESCE(server_id,''), COALESCE(name,''), COALESCE(size_bytes,0),
                COALESCE(location,''), COALESCE(status,'completed'), COALESCE(protected,0),
                COALESCE(deleted_at,''), COALESCE(created_at,'')
         FROM backups WHERE server_id = ?1 ORDER BY created_at DESC",
    )?;
    let rows = stmt.query_map(params![id], |r| {
        let raw_loc: String = r.get(4)?;
        Ok(Backup {
            id: r.get(0)?,
            server_id: r.get(1)?,
            name: r.get(2)?,
            size_bytes: r.get(3)?,
            location: format_local_location(raw_loc),
            status: r.get(5)?,
            protected: r.get::<_, i64>(6)? != 0,
            deleted_at: r.get(7)?,
            created_at: r.get(8)?,
        })
    })?;
    let mut backups = Vec::new();
    let mut trash = Vec::new();
    for r in rows {
        let b = r?;
        if b.deleted_at.is_empty() {
            backups.push(b);
        } else {
            trash.push(b);
        }
    }
    Ok(Json(serde_json::json!({
        "backups": backups,
        "trash": trash,
        "policy": {
            "keep_count": 10,
            "max_age_days": 30,
            "trash_retention_days": 7
        }
    })))
}

pub async fn handle_create_backup(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    body: Option<Json<CreateBackupReq>>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "backups.create")?;
    let _ = store.get(&id)?;
    let backup_id = Uuid::new_v4().to_string();
    let now = Utc::now();
    let label = body
        .map(|b| b.name.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("backup-{}", now.format("%Y%m%d-%H%M%S")));
    let filename = format!("{backup_id}.tar.gz");

    let src_dir = store.server_dir(&id);
    let dst_dir = backup_dir(&store, &id);
    std::fs::create_dir_all(&dst_dir).map_err(|e| ApiError::internal(e.to_string()))?;
    let dst_path = dst_dir.join(&filename);
    let location = dst_path.to_string_lossy().to_string();

    let archive_path = dst_path.clone();
    let size_bytes = tokio::task::spawn_blocking(move || -> anyhow::Result<i64> {
        if src_dir.exists() {
            let status = std::process::Command::new("tar")
                .env("GZIP", "-1")
                .arg("-czf")
                .arg(&archive_path)
                .arg("-C")
                .arg(&src_dir)
                .arg(".")
                .status();
            match status {
                Ok(s) if s.success() || s.code() == Some(1) => {}
                Ok(s) => anyhow::bail!("tar exited with status {s}"),
                Err(_) => {
                    let tar_gz = File::create(&archive_path)?;
                    let enc = GzEncoder::new(tar_gz, Compression::fast());
                    let mut tar = Builder::new(enc);
                    tar.append_dir_all(".", &src_dir)?;
                    tar.finish()?;
                }
            }
        } else {
            let tar_gz = File::create(&archive_path)?;
            let enc = GzEncoder::new(tar_gz, Compression::fast());
            let mut tar = Builder::new(enc);
            tar.finish()?;
        }
        let meta = std::fs::metadata(&archive_path)?;
        Ok(meta.len() as i64)
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?
    .map_err(|e| ApiError::internal(format!("create backup archive: {e}")))?;

    let created_at = now.to_rfc3339();
    {
        let conn = store.db.lock().unwrap();
        conn.execute(
            "INSERT INTO backups (id, server_id, name, size_bytes, location, status, created_at, protected, deleted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'completed', ?6, 0, '')",
            params![backup_id, id, label, size_bytes, location, created_at],
        )?;
    }

    Ok((
        StatusCode::CREATED,
        Json(Backup {
            id: backup_id,
            server_id: id,
            name: label,
            size_bytes,
            location,
            status: "completed".to_string(),
            protected: false,
            deleted_at: String::new(),
            created_at,
        }),
    ))
}

pub async fn handle_restore_backup(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path((id, backup_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "backups.restore")?;
    let _ = store.get(&id)?;
    let location: String = {
        let conn = store.db.lock().unwrap();
        conn.query_row(
            "SELECT location FROM backups WHERE id = ?1 AND server_id = ?2",
            params![backup_id, id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| ApiError::not_found("backup not found"))?
    };

    let archive_path = resolve_backup_path(&location);
    let dest_dir = store.server_dir(&id);

    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let tar_gz = File::open(&archive_path)?;
        let dec = GzDecoder::new(tar_gz);
        let mut archive = Archive::new(dec);
        std::fs::create_dir_all(&dest_dir)?;
        archive.unpack(&dest_dir)?;
        Ok(())
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?
    .map_err(|e| ApiError::internal(format!("restore backup: {e}")))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_delete_backup(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(backup_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    if !user.has_permission("backups.delete") {
        return Err(ApiError::forbidden("missing permission backups.delete"));
    }
    let protected: Option<bool> = {
        let conn = store.db.lock().unwrap();
        conn.query_row(
            "SELECT COALESCE(protected,0) FROM backups WHERE id = ?1",
            params![backup_id],
            |r| Ok(r.get::<_, i64>(0)? != 0),
        )
        .optional()?
    };
    let Some(protected) = protected else {
        return Err(ApiError::not_found("backup not found"));
    };
    if protected {
        return Err(ApiError::conflict("protected", "backup is protected"));
    }
    let deleted_at = Utc::now().to_rfc3339();
    let conn = store.db.lock().unwrap();
    conn.execute(
        "UPDATE backups SET deleted_at = ?1 WHERE id = ?2",
        params![deleted_at, backup_id],
    )?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct ProtectBackupReq {
    pub protected: bool,
}

pub async fn handle_protect_backup(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(backup_id): Path<String>,
    Json(req): Json<ProtectBackupReq>,
) -> Result<impl IntoResponse, ApiError> {
    if !user.has_permission("backups.delete") {
        return Err(ApiError::forbidden("missing permission backups.delete"));
    }
    let conn = store.db.lock().unwrap();
    let updated = conn.execute(
        "UPDATE backups SET protected = ?1 WHERE id = ?2",
        params![if req.protected { 1 } else { 0 }, backup_id],
    )?;
    if updated == 0 {
        return Err(ApiError::not_found("backup not found"));
    }
    Ok(Json(
        serde_json::json!({ "ok": true, "protected": req.protected }),
    ))
}

pub async fn handle_recover_backup(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(backup_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    if !user.has_permission("backups.delete") {
        return Err(ApiError::forbidden("missing permission backups.delete"));
    }
    let conn = store.db.lock().unwrap();
    let updated = conn.execute(
        "UPDATE backups SET deleted_at = '', protected = 1 WHERE id = ?1",
        params![backup_id],
    )?;
    if updated == 0 {
        return Err(ApiError::not_found("backup not found"));
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_purge_backup(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(backup_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    if !user.has_permission("backups.delete") {
        return Err(ApiError::forbidden("missing permission backups.delete"));
    }
    let row: Option<(String, bool)> = {
        let conn = store.db.lock().unwrap();
        conn.query_row(
            "SELECT COALESCE(location,''), COALESCE(protected,0) FROM backups WHERE id = ?1",
            params![backup_id],
            |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0)),
        )
        .optional()?
    };
    let Some((location, protected)) = row else {
        return Err(ApiError::not_found("backup not found"));
    };
    if protected {
        return Err(ApiError::conflict("protected", "backup is protected"));
    }
    if !location.is_empty() {
        let _ = tokio::fs::remove_file(resolve_backup_path(&location)).await;
    }
    let conn = store.db.lock().unwrap();
    conn.execute("DELETE FROM backups WHERE id = ?1", params![backup_id])?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_download_backup_by_id(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(backup_id): Path<String>,
) -> Result<Response, ApiError> {
    let row: Option<(String, String)> = {
        let conn = store.db.lock().unwrap();
        conn.query_row(
            "SELECT server_id, location FROM backups WHERE id = ?1",
            params![backup_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
    };
    let Some((server_id, location)) = row else {
        return Err(ApiError::not_found("backup not found"));
    };
    check_server_perm(&user, &server_id, "backups.download")?;
    serve_backup_file(&location).await
}

pub async fn handle_download_backup(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path((id, backup_id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    check_server_perm(&user, &id, "backups.download")?;
    let location: String = {
        let conn = store.db.lock().unwrap();
        conn.query_row(
            "SELECT location FROM backups WHERE id = ?1 AND server_id = ?2",
            params![backup_id, id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| ApiError::not_found("backup not found"))?
    };
    serve_backup_file(&location).await
}

async fn serve_backup_file(location: &str) -> Result<Response, ApiError> {
    let path = resolve_backup_path(location);
    let fname = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("backup.tar.gz")
        .to_string();
    let data = tokio::fs::read(&path)
        .await
        .map_err(|_| ApiError::not_found("backup file missing"))?;
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/gzip")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{fname}\""),
        )
        .body(Body::from(data))
        .unwrap())
}

pub async fn handle_upload_backup(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "backups.create")?;
    let _ = store.get(&id)?;
    let dst_dir = backup_dir(&store, &id);
    tokio::fs::create_dir_all(&dst_dir)
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
        let bytes = field
            .bytes()
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let backup_id = Uuid::new_v4().to_string();
        let dst_path = dst_dir.join(format!("{backup_id}.tar.gz"));
        tokio::fs::write(&dst_path, &bytes)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let size_bytes = bytes.len() as i64;
        let location = dst_path.to_string_lossy().to_string();
        let label = fname.strip_suffix(".tar.gz").unwrap_or(&fname).to_string();
        let created_at = Utc::now().to_rfc3339();
        {
            let conn = store.db.lock().unwrap();
            conn.execute(
                "INSERT INTO backups (id, server_id, name, size_bytes, location, status, created_at, protected, deleted_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'completed', ?6, 0, '')",
                params![backup_id, id, label, size_bytes, location, created_at],
            )?;
        }
        return Ok((
            StatusCode::CREATED,
            Json(Backup {
                id: backup_id,
                server_id: id,
                name: label,
                size_bytes,
                location: format_local_location(location),
                status: "completed".to_string(),
                protected: false,
                deleted_at: String::new(),
                created_at,
            }),
        ));
    }
    Err(ApiError::bad_request("missing backup file"))
}

pub async fn handle_empty_trash(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "backups.delete")?;
    let _ = store.get(&id)?;
    let rows: Vec<(String, String)> = {
        let conn = store.db.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, COALESCE(location,'') FROM backups WHERE server_id = ?1 AND COALESCE(deleted_at,'') != ''",
        )?;
        stmt.query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .flatten()
            .collect()
    };
    let purged = rows.len();
    for (bid, loc) in rows {
        if !loc.is_empty() {
            let _ = tokio::fs::remove_file(resolve_backup_path(&loc)).await;
        }
        let conn = store.db.lock().unwrap();
        let _ = conn.execute("DELETE FROM backups WHERE id = ?1", params![bid]);
    }
    Ok(Json(serde_json::json!({ "ok": true, "purged": purged })))
}

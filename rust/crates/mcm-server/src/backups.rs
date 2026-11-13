use crate::auth::User;
use crate::error::ApiError;
use crate::servers::{ServerStore, check_server_perm};
use axum::{
    Extension, Json,
    body::Body,
    extract::{Path, State},
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
        Ok(Backup {
            id: r.get(0)?,
            server_id: r.get(1)?,
            name: r.get(2)?,
            size_bytes: r.get(3)?,
            location: r.get(4)?,
            status: r.get(5)?,
            protected: r.get::<_, i64>(6)? != 0,
            deleted_at: r.get(7)?,
            created_at: r.get(8)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(Json(serde_json::json!({ "backups": out })))
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
        let tar_gz = File::create(&archive_path)?;
        let enc = GzEncoder::new(tar_gz, Compression::default());
        let mut tar = Builder::new(enc);
        if src_dir.exists() {
            tar.append_dir_all(".", &src_dir)?;
        }
        tar.finish()?;
        drop(tar);
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

    let archive_path = PathBuf::from(&location);
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
        let _ = tokio::fs::remove_file(&location).await;
    }
    let conn = store.db.lock().unwrap();
    conn.execute("DELETE FROM backups WHERE id = ?1", params![backup_id])?;
    Ok(Json(serde_json::json!({ "ok": true })))
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
    let path = PathBuf::from(&location);
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

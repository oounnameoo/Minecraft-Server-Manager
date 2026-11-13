use crate::auth::User;
use crate::error::ApiError;
use crate::servers::{ServerStore, check_server_perm};
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct ScheduledTask {
    pub id: String,
    pub server_id: String,
    pub name: String,
    pub action: String,
    pub command: String,
    pub cron: String,
    pub timezone: String,
    pub enabled: bool,
    pub next_run_at: String,
    pub last_run_at: String,
    pub last_status: String,
    pub last_error: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateScheduleReq {
    pub name: String,
    pub action: String,
    #[serde(default, alias = "payload")]
    pub command: String,
    #[serde(alias = "cron_expr")]
    pub cron: String,
    #[serde(default = "default_tz")]
    pub timezone: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_tz() -> String {
    "UTC".to_string()
}

fn default_true() -> bool {
    true
}

pub async fn handle_list_schedules(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "schedules.read")?;
    let _ = store.get(&id)?;
    let conn = store.db.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT id, server_id, name, action, command, cron, timezone, enabled,
                next_run_at, last_run_at, last_status, last_error, created_at, updated_at
         FROM scheduled_tasks WHERE server_id = ?1 ORDER BY created_at",
    )?;
    let rows = stmt.query_map(params![id], |r| {
        Ok(ScheduledTask {
            id: r.get(0)?,
            server_id: r.get(1)?,
            name: r.get(2)?,
            action: r.get(3)?,
            command: r.get(4)?,
            cron: r.get(5)?,
            timezone: r.get(6)?,
            enabled: r.get::<_, i64>(7)? != 0,
            next_run_at: r.get(8)?,
            last_run_at: r.get(9)?,
            last_status: r.get(10)?,
            last_error: r.get(11)?,
            created_at: r.get(12)?,
            updated_at: r.get(13)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(Json(serde_json::json!({ "schedules": out })))
}

pub async fn handle_create_schedule(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(req): Json<CreateScheduleReq>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "schedules.manage")?;
    let _ = store.get(&id)?;
    let task_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    {
        let conn = store.db.lock().unwrap();
        conn.execute(
            "INSERT INTO scheduled_tasks (id, server_id, name, action, command, cron, timezone, enabled, next_run_at, last_run_at, last_status, last_error, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, '', '', '', '', ?9, ?10)",
            params![
                task_id,
                id,
                req.name,
                req.action,
                req.command,
                req.cron,
                req.timezone,
                if req.enabled { 1 } else { 0 },
                now,
                now,
            ],
        )?;
    }
    Ok((
        StatusCode::CREATED,
        Json(ScheduledTask {
            id: task_id,
            server_id: id,
            name: req.name,
            action: req.action,
            command: req.command,
            cron: req.cron,
            timezone: req.timezone,
            enabled: req.enabled,
            next_run_at: String::new(),
            last_run_at: String::new(),
            last_status: String::new(),
            last_error: String::new(),
            created_at: now.clone(),
            updated_at: now,
        }),
    ))
}

pub async fn handle_delete_schedule(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path((id, task_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "schedules.manage")?;
    let conn = store.db.lock().unwrap();
    conn.execute(
        "DELETE FROM scheduled_tasks WHERE id = ?1 AND server_id = ?2",
        params![task_id, id],
    )?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

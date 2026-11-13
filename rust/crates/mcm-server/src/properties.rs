use crate::auth::User;
use crate::error::ApiError;
use crate::servers::{ServerStore, check_server_perm};
use axum::{
    Extension, Json,
    extract::{Path, State},
    response::IntoResponse,
};
use serde::Deserialize;
use std::collections::BTreeMap;

pub async fn handle_get_properties(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "servers.view")?;
    let _ = store.get(&id)?;
    let path = store.server_dir(&id).join("server.properties");
    let mut props = BTreeMap::new();
    if let Ok(content) = tokio::fs::read_to_string(&path).await {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = trimmed.split_once('=') {
                props.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    Ok(Json(serde_json::json!({ "properties": props })))
}

#[derive(Deserialize)]
pub struct UpdatePropertiesReq {
    pub properties: BTreeMap<String, String>,
}

pub async fn handle_put_properties(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(req): Json<UpdatePropertiesReq>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "servers.edit")?;
    let _ = store.get(&id)?;
    let dir = store.server_dir(&id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let path = dir.join("server.properties");

    let mut out = String::from("# Minecraft server properties\n");
    for (k, v) in &req.properties {
        let clean_k = k.replace(['\n', '\r', '='], "");
        let clean_v = v.replace(['\n', '\r'], "");
        out.push_str(&format!("{clean_k}={clean_v}\n"));
    }
    tokio::fs::write(&path, out.as_bytes())
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({ "properties": req.properties })))
}

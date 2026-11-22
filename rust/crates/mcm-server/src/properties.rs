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
    let srv = store.get(&id)?;
    let path = store.server_dir(&id).join("server.properties");
    let exists = path.is_file();
    let content = tokio::fs::read_to_string(&path).await.unwrap_or_default();
    let mut props = BTreeMap::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            props.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    Ok(Json(serde_json::json!({
        "content": content,
        "exists": exists,
        "file_name": "server.properties",
        "server_type": srv.server_type,
        "properties": props,
    })))
}

#[derive(Deserialize)]
pub struct UpdatePropertiesReq {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub properties: Option<BTreeMap<String, String>>,
}

pub async fn handle_put_properties(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(req): Json<UpdatePropertiesReq>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "servers.edit")?;
    let srv = store.get(&id)?;
    let dir = store.server_dir(&id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let path = dir.join("server.properties");

    let out = if let Some(content) = req.content {
        content
    } else if let Some(properties) = req.properties {
        let mut buf = String::from("# Minecraft server properties\n");
        for (k, v) in &properties {
            let clean_k = k.replace(['\n', '\r', '='], "");
            let clean_v = v.replace(['\n', '\r'], "");
            buf.push_str(&format!("{clean_k}={clean_v}\n"));
        }
        buf
    } else {
        return Err(ApiError::bad_request("content or properties is required"));
    };

    tokio::fs::write(&path, out.as_bytes())
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({
        "content": out,
        "exists": true,
        "file_name": "server.properties",
        "server_type": srv.server_type,
    })))
}

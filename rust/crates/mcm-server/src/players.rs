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

#[derive(Deserialize)]
struct UserCacheEntry {
    name: String,
    uuid: String,
}

async fn read_whitelist_entries(root: &std::path::Path) -> Vec<WhitelistEntry> {
    let path = root.join("whitelist.json");
    tokio::fs::read_to_string(&path)
        .await
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

async fn write_whitelist_entries(
    root: &std::path::Path,
    entries: &[WhitelistEntry],
) -> Result<(), ApiError> {
    let path = root.join("whitelist.json");
    let json = serde_json::to_string_pretty(entries)
        .map_err(|e| ApiError::internal(format!("serialize whitelist failed: {e}")))?;
    tokio::fs::write(&path, json)
        .await
        .map_err(|e| ApiError::internal(format!("write whitelist failed: {e}")))
}

async fn lookup_usercache_uuid(root: &std::path::Path, name: &str) -> Option<(String, String)> {
    let raw = tokio::fs::read_to_string(root.join("usercache.json"))
        .await
        .ok()?;
    let entries: Vec<UserCacheEntry> = serde_json::from_str(&raw).ok()?;
    entries
        .into_iter()
        .find(|e| e.name.eq_ignore_ascii_case(name))
        .map(|e| (e.uuid, e.name))
}

pub async fn handle_list_whitelist(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "players.read")?;
    let _ = store.get(&id)?;
    let entries = read_whitelist_entries(&store.server_dir(&id)).await;
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
    let root = store.server_dir(&id);
    let running = store.runtime.is_running(&id).await;
    if running {
        let _ = store
            .runtime
            .send_command(&id, &format!("whitelist add {name}"))
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
    let mut entries = read_whitelist_entries(&root).await;
    if !entries.iter().any(|e| e.name.eq_ignore_ascii_case(name)) {
        if let Some((uuid, exact_name)) = lookup_usercache_uuid(&root, name).await {
            entries.push(WhitelistEntry {
                uuid,
                name: exact_name,
            });
            write_whitelist_entries(&root, &entries).await?;
        } else if !running {
            return Err(ApiError::bad_request(
                "player UUID not found in usercache.json; start the server to resolve and whitelist this player",
            ));
        }
    }
    Ok(Json(
        serde_json::json!({ "ok": true, "whitelist": entries }),
    ))
}

pub async fn handle_remove_whitelist(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path((id, name)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "whitelist.manage")?;
    let _ = store.get(&id)?;
    let root = store.server_dir(&id);
    let running = store.runtime.is_running(&id).await;
    if running {
        let _ = store
            .runtime
            .send_command(&id, &format!("whitelist remove {name}"))
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
    let mut entries = read_whitelist_entries(&root).await;
    let before = entries.len();
    entries.retain(|e| !e.name.eq_ignore_ascii_case(name.trim()));
    if entries.len() != before && !running {
        write_whitelist_entries(&root, &entries).await?;
    }
    Ok(Json(
        serde_json::json!({ "ok": true, "whitelist": entries }),
    ))
}

#[derive(Debug, Default, Deserialize)]
pub struct PlayerCommandArgs {
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub item: Option<String>,
    #[serde(default)]
    pub amount: Option<u32>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub nbt: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PlayerCommandReq {
    pub action: String,
    #[serde(default)]
    pub args: PlayerCommandArgs,
}

pub async fn handle_list_players(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "players.read")?;
    let _ = store.get(&id)?;
    if !store.runtime.is_running(&id).await {
        return Ok(Json(serde_json::json!({
            "players": [],
            "source": "console",
        })));
    }
    let lines = store
        .runtime
        .logs_tail(&id, &store.server_dir(&id), 300)
        .await;
    let mut online = std::collections::BTreeSet::new();
    for line in &lines {
        if line.contains("Starting minecraft server version") {
            online.clear();
            continue;
        }
        if let Some((_, rest)) = line.split_once("players online:") {
            online.clear();
            for name in rest.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                online.insert(name.to_string());
            }
            continue;
        }
        if let Some(prefix) = line.strip_suffix(" joined the game")
            && let Some(name) = prefix.split_whitespace().last()
        {
            online.insert(name.to_string());
        } else if let Some(prefix) = line.strip_suffix(" left the game")
            && let Some(name) = prefix.split_whitespace().last()
        {
            online.remove(name);
        }
    }
    let players: Vec<serde_json::Value> = online
        .into_iter()
        .map(|name| serde_json::json!({ "name": name }))
        .collect();
    Ok(Json(serde_json::json!({
        "players": players,
        "source": "console",
    })))
}

pub async fn handle_player_command(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path((id, name)): Path<(String, String)>,
    Json(req): Json<PlayerCommandReq>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "players.manage")?;
    let _ = store.get(&id)?;
    let player = name.trim();
    if player.is_empty() {
        return Err(ApiError::bad_request("player name is required"));
    }
    let args = req.args;
    let cmd = match req.action.trim() {
        "kick" => match args
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(r) => format!("kick {player} {r}"),
            None => format!("kick {player}"),
        },
        "ban" => match args
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(r) => format!("ban {player} {r}"),
            None => format!("ban {player}"),
        },
        "pardon" => format!("pardon {player}"),
        "op" => format!("op {player}"),
        "deop" => format!("deop {player}"),
        "kill" => format!("kill {player}"),
        "gamemode" => {
            let mode = args.mode.as_deref().unwrap_or("survival").trim();
            format!("gamemode {mode} {player}")
        }
        "tp" => {
            let target = args.target.as_deref().unwrap_or("").trim();
            if target.is_empty() {
                return Err(ApiError::bad_request("tp target is required"));
            }
            format!("tp {player} {target}")
        }
        "give" => {
            let item = args.item.as_deref().unwrap_or("").trim();
            if item.is_empty() {
                return Err(ApiError::bad_request("item is required"));
            }
            let nbt = args.nbt.as_deref().unwrap_or("").trim();
            let amount = args.amount.unwrap_or(1);
            format!("give {player} {item}{nbt} {amount}")
        }
        "custom" => {
            let raw = args.command.as_deref().unwrap_or("").trim();
            if raw.is_empty() {
                return Err(ApiError::bad_request("command is required"));
            }
            raw.to_string()
        }
        other => {
            return Err(ApiError::bad_request(format!(
                "unsupported action: {other}"
            )));
        }
    };
    store
        .runtime
        .send_command(&id, &cmd)
        .await
        .map_err(|e| ApiError::internal(format!("send command: {e}")))?;
    Ok(Json(serde_json::json!({
        "ok": true,
        "response": format!("Sent: {cmd}"),
    })))
}

pub async fn handle_list_player_data(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "players.read")?;
    let _ = store.get(&id)?;
    let root = store.server_dir(&id);

    let mut name_by_uuid: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    if let Ok(raw) = tokio::fs::read_to_string(root.join("usercache.json")).await
        && let Ok(entries) = serde_json::from_str::<Vec<serde_json::Value>>(&raw)
    {
        for entry in entries {
            if let (Some(uuid), Some(name)) = (
                entry.get("uuid").and_then(|v| v.as_str()),
                entry.get("name").and_then(|v| v.as_str()),
            ) {
                name_by_uuid.insert(uuid.to_lowercase(), name.to_string());
            }
        }
    }

    let mut players = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&root) {
        for world_entry in rd.flatten() {
            if !world_entry.path().is_dir() {
                continue;
            }
            let world_name = world_entry.file_name().to_string_lossy().to_string();
            for (rel_sub, pd_dir) in [
                ("players/data", world_entry.path().join("players/data")),
                ("playerdata", world_entry.path().join("playerdata")),
            ] {
                if !pd_dir.is_dir() {
                    continue;
                }
                if let Ok(pd_rd) = std::fs::read_dir(&pd_dir) {
                    for file in pd_rd.flatten() {
                        let fname = file.file_name().to_string_lossy().to_string();
                        let Some(uuid) = fname.strip_suffix(".dat") else {
                            continue;
                        };
                        let modified_at = file
                            .metadata()
                            .ok()
                            .and_then(|m| m.modified().ok())
                            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
                            .unwrap_or_default();
                        let name = name_by_uuid
                            .get(&uuid.to_lowercase())
                            .cloned()
                            .unwrap_or_else(|| uuid.to_string());
                        players.push(serde_json::json!({
                            "uuid": uuid,
                            "name": name,
                            "world": world_name,
                            "path": format!("{world_name}/{rel_sub}/{fname}"),
                            "modified_at": modified_at,
                        }));
                    }
                }
            }
        }
    }

    Ok(Json(serde_json::json!({ "players": players })))
}

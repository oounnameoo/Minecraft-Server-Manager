use crate::auth::User;
use crate::error::ApiError;
use crate::servers::{Server, ServerStore, check_server_perm};
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use tokio::sync::RwLock;

const SAMPLE_INTERVAL_SECS: i64 = 15;
const COMMAND_POLL_TICKS: u64 = 2; // every 30s
const MAX_SAMPLES_PER_SERVER: usize = 20_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceSample {
    pub timestamp: String,
    pub online: bool,
    pub cpu_percent: Option<f64>,
    pub memory_bytes: Option<u64>,
    pub players: Option<usize>,
    pub tps: Option<f64>,
    pub tick_ms: Option<f64>,
}

#[derive(Default)]
struct PerfState {
    settings: HashMap<String, String>,
    samples: HashMap<String, Vec<PerformanceSample>>,
}

static PERF: OnceLock<RwLock<PerfState>> = OnceLock::new();

fn perf_state() -> &'static RwLock<PerfState> {
    PERF.get_or_init(|| RwLock::new(PerfState::default()))
}

fn settings_path(store: &ServerStore, server_id: &str) -> std::path::PathBuf {
    store
        .cfg
        .data_dir
        .join("performance")
        .join(format!("{server_id}.settings.json"))
}

async fn get_collection_mode(store: &ServerStore, server_id: &str) -> String {
    {
        let guard = perf_state().read().await;
        if let Some(m) = guard.settings.get(server_id) {
            return m.clone();
        }
    }
    let path = settings_path(store, server_id);
    let mode = tokio::fs::read_to_string(&path)
        .await
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| {
            v.get("collection_mode")
                .and_then(|m| m.as_str())
                .map(String::from)
        })
        .filter(|m| matches!(m.as_str(), "off" | "rcon" | "console"))
        .unwrap_or_else(|| "console".to_string());

    let mut guard = perf_state().write().await;
    guard
        .settings
        .entry(server_id.to_string())
        .or_insert_with(|| mode.clone());
    mode
}

async fn set_collection_mode(
    store: &ServerStore,
    server_id: &str,
    mode: String,
) -> Result<(), ApiError> {
    let path = settings_path(store, server_id);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }
    let body = serde_json::json!({ "collection_mode": mode });
    tokio::fs::write(&path, serde_json::to_vec(&body).unwrap_or_default())
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let mut guard = perf_state().write().await;
    guard.settings.insert(server_id.to_string(), mode);
    Ok(())
}

fn has_spark_jar(dir: &std::path::Path) -> bool {
    std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| {
            let n = e.file_name().to_string_lossy().to_lowercase();
            n.contains("spark") && n.ends_with(".jar")
        })
}

fn is_paper_with_bundled_spark(srv: &Server) -> bool {
    if !matches!(srv.server_type.as_str(), "paper" | "purpur" | "folia") {
        return false;
    }
    let parts: Vec<u32> = srv
        .version
        .split('.')
        .filter_map(|p| p.parse::<u32>().ok())
        .collect();
    match parts.as_slice() {
        [major, ..] if *major >= 2 => true,
        [1, minor, patch, ..] => *minor > 21 || (*minor == 21 && *patch >= 1),
        [1, minor] => *minor > 21,
        _ => false,
    }
}

fn detect_spark(srv: &Server, server_dir: &std::path::Path) -> (bool, &'static str) {
    if has_spark_jar(&server_dir.join("plugins")) {
        return (true, "plugin");
    }
    if has_spark_jar(&server_dir.join("mods")) {
        return (true, "mod");
    }
    if is_paper_with_bundled_spark(srv) {
        return (true, "bundled");
    }
    (false, "unavailable")
}

fn parse_first_number(s: &str) -> Option<f64> {
    let cleaned: String = s
        .trim()
        .trim_start_matches('*')
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    cleaned.parse::<f64>().ok()
}

fn parse_metrics_from_logs(lines: &[String]) -> (usize, Option<f64>, Option<f64>) {
    let mut online: HashSet<String> = HashSet::new();
    let mut tps: Option<f64> = None;
    let mut tick_ms: Option<f64> = None;
    let mut expect_spark_tps = false;
    let mut expect_spark_tick = false;

    for line in lines {
        let body = line.split("]: ").nth(1).unwrap_or(line.as_str());
        let trimmed = body
            .trim()
            .trim_start_matches("[?]")
            .trim_start_matches("[⚡]")
            .trim();

        if let Some((name, _)) = trimmed.split_once(" joined the game") {
            let n = name.trim();
            if !n.is_empty() && !n.contains(' ') {
                online.insert(n.to_string());
            }
        } else if let Some((name, _)) = trimmed.split_once(" left the game") {
            online.remove(name.trim());
        }

        // Paper / Spigot / Spark inline TPS: "TPS from last 5s, 10s, 1m, 5m, 15m: 20.0, ..."
        if let Some(after) = trimmed.split("TPS from last").nth(1) {
            if let Some((_, vals)) = after.split_once(':') {
                if let Some(v) = vals.split(',').next().and_then(parse_first_number) {
                    tps = Some(v.clamp(0.0, 20.0));
                } else {
                    expect_spark_tps = true;
                }
            } else {
                expect_spark_tps = true;
            }
            continue;
        }

        if expect_spark_tps {
            expect_spark_tps = false;
            if let Some(v) = trimmed.split(',').next().and_then(parse_first_number) {
                tps = Some(v.clamp(0.0, 20.0));
            }
        }

        // Spark tick durations: "Tick durations (min/med/95%ile/max ms) from last 10s, 1m:"
        // followed by "0.5/1.2/2.3/4.5, ..." (median is index 1)
        if trimmed.contains("Tick durations") {
            if let Some((_, vals)) = trimmed.split_once(':') {
                if let Some(med) = vals
                    .split(',')
                    .next()
                    .and_then(|g| g.split('/').nth(1))
                    .and_then(parse_first_number)
                {
                    tick_ms = Some(med);
                } else {
                    expect_spark_tick = true;
                }
            } else {
                expect_spark_tick = true;
            }
            continue;
        }

        if expect_spark_tick {
            expect_spark_tick = false;
            if let Some(med) = trimmed
                .split(',')
                .next()
                .and_then(|g| g.split('/').nth(1))
                .and_then(parse_first_number)
            {
                tick_ms = Some(med);
            }
        }
    }

    if tick_ms.is_none()
        && let Some(t) = tps
        && t > 0.01
    {
        tick_ms = Some((1000.0 / t).min(50.0));
    }

    (online.len(), tps, tick_ms)
}

pub async fn collect_sample_for_server(store: &ServerStore, server_id: &str, poll_commands: bool) {
    let mode = get_collection_mode(store, server_id).await;
    let Ok(srv) = store.get(server_id) else {
        return;
    };
    let running = store.runtime.is_running(server_id).await;
    let dir = store.server_dir(server_id);

    if running && poll_commands && mode == "console" {
        let (spark_ok, _) = detect_spark(&srv, &dir);
        let cmd = if spark_ok { "spark tps" } else { "tps" };
        if store
            .runtime
            .send_silent_perf_command(server_id, cmd)
            .await
            .is_ok()
        {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }

    let now = Utc::now().to_rfc3339();
    let sample = if running {
        let st = store.runtime.stats(server_id, srv.ram_mb).await;
        let (players, tps, tick_ms) = if mode != "off" {
            let lines = store.runtime.perf_logs_tail(server_id, &dir, 400).await;
            let (p, t, m) = parse_metrics_from_logs(&lines);
            (Some(p), t, m)
        } else {
            (None, None, None)
        };
        PerformanceSample {
            timestamp: now,
            online: true,
            cpu_percent: Some(st.cpu_percent),
            memory_bytes: Some((st.memory_used_mb.max(0.0) * 1024.0 * 1024.0) as u64),
            players,
            tps,
            tick_ms,
        }
    } else {
        PerformanceSample {
            timestamp: now,
            online: false,
            cpu_percent: None,
            memory_bytes: None,
            players: None,
            tps: None,
            tick_ms: None,
        }
    };

    let mut guard = perf_state().write().await;
    let vec = guard.samples.entry(server_id.to_string()).or_default();
    vec.push(sample);
    if vec.len() > MAX_SAMPLES_PER_SERVER {
        let excess = vec.len() - MAX_SAMPLES_PER_SERVER;
        vec.drain(0..excess);
    }
}

pub fn spawn_sampler(store: ServerStore) {
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(std::time::Duration::from_secs(SAMPLE_INTERVAL_SECS as u64));
        let mut tick: u64 = 0;
        loop {
            interval.tick().await;
            let poll_commands = tick.is_multiple_of(COMMAND_POLL_TICKS);
            tick = tick.wrapping_add(1);
            if let Ok(servers) = store.list() {
                for srv in servers {
                    collect_sample_for_server(&store, &srv.id, poll_commands).await;
                }
            }
        }
    });
}

#[derive(Debug, Deserialize)]
pub struct PerfQuery {
    #[serde(default)]
    pub range: String,
}

fn range_duration(range: &str) -> Duration {
    match range {
        "6h" => Duration::hours(6),
        "24h" => Duration::hours(24),
        "7d" => Duration::days(7),
        _ => Duration::hours(1),
    }
}

pub async fn handle_get_performance(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Query(q): Query<PerfQuery>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "servers.view")?;
    let srv = store.get(&id)?;
    let mode = get_collection_mode(&store, &id).await;

    let need_immediate = {
        let guard = perf_state().read().await;
        match guard.samples.get(&id).and_then(|v| v.last()) {
            Some(last) => DateTime::parse_from_rfc3339(&last.timestamp)
                .map(|dt| {
                    Utc::now().signed_duration_since(dt.with_timezone(&Utc)) > Duration::seconds(10)
                })
                .unwrap_or(true),
            None => true,
        }
    };
    if need_immediate {
        collect_sample_for_server(&store, &id, true).await;
    }

    let now = Utc::now();
    let start = now - range_duration(&q.range);
    let samples: Vec<PerformanceSample> = {
        let guard = perf_state().read().await;
        guard
            .samples
            .get(&id)
            .map(|v| {
                v.iter()
                    .filter(|s| {
                        DateTime::parse_from_rfc3339(&s.timestamp)
                            .map(|dt| dt.with_timezone(&Utc) >= start)
                            .unwrap_or(false)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    };

    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let (spark_avail, spark_src) = detect_spark(&srv, &store.server_dir(&id));

    Ok(Json(serde_json::json!({
        "start": start.to_rfc3339(),
        "end": now.to_rfc3339(),
        "resolution_seconds": SAMPLE_INTERVAL_SECS,
        "interval_seconds": SAMPLE_INTERVAL_SECS,
        "retention_days": 7,
        "cpu_cores": cores,
        "settings": { "collection_mode": mode },
        "spark": { "available": spark_avail, "source": spark_src },
        "report": null,
        "samples": samples,
    })))
}

pub async fn handle_get_performance_settings(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "servers.view")?;
    let _ = store.get(&id)?;
    let mode = get_collection_mode(&store, &id).await;
    Ok(Json(serde_json::json!({ "collection_mode": mode })))
}

#[derive(Debug, Deserialize)]
pub struct UpdatePerfSettingsReq {
    pub collection_mode: String,
}

pub async fn handle_put_performance_settings(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
    Json(req): Json<UpdatePerfSettingsReq>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "servers.edit")?;
    let _ = store.get(&id)?;
    let mode = req.collection_mode.trim().to_string();
    if !matches!(mode.as_str(), "off" | "rcon" | "console") {
        return Err(ApiError::bad_request(
            "collection_mode must be off, rcon, or console",
        ));
    }
    set_collection_mode(&store, &id, mode.clone()).await?;
    Ok(Json(serde_json::json!({ "collection_mode": mode })))
}

pub async fn handle_capture_spark(
    State(store): State<ServerStore>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    check_server_perm(&user, &id, "servers.edit")?;
    let srv = store.get(&id)?;
    let (spark_avail, _) = detect_spark(&srv, &store.server_dir(&id));
    if !spark_avail {
        return Err(ApiError::bad_request(
            "spark is not available on this server",
        ));
    }
    if !store.runtime.is_running(&id).await {
        return Err(ApiError::bad_request("server is not running"));
    }
    store
        .runtime
        .send_command(&id, "spark profiler --timeout 60")
        .await
        .map_err(|e| ApiError::internal(format!("failed to start spark profiler: {e}")))?;
    Ok(Json(serde_json::json!({
        "id": uuid::Uuid::new_v4().to_string(),
        "status": "capturing",
        "started_at": Utc::now().to_rfc3339(),
        "existing": false,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_metrics_from_logs_handles_spark_multiline_and_players() {
        let lines: Vec<String> = vec![
            "[16:45:01 INFO]: Steve joined the game",
            "[16:45:02 INFO]: Alex joined the game",
            "[16:45:05 INFO]: Steve left the game",
            "[16:45:12 INFO]: [?] TPS from last 5s, 10s, 1m, 5m, 15m:",
            "[16:45:12 INFO]: [?]  19.9, 20.0, *20.0, *20.0, *20.0",
            "[16:45:12 INFO]: [?] ",
            "[16:45:12 INFO]: [?] Tick durations (min/med/95%ile/max ms) from last 10s, 1m:",
            "[16:45:12 INFO]: [?]  0.2/1.6/2.6/9.6;  0.2/2.0/3.6/71.7",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        let (players, tps, tick_ms) = parse_metrics_from_logs(&lines);
        assert_eq!(players, 1);
        assert_eq!(tps, Some(19.9));
        assert_eq!(tick_ms, Some(1.6));
    }

    #[test]
    fn parse_metrics_from_logs_handles_inline_tps_fallback() {
        let lines =
            vec!["[12:00:00 INFO]: TPS from last 1m, 5m, 15m: *20.0, 20.0, 19.5".to_string()];
        let (players, tps, tick_ms) = parse_metrics_from_logs(&lines);
        assert_eq!(players, 0);
        assert_eq!(tps, Some(20.0));
        assert_eq!(tick_ms, Some(50.0));
    }
}

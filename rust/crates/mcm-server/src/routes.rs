use crate::{
    assets, auth, backups, files, players, properties, schedules, servers, system, worlds,
};
use axum::{
    Router,
    extract::Path,
    middleware,
    routing::{delete, get, post, put},
};

#[derive(Clone)]
pub struct AppState {
    pub auth: auth::AuthStore,
    pub servers: servers::ServerStore,
}

impl axum::extract::FromRef<AppState> for auth::AuthStore {
    fn from_ref(s: &AppState) -> Self {
        s.auth.clone()
    }
}

impl axum::extract::FromRef<AppState> for servers::ServerStore {
    fn from_ref(s: &AppState) -> Self {
        s.servers.clone()
    }
}

pub fn build_router(state: AppState) -> Router {
    let auth_state = state.auth.clone();
    Router::new()
        // Auth & Onboarding
        .route("/api/auth/csrf", get(auth::handle_csrf))
        .route(
            "/api/onboarding/status",
            get(auth::handle_onboarding_status),
        )
        .route("/api/onboarding", post(auth::handle_onboarding))
        .route("/api/auth/login", post(auth::handle_login))
        .route("/api/auth/logout", post(auth::handle_logout))
        .route("/api/auth/me", get(auth::handle_me))
        .route("/api/auth/permissions", get(auth::handle_permissions))
        .route("/api/permissions", get(auth::handle_permissions))
        .route(
            "/api/auth/totp",
            get(|| async { axum::Json(serde_json::json!({ "enabled": false })) }),
        )
        .route(
            "/api/passkey",
            get(|| async { axum::Json(serde_json::json!({ "passkeys": [] })) }),
        )
        // Users
        .route(
            "/api/users",
            get(auth::handle_list_users).post(auth::handle_create_user),
        )
        .route("/api/users/{id}", delete(auth::handle_delete_user))
        // Servers
        .route(
            "/api/servers",
            get(servers::handle_list_servers).post(servers::handle_create_server),
        )
        .route(
            "/api/servers/{id}",
            get(servers::handle_get_server)
                .patch(servers::handle_update_server)
                .delete(servers::handle_delete_server),
        )
        .route(
            "/api/servers/{id}/start",
            post(servers::handle_start_server),
        )
        .route("/api/servers/{id}/stop", post(servers::handle_stop_server))
        .route("/api/servers/{id}/kill", post(servers::handle_kill_server))
        .route(
            "/api/servers/{id}/restart",
            post(servers::handle_restart_server),
        )
        .route(
            "/api/servers/{id}/recreate",
            post(servers::handle_recreate_server),
        )
        .route(
            "/api/servers/{id}/status",
            get(servers::handle_server_status),
        )
        .route("/api/servers/{id}/stats", get(servers::handle_server_stats))
        .route("/api/servers/{id}/logs", get(servers::handle_server_logs))
        .route(
            "/api/servers/{id}/console",
            get(servers::handle_console_sse),
        )
        .route(
            "/api/servers/{id}/console/ws",
            get(servers::handle_console_ws),
        )
        .route(
            "/api/servers/{id}/console/command",
            post(servers::handle_console_command),
        )
        // Files
        .route(
            "/api/servers/{id}/files",
            get(files::handle_list_files).delete(files::handle_delete_file),
        )
        .route(
            "/api/servers/{id}/files/content",
            get(files::handle_read_file).put(files::handle_write_file),
        )
        .route("/api/servers/{id}/files/mkdir", post(files::handle_mkdir))
        .route(
            "/api/servers/{id}/files/rename",
            post(files::handle_rename_file),
        )
        .route(
            "/api/servers/{id}/files/download",
            get(files::handle_download_file),
        )
        .route(
            "/api/servers/{id}/files/upload",
            post(files::handle_upload_file),
        )
        // Properties
        .route(
            "/api/servers/{id}/properties",
            get(properties::handle_get_properties).put(properties::handle_put_properties),
        )
        // Players (Ops & Whitelist)
        .route(
            "/api/servers/{id}/ops",
            get(players::handle_list_ops).post(players::handle_add_op),
        )
        .route(
            "/api/servers/{id}/ops/{name}",
            delete(players::handle_remove_op),
        )
        .route(
            "/api/servers/{id}/whitelist",
            get(players::handle_list_whitelist).post(players::handle_add_whitelist),
        )
        .route(
            "/api/servers/{id}/whitelist/{name}",
            delete(players::handle_remove_whitelist),
        )
        // Worlds
        .route("/api/servers/{id}/worlds", get(worlds::handle_list_worlds))
        .route(
            "/api/servers/{id}/worlds/{name}",
            delete(worlds::handle_delete_world),
        )
        // Backups
        .route(
            "/api/servers/{id}/backups",
            get(backups::handle_list_backups),
        )
        .route(
            "/api/servers/{id}/backup",
            post(backups::handle_create_backup),
        )
        .route(
            "/api/servers/{id}/restore/{backupId}",
            post(backups::handle_restore_backup),
        )
        .route(
            "/api/servers/{id}/backups/{backupId}/download",
            get(backups::handle_download_backup),
        )
        .route(
            "/api/backups/{backupId}",
            delete(backups::handle_delete_backup),
        )
        // Schedules
        .route(
            "/api/servers/{id}/schedules",
            get(schedules::handle_list_schedules).post(schedules::handle_create_schedule),
        )
        .route(
            "/api/servers/{id}/schedules/{taskId}",
            delete(schedules::handle_delete_schedule),
        )
        // System & Settings & Ports & Jars
        .route("/api/runtime/status", get(system::handle_runtime_status))
        .route("/api/system/capacity", get(system::handle_system_capacity))
        .route(
            "/api/system/java-versions",
            get(system::handle_java_versions),
        )
        .route("/api/jars/software", get(system::handle_list_software))
        .route(
            "/api/jars/{software}/versions",
            get(system::handle_jar_versions),
        )
        .route(
            "/api/jars/{software}/versions/{version}/builds",
            get(system::handle_jar_builds),
        )
        .route("/api/ports/available", get(system::handle_available_ports))
        .route("/api/ports/check", get(system::handle_check_port))
        .route(
            "/api/ports/reservations",
            get(system::handle_list_port_reservations).post(system::handle_create_port_reservation),
        )
        .route(
            "/api/ports/reservations/{id}",
            delete(system::handle_delete_port_reservation),
        )
        .route(
            "/api/settings",
            get(system::handle_get_settings).put(put(|| async {
                axum::Json(serde_json::json!({ "ok": true }))
            })),
        )
        .route(
            "/api/updates",
            get(|| async { axum::Json(serde_json::json!({ "enabled": false, "servers": {} })) }),
        )
        .route(
            "/api/panel/update",
            get(|| async {
                let info = mcm_core::buildinfo::current();
                axum::Json(serde_json::json!({
                    "build": {
                        "commit": info.commit,
                        "built_at": info.built_at,
                        "dirty": info.dirty,
                    },
                    "supported": false,
                    "reason": "Panel updates are managed via apt or systemd.",
                    "latest_commit": info.commit,
                    "available": false,
                }))
            }),
        )
        .route(
            "/api/notifications/discord",
            get(|| async {
                axum::Json(serde_json::json!({
                    "enabled": false,
                    "configured": false,
                    "events": []
                }))
            }),
        )
        .route(
            "/api/dns",
            get(|| async { axum::Json(serde_json::json!({ "configured": false, "enabled": false })) }),
        )
        .route(
            "/api/curseforge/settings",
            get(|| async { axum::Json(serde_json::json!({ "configured": false, "source": "" })) }),
        )
        .route(
            "/api/servers/{id}/install",
            get(|Path(id): Path<String>| async move {
                axum::Json(serde_json::json!({
                    "server_id": id,
                    "installed": true,
                    "version": null,
                    "build": null
                }))
            }),
        )
        .route(
            "/api/servers/{id}/tasks/progress",
            get(|| async { axum::Json(serde_json::json!({ "active": false })) }),
        )
        .route(
            "/api/servers/{id}/backups/progress",
            get(|| async { axum::Json(serde_json::json!({ "active": false })) }),
        )
        .route(
            "/api/servers/{id}/backups/trash",
            get(|| async { axum::Json(serde_json::json!({ "backups": [] })) }),
        )
        .route(
            "/api/servers/{id}/backups/retention",
            get(|| async { axum::Json(serde_json::json!({ "keep_last": 10, "max_age_days": 30 })) }),
        )
        .route(
            "/api/servers/{id}/performance",
            get(|| async { axum::Json(serde_json::json!({ "collection_mode": "off", "points": [] })) }),
        )
        .route(
            "/api/servers/{id}/performance/settings",
            get(|| async { axum::Json(serde_json::json!({ "collection_mode": "off" })) }),
        )
        .route(
            "/api/servers/{id}/dns",
            get(|| async { axum::Json(serde_json::json!({ "enabled": false })) }),
        )
        // Middleware & SPA fallback
        .layer(middleware::from_fn_with_state(
            auth_state,
            auth::auth_middleware,
        ))
        .fallback(assets::spa_handler)
        .with_state(state)
}

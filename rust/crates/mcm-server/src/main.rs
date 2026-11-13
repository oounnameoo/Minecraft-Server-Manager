mod assets;
mod auth;
mod backups;
mod docker;
mod error;
mod files;
mod jars;
mod players;
mod ports;
mod properties;
mod routes;
mod schedules;
mod servers;
mod system;
mod tls;
mod worlds;

use anyhow::Result;
use mcm_core::{buildinfo, config::Config, db::Db};
use std::net::SocketAddr;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cfg = Arc::new(Config::load()?);
    std::fs::create_dir_all(&cfg.data_dir)?;
    std::fs::create_dir_all(&cfg.backup_dir)?;

    let db = Db::open(&cfg.db_path)?;
    let db_conn = db.conn_arc();

    let docker = match docker::DockerClient::connect(&cfg.docker_host) {
        Ok(d) => Some(d),
        Err(err) => {
            eprintln!("warning: container runtime unavailable ({err}); running in degraded mode");
            None
        }
    };

    let secure_cookies = !cfg.http_only;
    let auth_store = auth::AuthStore::new(db_conn.clone(), secure_cookies);
    let server_store = servers::ServerStore::new(db_conn.clone(), cfg.clone(), docker);

    // Auto-start servers marked with auto_start = true
    let boot_servers = server_store.clone();
    tokio::spawn(async move {
        boot_servers.auto_start_servers().await;
    });

    let app_state = routes::AppState {
        auth: auth_store,
        servers: server_store,
    };
    let app = routes::build_router(app_state);

    let addr: SocketAddr = cfg.addr.parse()?;
    println!(
        "MCM Rust server v{} listening on {} ({})",
        buildinfo::VERSION,
        addr,
        if cfg.http_only { "HTTP" } else { "HTTPS" }
    );

    if cfg.http_only {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;
    } else {
        let tls_cfg = tls::load_or_generate_tls(
            &cfg.data_dir,
            if cfg.tls_cert.is_empty() {
                None
            } else {
                Some(&cfg.tls_cert)
            },
            if cfg.tls_key.is_empty() {
                None
            } else {
                Some(&cfg.tls_key)
            },
        )
        .await?;
        axum_server::bind_rustls(addr, tls_cfg)
            .serve(app.into_make_service())
            .await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use serde_json::Value;
    use tempfile::TempDir;
    use tower::ServiceExt;

    fn test_app(tmp: &TempDir) -> axum::Router {
        let mut cfg = Config::load().unwrap();
        cfg.data_dir = tmp.path().join("data");
        cfg.backup_dir = tmp.path().join("backups");
        cfg.db_path = tmp.path().join("mcm.db");
        cfg.http_only = true;
        std::fs::create_dir_all(&cfg.data_dir).unwrap();
        std::fs::create_dir_all(&cfg.backup_dir).unwrap();

        let db = Db::open(&cfg.db_path).unwrap();
        let db_conn = db.conn_arc();
        let auth_store = auth::AuthStore::new(db_conn.clone(), false);
        let server_store = servers::ServerStore::new(db_conn, Arc::new(cfg), None);
        routes::build_router(routes::AppState {
            auth: auth_store,
            servers: server_store,
        })
    }

    #[tokio::test]
    async fn full_onboarding_login_server_files_backup_flow() {
        let tmp = TempDir::new().unwrap();
        let app = test_app(&tmp);

        // 1. Onboarding status should be true
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/onboarding/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let val: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(val["onboarding_required"], true);

        // 2. Onboarding creates admin user + session cookie
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/onboarding")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"email":"admin@example.com","password":"password123"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let session_cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();

        // 3. Get CSRF token
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/auth/csrf")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let csrf_cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        let csrf_token = csrf_cookie.split_once('=').unwrap().1.to_string();
        let cookie_hdr = format!("{session_cookie}; {csrf_cookie}");

        // 4. Create custom server (auto_start + start_command)
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/servers")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &cookie_hdr)
                    .header("X-CSRF-Token", &csrf_token)
                    .body(Body::from(
                        r#"{"name":"survival","server_type":"custom","version":"1.21.4","ram_mb":2048,"auto_start":true,"start_command":"java -Xmx2G -jar custom.jar"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let srv: Value = serde_json::from_slice(&bytes).unwrap();
        let srv_id = srv["id"].as_str().unwrap().to_string();
        assert_eq!(srv["auto_start"], true);
        assert_eq!(srv["start_command"], "java -Xmx2G -jar custom.jar");

        // 5. Write & read file
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/servers/{srv_id}/files/content"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, &cookie_hdr)
                    .header("X-CSRF-Token", &csrf_token)
                    .body(Body::from(
                        r#"{"path":"motd.txt","content":"Hello Rust MCM!"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // 6. Create & list backup
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/servers/{srv_id}/backup"))
                    .header(header::COOKIE, &cookie_hdr)
                    .header("X-CSRF-Token", &csrf_token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }
}

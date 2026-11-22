use crate::error::ApiError;
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use axum::{
    Json,
    extract::Request,
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

pub const SESSION_COOKIE_NAME: &str = "mcm_session";
pub const CSRF_COOKIE_NAME: &str = "mcm_csrf";
pub const CSRF_HEADER_NAME: &str = "x-csrf-token";
pub const MIN_PASSWORD_LEN: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionDef {
    pub id: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub server_scoped: bool,
}

pub const PERMISSION_CATALOG: &[PermissionDef] = &[
    PermissionDef {
        id: "servers.view",
        group: "Servers",
        label: "View servers",
        description: "List accessible servers and their status.",
        server_scoped: true,
    },
    PermissionDef {
        id: "servers.create",
        group: "Servers",
        label: "Create servers",
        description: "Create servers. Requires access to all servers.",
        server_scoped: false,
    },
    PermissionDef {
        id: "servers.edit",
        group: "Servers",
        label: "Edit configuration",
        description: "Change server settings, properties and runtime configuration.",
        server_scoped: true,
    },
    PermissionDef {
        id: "servers.delete",
        group: "Servers",
        label: "Delete servers",
        description: "Permanently remove servers and their data.",
        server_scoped: true,
    },
    PermissionDef {
        id: "servers.power",
        group: "Servers",
        label: "Power controls",
        description: "Start, stop, restart and force kill servers.",
        server_scoped: true,
    },
    PermissionDef {
        id: "servers.rebuild",
        group: "Servers",
        label: "Install and rebuild",
        description: "Install software and recreate containers.",
        server_scoped: true,
    },
    PermissionDef {
        id: "servers.export",
        group: "Servers",
        label: "Export servers",
        description: "Download a complete copy including configuration and files.",
        server_scoped: true,
    },
    PermissionDef {
        id: "servers.copy",
        group: "Servers",
        label: "Copy servers",
        description: "Duplicate servers. Also requires create servers and read files.",
        server_scoped: true,
    },
    PermissionDef {
        id: "console.read",
        group: "Console & logs",
        label: "Read console",
        description: "Stream live console output, which may contain sensitive data.",
        server_scoped: true,
    },
    PermissionDef {
        id: "console.write",
        group: "Console & logs",
        label: "Send commands",
        description: "Run arbitrary Minecraft console commands.",
        server_scoped: true,
    },
    PermissionDef {
        id: "logs.read",
        group: "Console & logs",
        label: "Read saved logs",
        description: "Browse saved server logs.",
        server_scoped: true,
    },
    PermissionDef {
        id: "files.read",
        group: "Files",
        label: "Read and download files",
        description: "Read all server files, including secrets and player data.",
        server_scoped: true,
    },
    PermissionDef {
        id: "files.write",
        group: "Files",
        label: "Write files",
        description: "Upload, edit, extract, rename and download files onto the server; may install executable code.",
        server_scoped: true,
    },
    PermissionDef {
        id: "files.delete",
        group: "Files",
        label: "Delete files",
        description: "Remove server files and folders.",
        server_scoped: true,
    },
    PermissionDef {
        id: "players.read",
        group: "Players",
        label: "View players",
        description: "List players, operators and whitelist entries.",
        server_scoped: true,
    },
    PermissionDef {
        id: "players.manage",
        group: "Players",
        label: "Manage players",
        description: "Kick, ban, give items and change player state.",
        server_scoped: true,
    },
    PermissionDef {
        id: "ops.manage",
        group: "Players",
        label: "Manage operators",
        description: "Grant or remove Minecraft operator privileges.",
        server_scoped: true,
    },
    PermissionDef {
        id: "whitelist.manage",
        group: "Players",
        label: "Manage whitelist",
        description: "Add and remove whitelist entries.",
        server_scoped: true,
    },
    PermissionDef {
        id: "mods.read",
        group: "Mods & worlds",
        label: "View mods and plugins",
        description: "Browse installed mods, plugins and modpacks.",
        server_scoped: true,
    },
    PermissionDef {
        id: "mods.manage",
        group: "Mods & worlds",
        label: "Manage mods and plugins",
        description: "Install, update or remove executable mods, plugins and modpacks.",
        server_scoped: true,
    },
    PermissionDef {
        id: "worlds.read",
        group: "Mods & worlds",
        label: "View worlds",
        description: "Browse worlds, regions and chunks.",
        server_scoped: true,
    },
    PermissionDef {
        id: "worlds.manage",
        group: "Mods & worlds",
        label: "Manage worlds",
        description: "Create, import, activate, delete worlds and reset chunks.",
        server_scoped: true,
    },
    PermissionDef {
        id: "worlds.export",
        group: "Mods & worlds",
        label: "Export worlds",
        description: "Download world archives.",
        server_scoped: true,
    },
    PermissionDef {
        id: "backups.read",
        group: "Backups & schedules",
        label: "View backups",
        description: "List backups and watch backup progress.",
        server_scoped: true,
    },
    PermissionDef {
        id: "backups.create",
        group: "Backups & schedules",
        label: "Create and upload backups",
        description: "Create snapshots and upload backup archives.",
        server_scoped: true,
    },
    PermissionDef {
        id: "backups.restore",
        group: "Backups & schedules",
        label: "Restore backups",
        description: "Replace server data from a backup, including executable files.",
        server_scoped: true,
    },
    PermissionDef {
        id: "backups.delete",
        group: "Backups & schedules",
        label: "Manage backup retention & trash",
        description: "Set retention, protect backups, move backups to trash, recover them and permanently delete archives.",
        server_scoped: true,
    },
    PermissionDef {
        id: "backups.download",
        group: "Backups & schedules",
        label: "Download backups",
        description: "Download complete backups, including configuration and secrets.",
        server_scoped: true,
    },
    PermissionDef {
        id: "schedules.read",
        group: "Backups & schedules",
        label: "View schedules",
        description: "Read scheduled tasks and their commands.",
        server_scoped: true,
    },
    PermissionDef {
        id: "schedules.manage",
        group: "Backups & schedules",
        label: "Manage schedules",
        description: "Create and manage persistent automation.",
        server_scoped: true,
    },
    PermissionDef {
        id: "performance.read",
        group: "Monitoring & DNS",
        label: "View performance",
        description: "Read resource usage and performance history.",
        server_scoped: true,
    },
    PermissionDef {
        id: "performance.manage",
        group: "Monitoring & DNS",
        label: "Manage profiling",
        description: "Change collection settings and capture spark reports.",
        server_scoped: true,
    },
    PermissionDef {
        id: "dns.read",
        group: "Monitoring & DNS",
        label: "View server DNS",
        description: "Read published join addresses.",
        server_scoped: true,
    },
    PermissionDef {
        id: "dns.manage",
        group: "Monitoring & DNS",
        label: "Publish server DNS",
        description: "Publish or remove DNS records using configured providers.",
        server_scoped: true,
    },
    PermissionDef {
        id: "system.read",
        group: "Panel administration",
        label: "View host status",
        description: "Read host capacity and container runtime status.",
        server_scoped: false,
    },
    PermissionDef {
        id: "settings.manage",
        group: "Panel administration",
        label: "Manage shared settings",
        description: "Read and change shared configuration, credentials, DNS providers and notifications.",
        server_scoped: false,
    },
    PermissionDef {
        id: "updates.manage",
        group: "Panel administration",
        label: "Manage update checks",
        description: "Configure and run update checks across all servers.",
        server_scoped: false,
    },
    PermissionDef {
        id: "panel.update",
        group: "Panel administration",
        label: "Update the panel",
        description: "Check and install MCM application updates.",
        server_scoped: false,
    },
    PermissionDef {
        id: "users.manage",
        group: "Panel administration",
        label: "Manage users and permissions",
        description: "Full account administration, including creating administrators and resetting passwords.",
        server_scoped: false,
    },
];

#[derive(Debug, Clone, Serialize)]
pub struct RolePreset {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub permissions: Vec<&'static str>,
}

pub fn role_presets() -> Vec<RolePreset> {
    let all: Vec<&'static str> = PERMISSION_CATALOG.iter().map(|p| p.id).collect();
    vec![
        RolePreset {
            id: "administrator",
            label: "Administrator",
            description: "Full access to every server and all panel administration.",
            permissions: all,
        },
        RolePreset {
            id: "operator",
            label: "Operator",
            description: "Operate assigned servers, manage content and create backups. No panel administration or server deletion.",
            permissions: vec![
                "servers.view",
                "servers.edit",
                "servers.power",
                "servers.rebuild",
                "console.read",
                "console.write",
                "logs.read",
                "files.read",
                "files.write",
                "files.delete",
                "players.read",
                "players.manage",
                "ops.manage",
                "whitelist.manage",
                "mods.read",
                "mods.manage",
                "worlds.read",
                "worlds.manage",
                "worlds.export",
                "backups.read",
                "backups.create",
                "backups.download",
                "schedules.read",
                "schedules.manage",
                "performance.read",
                "performance.manage",
                "dns.read",
            ],
        },
        RolePreset {
            id: "viewer",
            label: "Viewer",
            description: "Read server status, console, logs, players, content lists and monitoring. Cannot change server data.",
            permissions: vec![
                "servers.view",
                "console.read",
                "logs.read",
                "players.read",
                "mods.read",
                "worlds.read",
                "backups.read",
                "schedules.read",
                "performance.read",
                "dns.read",
            ],
        },
        RolePreset {
            id: "custom",
            label: "Custom",
            description: "Choose individual permissions for this account.",
            permissions: vec![],
        },
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub email: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    #[serde(skip_serializing)]
    #[allow(dead_code)]
    pub totp_secret: String,
    pub totp_enabled: bool,
    pub role: String,
    pub permissions: Vec<String>,
    pub all_servers: bool,
    pub server_ids: Vec<String>,
    pub disabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

impl User {
    pub fn has_permission(&self, perm: &str) -> bool {
        if self.disabled {
            return false;
        }
        if self.role == "administrator" {
            return true;
        }
        self.permissions.iter().any(|p| p == perm)
    }

    pub fn can_access_server(&self, server_id: &str) -> bool {
        if self.disabled {
            return false;
        }
        if self.role == "administrator" || self.all_servers {
            return true;
        }
        self.server_ids.iter().any(|id| id == server_id)
    }
}

pub fn hash_password(password: &str) -> Result<String, ApiError> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| ApiError::internal(format!("hash password: {e}")))
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

pub fn hash_token(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn generate_random_hex(num_bytes: usize) -> String {
    let mut buf = vec![0u8; num_bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

#[derive(Clone)]
pub struct AuthStore {
    pub db: Arc<Mutex<Connection>>,
    pub secure_cookies: bool,
}

impl AuthStore {
    pub fn new(db: Arc<Mutex<Connection>>, secure_cookies: bool) -> Self {
        Self { db, secure_cookies }
    }

    pub fn user_count(&self) -> Result<i64, ApiError> {
        let conn = self.db.lock().unwrap();
        let count: i64 = conn.query_row("SELECT COUNT(1) FROM users", [], |r| r.get(0))?;
        Ok(count)
    }

    pub fn create_first_user(&self, email: &str, password_hash: &str) -> Result<User, ApiError> {
        let email = email.trim().to_lowercase();
        let now = Utc::now().to_rfc3339();
        let id = Uuid::new_v4().to_string();
        let all_perms: Vec<&str> = PERMISSION_CATALOG.iter().map(|p| p.id).collect();
        let perms_json = serde_json::to_string(&all_perms).unwrap_or_else(|_| "[]".into());

        let conn = self.db.lock().unwrap();
        let count: i64 = conn.query_row("SELECT COUNT(1) FROM users", [], |r| r.get(0))?;
        if count > 0 {
            return Err(ApiError::conflict(
                "onboarding_complete",
                "a user already exists",
            ));
        }

        conn.execute(
            "INSERT INTO users (id, email, password_hash, totp_secret, totp_enabled, webauthn_id, created_at, updated_at, role, permissions, all_servers, server_ids, disabled)
             VALUES (?1, ?2, ?3, '', 0, '', ?4, ?5, 'administrator', ?6, 1, '[]', 0)",
            params![id, email, password_hash, now, now, perms_json],
        )?;
        drop(conn);
        self.get_user_by_id(&id)?
            .ok_or_else(|| ApiError::internal("created user missing"))
    }

    pub fn create_user(
        &self,
        email: &str,
        password_hash: &str,
        role: &str,
        permissions: &[String],
        all_servers: bool,
        server_ids: &[String],
    ) -> Result<User, ApiError> {
        let email = email.trim().to_lowercase();
        let now = Utc::now().to_rfc3339();
        let id = Uuid::new_v4().to_string();
        let perms_json = serde_json::to_string(permissions).unwrap_or_else(|_| "[]".into());
        let sids_json = serde_json::to_string(server_ids).unwrap_or_else(|_| "[]".into());

        let conn = self.db.lock().unwrap();
        let exists: i64 = conn.query_row(
            "SELECT COUNT(1) FROM users WHERE email = ?1",
            params![email],
            |r| r.get(0),
        )?;
        if exists > 0 {
            return Err(ApiError::conflict("email_taken", "email already in use"));
        }

        conn.execute(
            "INSERT INTO users (id, email, password_hash, totp_secret, totp_enabled, webauthn_id, created_at, updated_at, role, permissions, all_servers, server_ids, disabled)
             VALUES (?1, ?2, ?3, '', 0, '', ?4, ?5, ?6, ?7, ?8, ?9, 0)",
            params![
                id,
                email,
                password_hash,
                now,
                now,
                role,
                perms_json,
                if all_servers { 1 } else { 0 },
                sids_json
            ],
        )?;
        drop(conn);
        self.get_user_by_id(&id)?
            .ok_or_else(|| ApiError::internal("created user missing"))
    }

    pub fn get_user_by_email(&self, email: &str) -> Result<Option<User>, ApiError> {
        let email = email.trim().to_lowercase();
        let conn = self.db.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, email, password_hash, COALESCE(totp_secret,''), COALESCE(totp_enabled,0),
                    role, permissions, all_servers, server_ids, disabled, created_at, updated_at
             FROM users WHERE email = ?1",
        )?;
        stmt.query_row(params![email], row_to_user)
            .optional()
            .map_err(Into::into)
    }

    pub fn get_user_by_id(&self, id: &str) -> Result<Option<User>, ApiError> {
        let conn = self.db.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, email, password_hash, COALESCE(totp_secret,''), COALESCE(totp_enabled,0),
                    role, permissions, all_servers, server_ids, disabled, created_at, updated_at
             FROM users WHERE id = ?1",
        )?;
        stmt.query_row(params![id], row_to_user)
            .optional()
            .map_err(Into::into)
    }

    pub fn list_users(&self) -> Result<Vec<User>, ApiError> {
        let conn = self.db.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, email, password_hash, COALESCE(totp_secret,''), COALESCE(totp_enabled,0),
                    role, permissions, all_servers, server_ids, disabled, created_at, updated_at
             FROM users ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], row_to_user)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn create_session(&self, user_id: &str) -> Result<String, ApiError> {
        let raw = generate_random_hex(32);
        let hash = hash_token(&raw);
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let expires = now + Duration::days(7);

        let conn = self.db.lock().unwrap();
        conn.execute(
            "INSERT INTO sessions (id, user_id, token_hash, expires_at, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, user_id, hash, expires.to_rfc3339(), now.to_rfc3339()],
        )?;
        Ok(raw)
    }

    pub fn validate_session(&self, raw_token: &str) -> Result<Option<User>, ApiError> {
        let hash = hash_token(raw_token);
        let row: Option<(String, String)> = {
            let conn = self.db.lock().unwrap();
            conn.query_row(
                "SELECT user_id, expires_at FROM sessions WHERE token_hash = ?1",
                params![hash],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
        };

        let Some((user_id, expires_at)) = row else {
            return Ok(None);
        };
        let Ok(exp) = DateTime::parse_from_rfc3339(&expires_at) else {
            return Ok(None);
        };
        if Utc::now() > exp.with_timezone(&Utc) {
            return Ok(None);
        }
        let user = self.get_user_by_id(&user_id)?;
        if let Some(u) = user {
            if u.disabled {
                return Ok(None);
            }
            return Ok(Some(u));
        }
        Ok(None)
    }

    pub fn delete_session(&self, raw_token: &str) -> Result<(), ApiError> {
        let hash = hash_token(raw_token);
        let conn = self.db.lock().unwrap();
        conn.execute("DELETE FROM sessions WHERE token_hash = ?1", params![hash])?;
        Ok(())
    }

    pub fn should_set_secure_cookie(&self, req_headers: &HeaderMap) -> bool {
        if !self.secure_cookies {
            return false;
        }
        if let Some(proto) = req_headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            && proto.trim().eq_ignore_ascii_case("http")
        {
            return false;
        }
        for hdr in [header::ORIGIN, header::REFERER] {
            if let Some(val) = req_headers.get(hdr).and_then(|v| v.to_str().ok())
                && val.trim().to_ascii_lowercase().starts_with("http://")
            {
                return false;
            }
        }
        true
    }

    pub fn build_session_cookie_for(
        &self,
        req_headers: &HeaderMap,
        raw_token: &str,
        max_age_secs: i64,
    ) -> String {
        let secure = if self.should_set_secure_cookie(req_headers) {
            "; Secure"
        } else {
            ""
        };
        format!(
            "{SESSION_COOKIE_NAME}={raw_token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age_secs}{secure}"
        )
    }

    pub fn build_csrf_cookie_for(&self, req_headers: &HeaderMap, token: &str) -> String {
        let secure = if self.should_set_secure_cookie(req_headers) {
            "; Secure"
        } else {
            ""
        };
        format!("{CSRF_COOKIE_NAME}={token}; Path=/; SameSite=Lax{secure}")
    }
}

fn row_to_user(row: &rusqlite::Row) -> rusqlite::Result<User> {
    let perms_str: String = row.get(6)?;
    let sids_str: String = row.get(8)?;
    let permissions: Vec<String> = serde_json::from_str(&perms_str).unwrap_or_default();
    let server_ids: Vec<String> = serde_json::from_str(&sids_str).unwrap_or_default();
    Ok(User {
        id: row.get(0)?,
        email: row.get(1)?,
        password_hash: row.get(2)?,
        totp_secret: row.get(3)?,
        totp_enabled: row.get::<_, i64>(4)? != 0,
        role: row.get(5)?,
        permissions,
        all_servers: row.get::<_, i64>(7)? != 0,
        server_ids,
        disabled: row.get::<_, i64>(9)? != 0,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

pub fn extract_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    for val in headers.get_all(header::COOKIE) {
        let Ok(cookie_header) = val.to_str() else {
            continue;
        };
        for part in cookie_header.split(';') {
            let trimmed = part.trim();
            if let Some((k, v)) = trimmed.split_once('=')
                && k.trim() == name
            {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

// --- Middleware ---

pub async fn auth_middleware(
    axum::extract::State(auth): axum::extract::State<AuthStore>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let path = req.uri().path().to_string();
    let method = req.method().clone();

    // Only guard /api routes
    if !path.starts_with("/api/") {
        return Ok(next.run(req).await);
    }

    // CSRF validation for non-safe methods on non-exempt paths
    let is_safe = matches!(method, Method::GET | Method::HEAD | Method::OPTIONS);
    let csrf_exempt = matches!(
        path.as_str(),
        "/api/onboarding"
            | "/api/auth/login"
            | "/api/auth/csrf"
            | "/api/passkey/login/begin"
            | "/api/passkey/login/finish"
    );
    if !is_safe && !csrf_exempt {
        let cookie_csrf = extract_cookie(req.headers(), CSRF_COOKIE_NAME);
        let header_csrf = req
            .headers()
            .get(CSRF_HEADER_NAME)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        match (cookie_csrf, header_csrf) {
            (Some(c), Some(h)) if !c.is_empty() && c == h => {}
            _ => {
                return Err(ApiError::new(
                    StatusCode::FORBIDDEN,
                    "csrf_invalid",
                    "missing or invalid CSRF token",
                ));
            }
        }
    }

    // Public endpoints
    let public = matches!(
        path.as_str(),
        "/api/auth/csrf"
            | "/api/onboarding/status"
            | "/api/onboarding"
            | "/api/auth/login"
            | "/api/auth/logout"
    );
    if public {
        return Ok(next.run(req).await);
    }

    let Some(token) = extract_cookie(req.headers(), SESSION_COOKIE_NAME) else {
        return Err(ApiError::unauthorized("authentication required"));
    };
    let Some(user) = auth.validate_session(&token)? else {
        return Err(ApiError::unauthorized("invalid or expired session"));
    };

    req.extensions_mut().insert(user);
    Ok(next.run(req).await)
}

// --- HTTP Handlers ---

pub async fn handle_csrf(
    axum::extract::State(auth): axum::extract::State<AuthStore>,
    req_headers: HeaderMap,
) -> impl IntoResponse {
    let token = generate_random_hex(32);
    let cookie = auth.build_csrf_cookie_for(&req_headers, &token);
    let mut headers = HeaderMap::new();
    headers.insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).unwrap());
    (headers, Json(serde_json::json!({ "csrf_token": token })))
}

pub async fn handle_onboarding_status(
    axum::extract::State(auth): axum::extract::State<AuthStore>,
) -> Result<impl IntoResponse, ApiError> {
    let count = auth.user_count()?;
    Ok(Json(
        serde_json::json!({ "onboarding_required": count == 0 }),
    ))
}

#[derive(Deserialize)]
pub struct OnboardingReq {
    pub email: String,
    pub password: String,
}

pub async fn handle_onboarding(
    axum::extract::State(auth): axum::extract::State<AuthStore>,
    req_headers: HeaderMap,
    Json(req): Json<OnboardingReq>,
) -> Result<impl IntoResponse, ApiError> {
    let email = req.email.trim().to_lowercase();
    if email.is_empty() || req.password.len() < MIN_PASSWORD_LEN {
        return Err(ApiError::bad_request(
            "email and a password of at least 8 characters are required",
        ));
    }
    let hash = hash_password(&req.password)?;
    let user = auth.create_first_user(&email, &hash)?;
    let token = auth.create_session(&user.id)?;
    let cookie = auth.build_session_cookie_for(&req_headers, &token, 7 * 24 * 3600);
    let mut headers = HeaderMap::new();
    headers.insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).unwrap());
    Ok((StatusCode::CREATED, headers, Json(user)))
}

#[derive(Deserialize)]
pub struct LoginReq {
    pub email: String,
    pub password: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub totp_code: String,
}

pub async fn handle_login(
    axum::extract::State(auth): axum::extract::State<AuthStore>,
    req_headers: HeaderMap,
    Json(req): Json<LoginReq>,
) -> Result<impl IntoResponse, ApiError> {
    let email = req.email.trim().to_lowercase();
    let Some(user) = auth.get_user_by_email(&email)? else {
        return Err(ApiError::unauthorized("invalid email or password"));
    };
    if user.disabled {
        return Err(ApiError::forbidden("account is disabled"));
    }
    if !verify_password(&req.password, &user.password_hash) {
        return Err(ApiError::unauthorized("invalid email or password"));
    }
    let token = auth.create_session(&user.id)?;
    let cookie = auth.build_session_cookie_for(&req_headers, &token, 7 * 24 * 3600);
    let mut headers = HeaderMap::new();
    headers.insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).unwrap());
    Ok((headers, Json(user)))
}

pub async fn handle_logout(
    axum::extract::State(auth): axum::extract::State<AuthStore>,
    req_headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    if let Some(tok) = extract_cookie(&req_headers, SESSION_COOKIE_NAME) {
        let _ = auth.delete_session(&tok);
    }
    let cookie = auth.build_session_cookie_for(&req_headers, "", 0);
    let mut headers = HeaderMap::new();
    headers.insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).unwrap());
    Ok((headers, Json(serde_json::json!({ "ok": true }))))
}

pub async fn handle_me(axum::Extension(user): axum::Extension<User>) -> impl IntoResponse {
    Json(user)
}

pub async fn handle_permissions() -> impl IntoResponse {
    Json(serde_json::json!({
        "permissions": PERMISSION_CATALOG,
        "presets": role_presets(),
    }))
}

pub async fn handle_list_users(
    axum::extract::State(auth): axum::extract::State<AuthStore>,
    axum::Extension(user): axum::Extension<User>,
) -> Result<impl IntoResponse, ApiError> {
    if !user.has_permission("users.manage") {
        return Err(ApiError::forbidden("missing permission users.manage"));
    }
    let users = auth.list_users()?;
    Ok(Json(serde_json::json!({ "users": users })))
}

#[derive(Deserialize)]
pub struct CreateUserReq {
    pub email: String,
    pub password: String,
    #[serde(default = "default_viewer_role")]
    pub role: String,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub all_servers: bool,
    #[serde(default)]
    pub server_ids: Vec<String>,
}

fn default_viewer_role() -> String {
    "viewer".to_string()
}

pub async fn handle_create_user(
    axum::extract::State(auth): axum::extract::State<AuthStore>,
    axum::Extension(actor): axum::Extension<User>,
    Json(req): Json<CreateUserReq>,
) -> Result<impl IntoResponse, ApiError> {
    if !actor.has_permission("users.manage") {
        return Err(ApiError::forbidden("missing permission users.manage"));
    }
    if req.email.trim().is_empty() || req.password.len() < MIN_PASSWORD_LEN {
        return Err(ApiError::bad_request(
            "email and a password of at least 8 characters are required",
        ));
    }
    let perms = if req.permissions.is_empty() {
        role_presets()
            .into_iter()
            .find(|p| p.id == req.role)
            .map(|p| p.permissions.into_iter().map(String::from).collect())
            .unwrap_or_default()
    } else {
        req.permissions
    };
    let hash = hash_password(&req.password)?;
    let created = auth.create_user(
        &req.email,
        &hash,
        &req.role,
        &perms,
        req.all_servers,
        &req.server_ids,
    )?;
    Ok((StatusCode::CREATED, Json(created)))
}

pub async fn handle_delete_user(
    axum::extract::State(auth): axum::extract::State<AuthStore>,
    axum::Extension(actor): axum::Extension<User>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    if !actor.has_permission("users.manage") {
        return Err(ApiError::forbidden("missing permission users.manage"));
    }
    if actor.id == id {
        return Err(ApiError::bad_request("cannot delete your own account"));
    }
    let conn = auth.db.lock().unwrap();
    let changed = conn.execute("DELETE FROM users WHERE id = ?1", params![id])?;
    if changed == 0 {
        return Err(ApiError::not_found("user not found"));
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secure_cookie_respects_http_proxy_and_origin_headers() {
        let conn = Connection::open_in_memory().unwrap();
        let store_https = AuthStore::new(Arc::new(Mutex::new(conn)), true);

        let mut https_headers = HeaderMap::new();
        https_headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://mc.example.com"),
        );
        assert!(
            store_https
                .build_session_cookie_for(&https_headers, "tok", 60)
                .contains("; Secure")
        );

        let mut http_origin = HeaderMap::new();
        http_origin.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://192.168.1.50:8080"),
        );
        assert!(
            !store_https
                .build_session_cookie_for(&http_origin, "tok", 60)
                .contains("; Secure")
        );

        let mut http_proto = HeaderMap::new();
        http_proto.insert("x-forwarded-proto", HeaderValue::from_static("http"));
        assert!(
            !store_https
                .build_session_cookie_for(&http_proto, "tok", 60)
                .contains("; Secure")
        );
    }

    #[test]
    fn extract_cookie_reads_across_multiple_cookie_headers() {
        let mut headers = HeaderMap::new();
        headers.append(header::COOKIE, HeaderValue::from_static("mcm_csrf=abc123"));
        headers.append(
            header::COOKIE,
            HeaderValue::from_static("mcm_session=sess_xyz"),
        );
        assert_eq!(
            extract_cookie(&headers, CSRF_COOKIE_NAME).as_deref(),
            Some("abc123")
        );
        assert_eq!(
            extract_cookie(&headers, SESSION_COOKIE_NAME).as_deref(),
            Some("sess_xyz")
        );
    }
}

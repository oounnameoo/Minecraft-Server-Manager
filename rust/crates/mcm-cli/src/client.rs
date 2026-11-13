use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::Method;
use reqwest::header::{COOKIE, SET_COOKIE};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
pub struct ServerSummary {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub server_type: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub host_port: i64,
    #[serde(default)]
    pub ram_mb: i64,
    #[serde(default)]
    pub cpu_limit: f64,
    #[serde(default)]
    pub memory_limit_mb: i64,
    #[serde(default)]
    pub java_version: i64,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default)]
    pub start_command: String,
}

#[derive(Clone)]
pub struct ApiClient {
    base_url: String,
    http: reqwest::Client,
    cookies: Arc<Mutex<Vec<String>>>,
    csrf: Arc<Mutex<Option<String>>>,
}

impl ApiClient {
    pub fn new(base_url: &str, insecure: bool, saved_cookies: Vec<String>) -> Result<Self> {
        let base_url = base_url.trim().trim_end_matches('/').to_string();
        let http = reqwest::Client::builder()
            .danger_accept_invalid_certs(insecure)
            .timeout(Duration::from_secs(60))
            .build()
            .context("build HTTP client")?;
        Ok(Self {
            base_url,
            http,
            cookies: Arc::new(Mutex::new(saved_cookies)),
            csrf: Arc::new(Mutex::new(None)),
        })
    }

    pub fn cookies(&self) -> Vec<String> {
        self.cookies.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn cookie_header(&self) -> String {
        let guard = self.cookies.lock().unwrap();
        guard
            .iter()
            .filter_map(|c| c.split(';').next().map(|kv| kv.trim()))
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn store_set_cookies(&self, headers: &reqwest::header::HeaderMap) {
        let mut guard = self.cookies.lock().unwrap();
        for val in headers.get_all(SET_COOKIE) {
            if let Ok(s) = val.to_str() {
                if let Some((kv, _)) = s.split_once(';') {
                    if let Some((name, _)) = kv.split_once('=') {
                        let name_eq = format!("{}=", name.trim());
                        guard.retain(|existing| !existing.starts_with(&name_eq));
                    }
                    guard.push(kv.trim().to_string());
                } else {
                    guard.push(s.trim().to_string());
                }
            }
        }
    }

    async fn ensure_csrf(&self) -> Result<String> {
        if let Some(tok) = self.csrf.lock().unwrap().clone() {
            return Ok(tok);
        }
        let url = format!("{}/api/auth/csrf", self.base_url);
        let resp = self.http.get(&url).send().await?;
        let val: Value = resp.json().await?;
        let tok = val
            .get("csrf_token")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        *self.csrf.lock().unwrap() = Some(tok.clone());
        Ok(tok)
    }

    pub async fn request_json(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let is_mutation = matches!(
            method,
            Method::POST | Method::PUT | Method::PATCH | Method::DELETE
        );
        let csrf = if is_mutation && path != "/api/auth/csrf" {
            Some(self.ensure_csrf().await?)
        } else {
            None
        };

        let url = format!("{}{}", self.base_url, path);
        let mut req = self.http.request(method, &url);
        let cookie_hdr = self.cookie_header();
        if !cookie_hdr.is_empty() {
            req = req.header(COOKIE, cookie_hdr);
        }
        if let Some(token) = csrf {
            req = req.header("X-CSRF-Token", token);
        }
        req = req.header("Accept", "application/json");
        if let Some(b) = body {
            req = req.json(&b);
        }

        let resp = req.send().await.with_context(|| format!("request {url}"))?;
        self.store_set_cookies(resp.headers());
        let status = resp.status();
        let text = resp.text().await?;
        if status.as_u16() >= 400 {
            if let Ok(v) = serde_json::from_str::<Value>(&text)
                && let Some(msg) = v
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .or_else(|| v.get("message").and_then(|m| m.as_str()))
            {
                bail!("HTTP {}: {msg}", status.as_u16());
            }
            bail!("HTTP {}: {}", status.as_u16(), text.trim());
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_str(&text)?)
    }

    pub async fn open_stream(&self, path: &str) -> Result<reqwest::Response> {
        let url = format!("{}{}", self.base_url, path);
        let stream_client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()?;
        let mut req = stream_client
            .get(&url)
            .header("Accept", "text/event-stream");
        let cookie_hdr = self.cookie_header();
        if !cookie_hdr.is_empty() {
            req = req.header(COOKIE, cookie_hdr);
        }
        let resp = req.send().await?;
        if resp.status().as_u16() >= 400 {
            let status = resp.status().as_u16();
            let txt = resp.text().await.unwrap_or_default();
            bail!("HTTP {status}: {}", txt.trim());
        }
        Ok(resp)
    }

    pub async fn resolve_server_id(&self, id_or_name: &str) -> Result<String> {
        let target = id_or_name.trim();
        if target.is_empty() {
            bail!("server ID or name is required");
        }
        let resp = self.request_json(Method::GET, "/api/servers", None).await?;
        let servers: Vec<ServerSummary> =
            serde_json::from_value(resp.get("servers").cloned().unwrap_or(Value::Array(vec![])))?;
        for s in &servers {
            if s.id == target {
                return Ok(s.id.clone());
            }
        }
        let name_matches: Vec<_> = servers
            .iter()
            .filter(|s| s.name.eq_ignore_ascii_case(target))
            .collect();
        if name_matches.len() == 1 {
            return Ok(name_matches[0].id.clone());
        }
        if name_matches.len() > 1 {
            bail!("multiple servers named {target:?}; specify the server ID");
        }
        let prefix_matches: Vec<_> = servers
            .iter()
            .filter(|s| s.id.starts_with(target))
            .collect();
        if prefix_matches.len() == 1 {
            return Ok(prefix_matches[0].id.clone());
        }
        if prefix_matches.len() > 1 {
            bail!("ambiguous server ID prefix {target:?}");
        }
        bail!("server {target:?} not found")
    }
}

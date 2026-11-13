use std::env;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("invalid port range {0:?}: expected start-end")]
    InvalidPortRange(String),
    #[error("invalid numeric value for {0}: {1}")]
    InvalidNumber(&'static str, String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortRange {
    pub start: u16,
    pub end: u16,
}

#[derive(Debug, Clone)]
pub struct S3Config {
    pub endpoint: String,
    pub access_key: String,
    pub secret_key: String,
    pub bucket: String,
    pub region: String,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub addr: String,
    pub port_range: PortRange,
    pub data_dir: PathBuf,
    pub data_dir_host: String,
    pub backup_dir: PathBuf,
    pub db_path: PathBuf,
    pub docker_host: String,
    pub container_runtime: String,
    pub server_image: String,
    pub http_only: bool,
    pub tls_cert: String,
    pub tls_key: String,
    pub default_cpu_limit: f64,
    pub default_memory_mb: i64,
    pub s3: S3Config,
}

impl Config {
    pub fn load() -> Result<Self, ConfigError> {
        let addr = env_or("MCM_ADDR", ":8080");
        let addr = if addr.starts_with(':') {
            format!("0.0.0.0{addr}")
        } else {
            addr
        };

        let port_range_raw = env_or("MCM_PORT_RANGE", "25565-25665");
        let port_range = parse_port_range(&port_range_raw)?;

        let data_dir = PathBuf::from(env_or("MCM_DATA_DIR", "./data"));
        let backup_dir = env::var("MCM_BACKUP_DIR")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.join("backups"));
        let db_path = env::var("MCM_DB_PATH")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.join("mcm.db"));

        let container_runtime = env_or("MCM_CONTAINER_RUNTIME", "docker");
        let docker_host = env::var("DOCKER_HOST")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "unix:///var/run/docker.sock".to_string());

        let http_only = matches!(
            env_or("MCM_HTTP_ONLY", "false")
                .to_ascii_lowercase()
                .as_str(),
            "true" | "1" | "yes"
        );

        let default_cpu_limit = match env::var("MCM_DEFAULT_CPU_LIMIT") {
            Ok(v) if !v.trim().is_empty() => v
                .trim()
                .parse::<f64>()
                .map_err(|_| ConfigError::InvalidNumber("MCM_DEFAULT_CPU_LIMIT", v))?,
            _ => 0.0,
        };
        let default_memory_mb = match env::var("MCM_DEFAULT_MEMORY_MB") {
            Ok(v) if !v.trim().is_empty() => v
                .trim()
                .parse::<i64>()
                .map_err(|_| ConfigError::InvalidNumber("MCM_DEFAULT_MEMORY_MB", v))?,
            _ => 0,
        };

        Ok(Self {
            addr,
            port_range,
            data_dir,
            data_dir_host: env::var("MCM_DATA_DIR_HOST").unwrap_or_default(),
            backup_dir,
            db_path,
            docker_host,
            container_runtime,
            server_image: env_or("MCM_SERVER_IMAGE", "eclipse-temurin:21-jre-alpine"),
            http_only,
            tls_cert: env::var("MCM_TLS_CERT").unwrap_or_default(),
            tls_key: env::var("MCM_TLS_KEY").unwrap_or_default(),
            default_cpu_limit,
            default_memory_mb,
            s3: S3Config {
                endpoint: env::var("MCM_S3_ENDPOINT").unwrap_or_default(),
                access_key: env::var("MCM_S3_ACCESS_KEY").unwrap_or_default(),
                secret_key: env::var("MCM_S3_SECRET_KEY").unwrap_or_default(),
                bucket: env::var("MCM_S3_BUCKET").unwrap_or_default(),
                region: env_or("MCM_S3_REGION", "us-east-1"),
            },
        })
    }
}

fn env_or(key: &str, default: &str) -> String {
    env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn parse_port_range(raw: &str) -> Result<PortRange, ConfigError> {
    let (a, b) = raw
        .split_once('-')
        .ok_or_else(|| ConfigError::InvalidPortRange(raw.to_string()))?;
    let start = a
        .trim()
        .parse::<u16>()
        .map_err(|_| ConfigError::InvalidPortRange(raw.to_string()))?;
    let end = b
        .trim()
        .parse::<u16>()
        .map_err(|_| ConfigError::InvalidPortRange(raw.to_string()))?;
    if start == 0 || end < start {
        return Err(ConfigError::InvalidPortRange(raw.to_string()));
    }
    Ok(PortRange { start, end })
}

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliConfig {
    pub url: String,
    #[serde(default)]
    pub insecure: bool,
    #[serde(default)]
    pub cookies: Vec<String>,
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            url: "https://localhost:8080".to_string(),
            insecure: false,
            cookies: Vec::new(),
        }
    }
}

pub fn default_config_path() -> Result<PathBuf> {
    if let Ok(p) = env::var("MCM_CLI_CONFIG") {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("HOME environment variable not set"))?;
    Ok(home.join(".config").join("mcm").join("cli.json"))
}

pub fn load_config(path: &Path) -> Result<CliConfig> {
    match fs::read_to_string(path) {
        Ok(raw) if raw.trim().is_empty() => Ok(CliConfig::default()),
        Ok(raw) => {
            let mut cfg: CliConfig =
                serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
            if cfg.url.trim().is_empty() {
                cfg.url = "https://localhost:8080".to_string();
            }
            Ok(cfg)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(CliConfig::default()),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

pub fn save_config(path: &Path, cfg: &CliConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(cfg)? + "\n";
    fs::write(path, json)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

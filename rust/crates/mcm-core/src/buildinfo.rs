use serde::Serialize;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize)]
pub struct BuildInfo {
    pub commit: &'static str,
    pub dirty: bool,
    pub built_at: &'static str,
}

pub fn current() -> BuildInfo {
    BuildInfo {
        commit: option_env!("MCM_BUILD_COMMIT").unwrap_or("unknown"),
        dirty: option_env!("MCM_BUILD_DIRTY") == Some("true"),
        built_at: option_env!("MCM_BUILD_TIME").unwrap_or(""),
    }
}

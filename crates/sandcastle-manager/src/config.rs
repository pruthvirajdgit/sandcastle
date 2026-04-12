use serde::Deserialize;
use std::path::PathBuf;
use std::time::Duration;

use sandcastle_runtime::ResourceLimits;

/// Top-level manager configuration, loaded from sandcastle.toml.
#[derive(Debug, Clone, Deserialize)]
pub struct ManagerConfig {
    /// Maximum number of concurrent sessions.
    #[serde(default = "default_max_sessions")]
    pub max_sessions: usize,

    /// Idle timeout for sessions (seconds).
    #[serde(default = "default_session_timeout")]
    pub session_timeout_seconds: u64,

    /// Default resource limits for sandboxes.
    #[serde(default)]
    pub defaults: ResourceLimits,

    /// File upload/download configuration.
    pub files: FileConfig,
}

/// File security configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct FileConfig {
    /// Directories from which files can be uploaded into sandboxes.
    pub allowed_input_dirs: Vec<PathBuf>,

    /// Directory where downloaded files are written.
    pub output_dir: PathBuf,

    /// Maximum file size in bytes.
    #[serde(default = "default_max_file_size")]
    pub max_file_size_bytes: u64,
}

impl ManagerConfig {
    pub fn session_timeout(&self) -> Duration {
        Duration::from_secs(self.session_timeout_seconds)
    }
}

fn default_max_sessions() -> usize {
    50
}

fn default_session_timeout() -> u64 {
    300
}

fn default_max_file_size() -> u64 {
    10_485_760 // 10 MB
}

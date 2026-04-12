use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

/// Unique sandbox identifier.
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct SandboxId(pub String);

impl std::fmt::Display for SandboxId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for SandboxId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Supported languages for code execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Python,
    Javascript,
    Bash,
}

impl Language {
    /// File extension for this language.
    pub fn extension(&self) -> &'static str {
        match self {
            Language::Python => "py",
            Language::Javascript => "js",
            Language::Bash => "sh",
        }
    }

    /// Runtime binary name inside the sandbox.
    pub fn runtime_binary(&self) -> &'static str {
        match self {
            Language::Python => "python3",
            Language::Javascript => "node",
            Language::Bash => "bash",
        }
    }
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Language::Python => write!(f, "python"),
            Language::Javascript => write!(f, "javascript"),
            Language::Bash => write!(f, "bash"),
        }
    }
}

/// Configuration for creating a new sandbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    pub language: Language,
    pub limits: ResourceLimits,
    #[serde(default)]
    pub env_vars: HashMap<String, String>,
}

/// Resource constraints for a sandbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Memory limit in MB.
    pub memory_mb: u32,
    /// CPU quota as percentage (100 = 1 full core).
    pub cpu_quota: u32,
    /// Maximum execution time per code run.
    #[serde(with = "duration_secs")]
    pub timeout: Duration,
    /// Maximum number of processes/threads.
    pub max_pids: u32,
    /// Maximum disk space in MB for /workspace.
    pub max_disk_mb: u32,
    /// Maximum stdout/stderr output in bytes.
    pub max_output_bytes: u64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            memory_mb: 512,
            cpu_quota: 100,
            timeout: Duration::from_secs(30),
            max_pids: 64,
            max_disk_mb: 512,
            max_output_bytes: 1_048_576, // 1 MB
        }
    }
}

/// Request to execute code in a sandbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecRequest {
    pub code: String,
    #[serde(with = "duration_secs")]
    pub timeout: Duration,
}

/// Result of code execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub execution_time_ms: u64,
    pub timed_out: bool,
    pub oom_killed: bool,
}

/// Sandbox lifecycle status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SandboxStatus {
    Created,
    Running,
    Stopped,
    Failed(String),
}

/// Serde helper for Duration as seconds.
mod duration_secs {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        d.as_secs().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let secs = u64::deserialize(d)?;
        Ok(Duration::from_secs(secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language_extension() {
        assert_eq!(Language::Python.extension(), "py");
        assert_eq!(Language::Javascript.extension(), "js");
        assert_eq!(Language::Bash.extension(), "sh");
    }

    #[test]
    fn test_language_runtime_binary() {
        assert_eq!(Language::Python.runtime_binary(), "python3");
        assert_eq!(Language::Javascript.runtime_binary(), "node");
        assert_eq!(Language::Bash.runtime_binary(), "bash");
    }

    #[test]
    fn test_language_serde_roundtrip() {
        let json = serde_json::to_string(&Language::Python).unwrap();
        assert_eq!(json, "\"python\"");
        let parsed: Language = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, Language::Python);
    }

    #[test]
    fn test_resource_limits_defaults() {
        let limits = ResourceLimits::default();
        assert_eq!(limits.memory_mb, 512);
        assert_eq!(limits.cpu_quota, 100);
        assert_eq!(limits.timeout, Duration::from_secs(30));
        assert_eq!(limits.max_pids, 64);
        assert_eq!(limits.max_disk_mb, 512);
        assert_eq!(limits.max_output_bytes, 1_048_576);
    }

    #[test]
    fn test_sandbox_config_serde() {
        let config = SandboxConfig {
            language: Language::Javascript,
            limits: ResourceLimits::default(),
            env_vars: HashMap::new(),
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: SandboxConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.language, Language::Javascript);
    }

    #[test]
    fn test_exec_request_serde() {
        let req = ExecRequest {
            code: "print('hello')".into(),
            timeout: Duration::from_secs(10),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"timeout\":10"));
        let parsed: ExecRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.code, "print('hello')");
        assert_eq!(parsed.timeout, Duration::from_secs(10));
    }

    #[test]
    fn test_exec_result_serde() {
        let result = ExecResult {
            stdout: "hello\n".into(),
            stderr: String::new(),
            exit_code: 0,
            execution_time_ms: 42,
            timed_out: false,
            oom_killed: false,
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: ExecResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.stdout, "hello\n");
        assert_eq!(parsed.exit_code, 0);
        assert!(!parsed.timed_out);
    }

    #[test]
    fn test_sandbox_id_display() {
        let id = SandboxId("test-123".to_string());
        assert_eq!(format!("{id}"), "test-123");
    }

    #[test]
    fn test_sandbox_status_eq() {
        assert_eq!(SandboxStatus::Created, SandboxStatus::Created);
        assert_ne!(SandboxStatus::Running, SandboxStatus::Stopped);
        assert_eq!(
            SandboxStatus::Failed("err".into()),
            SandboxStatus::Failed("err".into())
        );
    }
}

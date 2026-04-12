use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::schemars::JsonSchema;
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use serde::Deserialize;

use sandcastle_manager::SandboxManager;
use sandcastle_runtime::{
    ExecRequest, ExecResult, Language, Result as SandResult, SandboxConfig, SandboxId,
    SandboxRuntime, SandboxStatus, ResourceLimits,
};

/// Stub runtime for development and testing — returns mock results.
#[allow(dead_code)]
pub struct StubRuntime;

#[async_trait::async_trait]
impl SandboxRuntime for StubRuntime {
    async fn create(&self, _config: &SandboxConfig) -> SandResult<SandboxId> {
        Ok(SandboxId(format!("stub-{}", uuid::Uuid::new_v4())))
    }

    async fn start(&self, _id: &SandboxId) -> SandResult<()> {
        Ok(())
    }

    async fn execute(&self, _id: &SandboxId, request: &ExecRequest) -> SandResult<ExecResult> {
        Ok(ExecResult {
            stdout: format!("[stub] would execute: {}\n", &request.code[..request.code.len().min(50)]),
            stderr: String::new(),
            exit_code: 0,
            execution_time_ms: 0,
            timed_out: false,
            oom_killed: false,
        })
    }

    async fn stop(&self, _id: &SandboxId) -> SandResult<()> {
        Ok(())
    }

    async fn destroy(&self, _id: &SandboxId) -> SandResult<()> {
        Ok(())
    }

    async fn upload_file(&self, _id: &SandboxId, _host: &Path, _sandbox: &Path) -> SandResult<u64> {
        Ok(0)
    }

    async fn download_file(&self, _id: &SandboxId, _sandbox: &Path, _host: &Path) -> SandResult<u64> {
        Ok(0)
    }

    async fn status(&self, _id: &SandboxId) -> SandResult<SandboxStatus> {
        Ok(SandboxStatus::Running)
    }
}

// --- Parameter types for MCP tools ---

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExecuteCodeParams {
    /// The code to execute.
    pub code: String,
    /// Programming language: python, javascript, or bash.
    #[serde(default = "default_language")]
    pub language: String,
    /// Max execution time in seconds.
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateSandboxParams {
    /// Programming language: python, javascript, or bash.
    #[serde(default = "default_language")]
    pub language: String,
    /// Memory limit in MB.
    #[serde(default = "default_memory")]
    pub memory_mb: u32,
    /// Session timeout in seconds.
    #[serde(default = "default_session_timeout")]
    pub timeout_seconds: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExecuteInSessionParams {
    /// Session ID returned by create_sandbox.
    pub session_id: String,
    /// The code to execute.
    pub code: String,
    /// Max execution time in seconds.
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UploadFileParams {
    /// Session ID returned by create_sandbox.
    pub session_id: String,
    /// Absolute path to file on host (must be in allowed_input_dirs).
    pub host_path: String,
    /// Destination path inside sandbox (relative to /workspace).
    pub sandbox_path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DownloadFileParams {
    /// Session ID returned by create_sandbox.
    pub session_id: String,
    /// Path inside sandbox (relative to /workspace).
    pub sandbox_path: String,
    /// Optional destination path on host (defaults to output_dir).
    pub host_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DestroyParams {
    /// Session ID returned by create_sandbox.
    pub session_id: String,
}

fn default_language() -> String { "python".to_string() }
fn default_timeout() -> u32 { 30 }
fn default_memory() -> u32 { 512 }
fn default_session_timeout() -> u32 { 300 }

// --- MCP tool definitions ---

/// MCP tool definitions for Sandcastle.
#[derive(Clone)]
pub struct SandcastleTools {
    manager: Arc<SandboxManager>,
    tool_router: ToolRouter<Self>,
}

#[tool_router(router = tool_router)]
impl SandcastleTools {
    pub fn new(manager: Arc<SandboxManager>) -> Self {
        Self {
            manager,
            tool_router: Self::tool_router(),
        }
    }

    /// Execute code in a new ephemeral sandbox (one-shot).
    #[tool(name = "execute_code", description = "Execute code in an ephemeral sandbox (one-shot)")]
    async fn execute_code(&self, params: Parameters<ExecuteCodeParams>) -> String {
        let p = params.0;
        let lang = match parse_language(&p.language) {
            Ok(l) => l,
            Err(e) => return format!("error: {e}"),
        };

        let timeout = Duration::from_secs(p.timeout_seconds as u64);

        match self.manager.execute_oneshot(&p.code, lang, timeout).await {
            Ok(result) => serde_json::to_string(&result).unwrap_or_else(|e| format!("error: {e}")),
            Err(e) => format!("error: {e}"),
        }
    }

    /// Create a persistent sandbox session.
    #[tool(name = "create_sandbox", description = "Create a persistent sandbox session")]
    async fn create_sandbox(&self, params: Parameters<CreateSandboxParams>) -> String {
        let p = params.0;
        let lang = match parse_language(&p.language) {
            Ok(l) => l,
            Err(e) => return format!("error: {e}"),
        };

        let mut limits = ResourceLimits::default();
        limits.memory_mb = p.memory_mb;
        limits.timeout = Duration::from_secs(p.timeout_seconds as u64);

        let config = SandboxConfig {
            language: lang,
            limits,
            env_vars: Default::default(),
        };

        match self.manager.create_session(config).await {
            Ok(session_id) => {
                serde_json::json!({
                    "session_id": session_id,
                    "language": lang.to_string(),
                })
                .to_string()
            }
            Err(e) => format!("error: {e}"),
        }
    }

    /// Execute code in an existing sandbox session.
    #[tool(name = "execute_in_session", description = "Execute code in an existing sandbox session")]
    async fn execute_in_session(&self, params: Parameters<ExecuteInSessionParams>) -> String {
        let p = params.0;
        let timeout = Duration::from_secs(p.timeout_seconds as u64);

        match self.manager.execute_in_session(&p.session_id, &p.code, timeout).await {
            Ok(result) => serde_json::to_string(&result).unwrap_or_else(|e| format!("error: {e}")),
            Err(e) => format!("error: {e}"),
        }
    }

    /// Upload a file from the host into a sandbox's /workspace directory.
    #[tool(name = "upload_file", description = "Upload a file from host into a sandbox")]
    async fn upload_file(&self, params: Parameters<UploadFileParams>) -> String {
        let p = params.0;

        match self.manager.upload(&p.session_id, Path::new(&p.host_path), &p.sandbox_path).await {
            Ok(bytes) => {
                serde_json::json!({
                    "sandbox_path": format!("/workspace/{}", p.sandbox_path),
                    "size_bytes": bytes,
                })
                .to_string()
            }
            Err(e) => format!("error: {e}"),
        }
    }

    /// Download a file from a sandbox to the host filesystem.
    #[tool(name = "download_file", description = "Download a file from sandbox to host")]
    async fn download_file(&self, params: Parameters<DownloadFileParams>) -> String {
        let p = params.0;

        match self.manager.download(&p.session_id, &p.sandbox_path, p.host_path.as_deref()).await {
            Ok((path, bytes)) => {
                serde_json::json!({
                    "host_path": path.to_string_lossy(),
                    "size_bytes": bytes,
                })
                .to_string()
            }
            Err(e) => format!("error: {e}"),
        }
    }

    /// Destroy a sandbox and all its data immediately.
    #[tool(name = "destroy_sandbox", description = "Destroy a sandbox session")]
    async fn destroy_sandbox(&self, params: Parameters<DestroyParams>) -> String {
        let p = params.0;

        match self.manager.destroy_session(&p.session_id).await {
            Ok(()) => serde_json::json!({ "destroyed": true }).to_string(),
            Err(e) => format!("error: {e}"),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SandcastleTools {}

fn parse_language(s: &str) -> std::result::Result<Language, String> {
    match s {
        "python" => Ok(Language::Python),
        "javascript" | "js" => Ok(Language::Javascript),
        "bash" | "sh" => Ok(Language::Bash),
        other => Err(format!("unsupported language: {other}")),
    }
}

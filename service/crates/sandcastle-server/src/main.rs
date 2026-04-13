mod tools;

use std::collections::HashMap;
use std::sync::Arc;

use clap::Parser;
use rmcp::ServiceExt;
use tracing_subscriber::EnvFilter;

use sandcastle_manager::{FileConfig, ManagerConfig, SandboxManager};
use sandcastle_process::{ProcessConfig, ProcessSandbox};
use sandcastle_gvisor::{GvisorConfig, GvisorSandbox};
use sandcastle_firecracker::{FirecrackerConfig, FirecrackerSandbox};
use sandcastle_runtime::{IsolationLevel, ResourceLimits, SandboxRuntime};

#[derive(Parser)]
#[command(name = "sandcastle", about = "Sandboxed code execution for AI agents")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Start the MCP server
    Serve {
        /// Transport mode
        #[arg(long, default_value = "stdio")]
        transport: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Serve { transport } => {
            if transport != "stdio" {
                anyhow::bail!("only stdio transport is supported in Phase 1");
            }

            // For now, use a stub runtime until ProcessSandbox is implemented
            let config = ManagerConfig {
                max_sessions: 50,
                session_timeout_seconds: 300,
                defaults: ResourceLimits::default(),
                files: FileConfig {
                    allowed_input_dirs: vec!["/tmp/sandcastle/input".into()],
                    output_dir: "/tmp/sandcastle/output".into(),
                    max_file_size_bytes: 10_485_760,
                },
            };

            tracing::info!("starting sandcastle MCP server on stdio");

            // Build runtime map — register available backends
            let mut runtimes: HashMap<IsolationLevel, Arc<dyn SandboxRuntime>> = HashMap::new();

            // Low isolation: ProcessSandbox (Linux namespaces via libcontainer)
            let process_config = ProcessConfig::default();
            let process_runtime = Arc::new(ProcessSandbox::new(process_config));
            if let Err(e) = process_runtime.ensure_dirs() {
                tracing::error!("failed to create process runtime directories: {e}");
                anyhow::bail!("failed to create process runtime directories: {e}");
            }
            runtimes.insert(IsolationLevel::Low, process_runtime);
            tracing::info!("registered backend: low (ProcessSandbox/libcontainer)");

            // Medium isolation: GvisorSandbox (runsc)
            let gvisor_config = GvisorConfig::default();
            let gvisor_runtime = GvisorSandbox::new(gvisor_config);
            if gvisor_runtime.is_available() {
                if let Err(e) = gvisor_runtime.ensure_dirs() {
                    tracing::warn!("failed to create gVisor runtime directories: {e}");
                } else {
                    runtimes.insert(IsolationLevel::Medium, Arc::new(gvisor_runtime));
                    tracing::info!("registered backend: medium (GvisorSandbox/runsc)");
                }
            } else {
                tracing::warn!("runsc not found — medium isolation (gVisor) unavailable");
            }

            // High isolation: FirecrackerSandbox (microVM)
            let fc_config = FirecrackerConfig::default();
            let fc_runtime = FirecrackerSandbox::new(fc_config);
            if fc_runtime.is_available() {
                match fc_runtime.ensure_dirs() {
                    Ok(()) => {
                        runtimes.insert(IsolationLevel::High, Arc::new(fc_runtime));
                        tracing::info!("registered backend: high (FirecrackerSandbox/Firecracker)");
                    }
                    Err(err) => {
                        tracing::warn!(
                            error = %err,
                            "failed to create Firecracker runtime directories — high isolation unavailable"
                        );
                    }
                }
            } else {
                tracing::warn!("firecracker or kernel not found — high isolation unavailable");
            }

            if runtimes.is_empty() {
                anyhow::bail!("no sandbox backends registered — server cannot start without at least one backend");
            }

            let manager = Arc::new(SandboxManager::new(runtimes, config));

            // Start reaper background task
            let reaper = manager.clone();
            tokio::spawn(async move {
                loop {
                    reaper.reap_expired().await;
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            });

            // Start MCP server on stdio
            let service = tools::SandcastleTools::new(manager);
            let transport = rmcp::transport::io::stdio();
            let server = service.serve(transport).await?;
            server.waiting().await?;

            Ok(())
        }
    }
}

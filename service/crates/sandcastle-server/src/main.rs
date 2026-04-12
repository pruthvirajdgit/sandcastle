mod tools;

use std::sync::Arc;

use clap::Parser;
use rmcp::ServiceExt;
use tracing_subscriber::EnvFilter;

use sandcastle_manager::{FileConfig, ManagerConfig, SandboxManager};
use sandcastle_process::{ProcessConfig, ProcessSandbox};
use sandcastle_runtime::ResourceLimits;

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

            let process_config = ProcessConfig::default();
            let runtime = Arc::new(ProcessSandbox::new(process_config));
            // Ensure runtime directories exist
            runtime.ensure_dirs().expect("failed to create runtime directories");
            let manager = Arc::new(SandboxManager::new(runtime, config));

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

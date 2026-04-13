//! Host-side vsock client for communicating with the executor inside a Firecracker VM.
//!
//! Firecracker exposes guest vsock via a Unix domain socket (UDS) proxy.
//! Protocol: connect to UDS, send "CONNECT <port>\n", receive "OK <port>\n",
//! then the connection is a bidirectional byte stream to the guest.

use std::path::Path;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::debug;

use sandcastle_runtime::SandcastleError;

/// A connected vsock session to a guest executor.
pub struct VsockConnection {
    reader: BufReader<tokio::io::ReadHalf<UnixStream>>,
    writer: tokio::io::WriteHalf<UnixStream>,
}

impl VsockConnection {
    /// Connect to the guest executor via the Firecracker vsock UDS proxy.
    ///
    /// `uds_path` is the Unix socket path configured in Firecracker's vsock.
    /// `port` is the vsock port the executor is listening on inside the VM.
    pub async fn connect(uds_path: &Path, port: u32) -> Result<Self, SandcastleError> {
        debug!("connecting to vsock proxy at {:?} port {}", uds_path, port);

        let stream = UnixStream::connect(uds_path)
            .await
            .map_err(|e| SandcastleError::RuntimeError(
                format!("failed to connect to vsock UDS proxy at {:?}: {}", uds_path, e)
            ))?;

        let (read_half, mut write_half) = tokio::io::split(stream);
        let mut reader = BufReader::new(read_half);

        // Send CONNECT request per Firecracker vsock proxy protocol
        let connect_msg = format!("CONNECT {}\n", port);
        write_half.write_all(connect_msg.as_bytes()).await.map_err(|e| {
            SandcastleError::RuntimeError(format!("failed to send CONNECT to vsock proxy: {}", e))
        })?;

        // Read response — expect "OK <port>\n"
        let mut response = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            reader.read_line(&mut response),
        )
        .await
        .map_err(|_| SandcastleError::RuntimeError(
            "timeout waiting for vsock CONNECT response".to_string()
        ))?
        .map_err(|e| SandcastleError::RuntimeError(
            format!("failed to read vsock CONNECT response: {}", e)
        ))?;

        let response = response.trim();
        if !response.starts_with("OK") {
            return Err(SandcastleError::RuntimeError(
                format!("unexpected vsock CONNECT response: {}", response)
            ));
        }

        debug!("vsock connected: {}", response);

        Ok(Self { reader, writer: write_half })
    }

    /// Wait for the executor's readiness signal: {"ready":true}
    pub async fn wait_ready(&mut self) -> Result<(), SandcastleError> {
        let mut line = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.reader.read_line(&mut line),
        )
        .await
        .map_err(|_| SandcastleError::SandboxCreationFailed(
            "timeout waiting for executor readiness inside VM".to_string()
        ))?
        .map_err(|e| SandcastleError::SandboxCreationFailed(
            format!("failed to read executor readiness: {}", e)
        ))?;

        if !line.contains("\"ready\"") {
            return Err(SandcastleError::SandboxCreationFailed(
                format!("unexpected readiness signal from executor: {}", line.trim())
            ));
        }

        debug!("executor inside VM is ready");
        Ok(())
    }

    /// Send a JSON command and read a JSON response (one line each).
    pub async fn execute_json(
        &mut self,
        request: &str,
        timeout: std::time::Duration,
    ) -> Result<String, SandcastleError> {
        // Write the request
        self.writer.write_all(request.as_bytes()).await.map_err(|e| {
            SandcastleError::ExecutionFailed(format!("failed to write to executor: {}", e))
        })?;
        self.writer.write_all(b"\n").await.map_err(|e| {
            SandcastleError::ExecutionFailed(format!("failed to write newline: {}", e))
        })?;
        self.writer.flush().await.map_err(|e| {
            SandcastleError::ExecutionFailed(format!("failed to flush to executor: {}", e))
        })?;

        // Read the response with timeout
        let mut response = String::new();
        tokio::time::timeout(timeout, self.reader.read_line(&mut response))
            .await
            .map_err(|_| SandcastleError::Timeout)?
            .map_err(|e| {
                SandcastleError::ExecutionFailed(format!("failed to read from executor: {}", e))
            })?;

        if response.is_empty() {
            return Err(SandcastleError::ExecutionFailed(
                "executor closed connection (empty response)".to_string()
            ));
        }

        Ok(response)
    }
}

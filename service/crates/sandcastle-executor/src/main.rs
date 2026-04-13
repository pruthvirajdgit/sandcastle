use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Command received from the host via stdin or vsock.
#[derive(Debug, Deserialize)]
struct ExecCommand {
    action: String,
    #[serde(default)]
    language: String,
    #[serde(default)]
    code: String,
    #[serde(default = "default_timeout")]
    timeout_ms: u64,
    #[serde(default = "default_max_output")]
    max_output_bytes: u64,
    /// For upload action: base64-encoded file content
    #[serde(default)]
    content_base64: String,
    /// For upload/download action: path relative to /workspace
    #[serde(default)]
    path: String,
}

fn default_timeout() -> u64 {
    30000
}

fn default_max_output() -> u64 {
    1_048_576 // 1 MB
}

/// Result sent back to the host via stdout.
#[derive(Debug, Serialize)]
struct ExecResponse {
    stdout: String,
    stderr: String,
    exit_code: i32,
    execution_time_ms: u64,
    timed_out: bool,
    oom_killed: bool,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let vsock_mode = args.iter().any(|a| a == "--vsock");

    if vsock_mode {
        #[cfg(feature = "vsock-mode")]
        {
            run_vsock_mode();
        }
        #[cfg(not(feature = "vsock-mode"))]
        {
            eprintln!("executor: --vsock requires vsock-mode feature");
            std::process::exit(1);
        }
    } else {
        run_stdio_mode();
    }
}

/// Standard mode: read JSON commands from stdin, write responses to stdout.
fn run_stdio_mode() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout_lock = stdout.lock();

    // Emit readiness signal so the host knows we're ready for commands
    let _ = writeln!(stdout_lock, r#"{{"ready":true}}"#);
    let _ = stdout_lock.flush();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        if line.trim().is_empty() {
            continue;
        }

        let response = handle_line(&line);
        let _ = writeln!(stdout_lock, "{}", serde_json::to_string(&response).unwrap());
        let _ = stdout_lock.flush();
    }
}

/// Vsock mode: listen on vsock port 5000, accept connections, serve JSON protocol.
#[cfg(feature = "vsock-mode")]
fn run_vsock_mode() {
    use vsock::{VsockAddr, VsockListener};

    const VSOCK_PORT: u32 = 5000;
    // VMADDR_CID_ANY = u32::MAX — listen on any CID
    let addr = VsockAddr::new(u32::MAX, VSOCK_PORT);

    let listener = match VsockListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("executor: failed to bind vsock port {}: {}", VSOCK_PORT, e);
            std::process::exit(1);
        }
    };

    eprintln!("executor: listening on vsock port {}", VSOCK_PORT);

    // Accept connections in a loop (one at a time)
    for conn in listener.incoming() {
        match conn {
            Ok(stream) => handle_vsock_connection(stream),
            Err(e) => {
                eprintln!("executor: vsock accept error: {}", e);
                continue;
            }
        }
    }
}

#[cfg(feature = "vsock-mode")]
fn handle_vsock_connection(stream: vsock::VsockStream) {
    use std::io::BufReader;

    let reader = BufReader::new(&stream);
    let mut writer = &stream;

    // Emit readiness signal
    let _ = writeln!(writer, r#"{{"ready":true}}"#);
    let _ = writer.flush();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        if line.trim().is_empty() {
            continue;
        }

        let response = handle_line(&line);
        let _ = writeln!(writer, "{}", serde_json::to_string(&response).unwrap());
        let _ = writer.flush();
    }
}

/// Parse and handle a single JSON command line.
fn handle_line(line: &str) -> ExecResponse {
    let cmd: ExecCommand = match serde_json::from_str(line) {
        Ok(c) => c,
        Err(e) => {
            return ExecResponse {
                stdout: String::new(),
                stderr: format!("executor: failed to parse command: {e}"),
                exit_code: -1,
                execution_time_ms: 0,
                timed_out: false,
                oom_killed: false,
            };
        }
    };

    match cmd.action.as_str() {
        "exec" => execute_code(&cmd),
        "upload" => handle_upload(&cmd),
        "download" => handle_download(&cmd),
        other => ExecResponse {
            stdout: String::new(),
            stderr: format!("executor: unknown action: {}", other),
            exit_code: -1,
            execution_time_ms: 0,
            timed_out: false,
            oom_killed: false,
        },
    }
}

/// Handle file upload: decode base64 content and write to /workspace/<path>
fn handle_upload(cmd: &ExecCommand) -> ExecResponse {
    use std::path::Path;

    if cmd.path.is_empty() {
        return ExecResponse {
            stdout: String::new(),
            stderr: "executor: upload requires 'path'".to_string(),
            exit_code: -1,
            execution_time_ms: 0,
            timed_out: false,
            oom_killed: false,
        };
    }

    // Sanitize path — no absolute or traversal
    let rel = Path::new(&cmd.path);
    if rel.is_absolute() || cmd.path.contains("..") {
        return ExecResponse {
            stdout: String::new(),
            stderr: "executor: invalid path".to_string(),
            exit_code: -1,
            execution_time_ms: 0,
            timed_out: false,
            oom_killed: false,
        };
    }

    let dest = Path::new("/workspace").join(rel);
    if let Some(parent) = dest.parent() {
        let _ = fs::create_dir_all(parent);
    }

    // Decode base64 — use simple decoder (no external crate)
    let data = match base64_decode(&cmd.content_base64) {
        Ok(d) => d,
        Err(e) => {
            return ExecResponse {
                stdout: String::new(),
                stderr: format!("executor: base64 decode failed: {}", e),
                exit_code: -1,
                execution_time_ms: 0,
                timed_out: false,
                oom_killed: false,
            };
        }
    };

    match fs::write(&dest, &data) {
        Ok(()) => ExecResponse {
            stdout: format!("{} bytes written to {}", data.len(), cmd.path),
            stderr: String::new(),
            exit_code: 0,
            execution_time_ms: 0,
            timed_out: false,
            oom_killed: false,
        },
        Err(e) => ExecResponse {
            stdout: String::new(),
            stderr: format!("executor: write failed: {}", e),
            exit_code: -1,
            execution_time_ms: 0,
            timed_out: false,
            oom_killed: false,
        },
    }
}

/// Handle file download: read file from /workspace/<path> and return base64-encoded content
fn handle_download(cmd: &ExecCommand) -> ExecResponse {
    use std::path::Path;

    if cmd.path.is_empty() {
        return ExecResponse {
            stdout: String::new(),
            stderr: "executor: download requires 'path'".to_string(),
            exit_code: -1,
            execution_time_ms: 0,
            timed_out: false,
            oom_killed: false,
        };
    }

    let rel = Path::new(&cmd.path);
    if rel.is_absolute() || cmd.path.contains("..") {
        return ExecResponse {
            stdout: String::new(),
            stderr: "executor: invalid path".to_string(),
            exit_code: -1,
            execution_time_ms: 0,
            timed_out: false,
            oom_killed: false,
        };
    }

    let src = Path::new("/workspace").join(rel);
    match fs::read(&src) {
        Ok(data) => ExecResponse {
            stdout: base64_encode(&data),
            stderr: String::new(),
            exit_code: 0,
            execution_time_ms: 0,
            timed_out: false,
            oom_killed: false,
        },
        Err(e) => ExecResponse {
            stdout: String::new(),
            stderr: format!("executor: read failed: {}", e),
            exit_code: -1,
            execution_time_ms: 0,
            timed_out: false,
            oom_killed: false,
        },
    }
}

/// Simple base64 encoder (no external crate needed).
fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((n >> 18) & 63) as usize] as char);
        result.push(CHARS[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((n >> 6) & 63) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(n & 63) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

/// Simple base64 decoder (no external crate needed).
fn base64_decode(input: &str) -> std::result::Result<Vec<u8>, String> {
    fn char_to_val(c: u8) -> std::result::Result<u8, String> {
        match c {
            b'A'..=b'Z' => Ok(c - b'A'),
            b'a'..=b'z' => Ok(c - b'a' + 26),
            b'0'..=b'9' => Ok(c - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err(format!("invalid base64 char: {}", c as char)),
        }
    }

    let input = input.trim();
    if input.is_empty() {
        return Ok(Vec::new());
    }

    let bytes: Vec<u8> = input.bytes().filter(|&b| b != b'\n' && b != b'\r').collect();
    if bytes.len() % 4 != 0 {
        return Err("invalid base64 length".to_string());
    }

    let mut result = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let pad = chunk.iter().filter(|&&b| b == b'=').count();
        let v0 = char_to_val(chunk[0])?;
        let v1 = char_to_val(chunk[1])?;
        result.push((v0 << 2) | (v1 >> 4));
        if pad < 2 {
            let v2 = char_to_val(chunk[2])?;
            result.push((v1 << 4) | (v2 >> 2));
            if pad < 1 {
                let v3 = char_to_val(chunk[3])?;
                result.push((v2 << 6) | v3);
            }
        }
    }
    Ok(result)
}

fn execute_code(cmd: &ExecCommand) -> ExecResponse {
    let (ext, runtime) = match cmd.language.as_str() {
        "python" => ("py", "python3"),
        "javascript" => ("js", "node"),
        "bash" => ("sh", "bash"),
        other => {
            return ExecResponse {
                stdout: String::new(),
                stderr: format!("executor: unsupported language: {other}"),
                exit_code: -1,
                execution_time_ms: 0,
                timed_out: false,
                oom_killed: false,
            };
        }
    };

    let code_path = format!("/workspace/code.{ext}");
    if let Err(e) = fs::write(&code_path, &cmd.code) {
        return ExecResponse {
            stdout: String::new(),
            stderr: format!("executor: failed to write code file: {e}"),
            exit_code: -1,
            execution_time_ms: 0,
            timed_out: false,
            oom_killed: false,
        };
    }

    let start = Instant::now();
    let timeout = Duration::from_millis(cmd.timeout_ms);

    let mut child = match Command::new(runtime)
        .arg(&code_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return ExecResponse {
                stdout: String::new(),
                stderr: format!("executor: failed to spawn {runtime}: {e}"),
                exit_code: -1,
                execution_time_ms: start.elapsed().as_millis() as u64,
                timed_out: false,
                oom_killed: false,
            };
        }
    };

    // Poll for completion with timeout
    let result = wait_with_timeout(&mut child, timeout);
    let elapsed = start.elapsed();

    // Clean up code file
    let _ = fs::remove_file(&code_path);

    let (stdout_str, stderr_str) = read_output(&mut child, cmd.max_output_bytes);

    let (exit_code, timed_out, oom_killed) = match result {
        WaitResult::Exited(code) => {
            let oom = code == 137; // SIGKILL from cgroup OOM killer
            (code, false, oom)
        }
        WaitResult::TimedOut => {
            // Kill the child
            let _ = child.kill();
            let _ = child.wait();
            (124, true, false)
        }
    };

    ExecResponse {
        stdout: stdout_str,
        stderr: stderr_str,
        exit_code,
        execution_time_ms: elapsed.as_millis() as u64,
        timed_out,
        oom_killed,
    }
}

enum WaitResult {
    Exited(i32),
    TimedOut,
}

fn wait_with_timeout(child: &mut std::process::Child, timeout: Duration) -> WaitResult {
    let start = Instant::now();
    let poll_interval = Duration::from_millis(10);

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return WaitResult::Exited(status.code().unwrap_or(-1));
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    return WaitResult::TimedOut;
                }
                std::thread::sleep(poll_interval);
            }
            Err(_) => {
                return WaitResult::Exited(-1);
            }
        }
    }
}

fn read_output(child: &mut std::process::Child, max_bytes: u64) -> (String, String) {
    let max = max_bytes as usize;

    let stdout = child
        .stdout
        .take()
        .map(|out| {
            let mut buf = vec![0u8; max];
            let mut reader = io::BufReader::new(out);
            let mut total = 0;
            loop {
                let remaining = max - total;
                if remaining == 0 {
                    break;
                }
                match reader.read(&mut buf[total..]) {
                    Ok(0) => break,
                    Ok(n) => total += n,
                    Err(_) => break,
                }
            }
            buf.truncate(total);
            String::from_utf8_lossy(&buf).into_owned()
        })
        .unwrap_or_default();

    let stderr = child
        .stderr
        .take()
        .map(|err| {
            let mut buf = vec![0u8; max];
            let mut reader = io::BufReader::new(err);
            let mut total = 0;
            loop {
                let remaining = max - total;
                if remaining == 0 {
                    break;
                }
                match reader.read(&mut buf[total..]) {
                    Ok(0) => break,
                    Ok(n) => total += n,
                    Err(_) => break,
                }
            }
            buf.truncate(total);
            String::from_utf8_lossy(&buf).into_owned()
        })
        .unwrap_or_default();

    (stdout, stderr)
}

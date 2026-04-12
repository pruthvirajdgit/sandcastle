use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Command received from the host via stdin.
#[derive(Debug, Deserialize)]
struct ExecCommand {
    action: String,
    language: String,
    code: String,
    timeout_ms: u64,
    #[serde(default = "default_max_output")]
    max_output_bytes: u64,
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
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout_lock = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        if line.trim().is_empty() {
            continue;
        }

        let cmd: ExecCommand = match serde_json::from_str(&line) {
            Ok(c) => c,
            Err(e) => {
                let err_response = ExecResponse {
                    stdout: String::new(),
                    stderr: format!("executor: failed to parse command: {e}"),
                    exit_code: -1,
                    execution_time_ms: 0,
                    timed_out: false,
                    oom_killed: false,
                };
                let _ = writeln!(stdout_lock, "{}", serde_json::to_string(&err_response).unwrap());
                let _ = stdout_lock.flush();
                continue;
            }
        };

        if cmd.action != "exec" {
            let err_response = ExecResponse {
                stdout: String::new(),
                stderr: format!("executor: unknown action: {}", cmd.action),
                exit_code: -1,
                execution_time_ms: 0,
                timed_out: false,
                oom_killed: false,
            };
            let _ = writeln!(stdout_lock, "{}", serde_json::to_string(&err_response).unwrap());
            let _ = stdout_lock.flush();
            continue;
        }

        let response = execute_code(&cmd);
        let _ = writeln!(stdout_lock, "{}", serde_json::to_string(&response).unwrap());
        let _ = stdout_lock.flush();
    }
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

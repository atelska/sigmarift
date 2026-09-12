use std::{
    collections::VecDeque,
    io::{self, Read},
    os::unix::process::CommandExt,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

const CAPTURE_CLOSE_GRACE: Duration = Duration::from_millis(100);

/// Runtime-wide limits for model-requested local execution.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ExecutionSettings {
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default = "default_max_stdout_bytes")]
    pub max_stdout_bytes: usize,
    #[serde(default = "default_max_stderr_bytes")]
    pub max_stderr_bytes: usize,
    #[serde(default = "default_max_tool_calls_per_turn")]
    pub max_tool_calls_per_turn: usize,
}

impl Default for ExecutionSettings {
    fn default() -> Self {
        Self {
            timeout_seconds: default_timeout_seconds(),
            max_stdout_bytes: default_max_stdout_bytes(),
            max_stderr_bytes: default_max_stderr_bytes(),
            max_tool_calls_per_turn: default_max_tool_calls_per_turn(),
        }
    }
}

fn default_timeout_seconds() -> u64 {
    30
}

fn default_max_stdout_bytes() -> usize {
    2 * 1024
}

fn default_max_stderr_bytes() -> usize {
    1024
}

fn default_max_tool_calls_per_turn() -> usize {
    8
}

/// The bounded outcome of one local shell command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub status: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u128,
    pub timed_out: bool,
}

/// Reports whether commands inherit administrator privileges from SigmaRift.
pub fn running_as_administrator() -> bool {
    // SAFETY: `geteuid` has no memory-safety preconditions and does not dereference
    // caller-provided data.
    unsafe { libc::geteuid() == 0 }
}

/// Runs a model-selected command with caller-owned runtime limits.
#[cfg(test)]
pub fn execute_with_settings(
    command: &str,
    settings: &ExecutionSettings,
) -> io::Result<ExecResult> {
    execute_with_settings_and_cancel(command, settings, &AtomicBool::new(false))
}

/// Runs a model-selected command and stops its complete process group when cancelled.
pub fn execute_with_settings_and_cancel(
    command: &str,
    settings: &ExecutionSettings,
    cancelled: &AtomicBool,
) -> io::Result<ExecResult> {
    let mut process = Command::new("sh");
    process
        .arg("-c")
        .arg(command)
        // Commands must not read TUI input or open its controlling terminal.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // The shell and everything it launches run in a separate session so a
    // timeout can stop the complete command tree without inheriting the TUI.
    // SAFETY: the closure only calls async-signal-safe `setsid` between fork and exec.
    unsafe {
        process.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = process.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("shell stdout was not captured"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("shell stderr was not captured"))?;
    let stdout_limit = settings.max_stdout_bytes.max(1);
    let stderr_limit = settings.max_stderr_bytes.max(1);
    let stdout = thread::spawn(move || capture_output(stdout, stdout_limit, "stdout"));
    let stderr = thread::spawn(move || capture_output(stderr, stderr_limit, "stderr"));
    let timeout = Duration::from_secs(settings.timeout_seconds.max(1));
    let started = Instant::now();
    let mut timed_out = false;
    let mut interrupted = false;
    let status = loop {
        if child_has_exited(&child)? {
            // Keep the exited shell unreaped so its process-group ID cannot be reused
            // while readers get a brief chance to observe an ordinary pipe close.
            let capture_deadline = Instant::now() + CAPTURE_CLOSE_GRACE;
            while (!stdout.is_finished() || !stderr.is_finished())
                && Instant::now() < capture_deadline
            {
                thread::sleep(Duration::from_millis(1));
            }
            if !stdout.is_finished() || !stderr.is_finished() {
                // Background descendants still holding the pipes would make the joins block.
                kill_process_group(&mut child)?;
            }
            break child.wait()?;
        }
        if cancelled.load(Ordering::Relaxed) {
            interrupted = kill_process_group(&mut child)?;
            break child.wait()?;
        }
        if started.elapsed() >= timeout {
            timed_out = kill_process_group(&mut child)?;
            break child.wait()?;
        }
        thread::sleep(Duration::from_millis(10));
    };

    let duration_ms = started.elapsed().as_millis();
    Ok(ExecResult {
        stdout: join_capture(stdout)?,
        stderr: join_capture(stderr)?,
        status: if interrupted {
            "interrupted by user".to_owned()
        } else if timed_out {
            format!(
                "timed out after {} seconds",
                settings.timeout_seconds.max(1)
            )
        } else {
            status.to_string()
        },
        exit_code: status.code(),
        duration_ms,
        timed_out,
    })
}

fn child_has_exited(child: &std::process::Child) -> io::Result<bool> {
    // `waitid` with `WNOWAIT` observes termination without reaping the process.
    // SAFETY: `info` points to writable initialized storage and the child PID is valid.
    let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            child.id(),
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `waitid` initialized `info`; a zero PID means no state change was available.
    Ok(unsafe { info.si_pid() } != 0)
}

/// Returns whether the process group was still running and received SIGKILL.
fn kill_process_group(child: &mut std::process::Child) -> io::Result<bool> {
    // SAFETY: the negative child PID targets the process group created by `setsid` in
    // `execute_with_settings_and_cancel`; no Rust memory is accessed by `kill`.
    let result = unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    if result == 0 {
        Ok(true)
    } else {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            // The child exited between `try_wait` and `kill`. Reap it normally so
            // its actual status and captured output are preserved.
            Ok(false)
        } else {
            Err(error)
        }
    }
}

fn join_capture(handle: thread::JoinHandle<io::Result<String>>) -> io::Result<String> {
    handle
        .join()
        .map_err(|_| io::Error::other("output reader thread panicked"))?
}

fn capture_output<R: Read>(mut reader: R, limit: usize, stream: &str) -> io::Result<String> {
    let head_limit = limit * 2 / 3;
    let tail_limit = limit - head_limit;
    let mut head = Vec::with_capacity(head_limit);
    let mut tail = VecDeque::with_capacity(tail_limit);
    let mut total_bytes = 0_usize;
    let mut buffer = [0_u8; 4096];

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total_bytes += read;
        for byte in &buffer[..read] {
            if head.len() < head_limit {
                head.push(*byte);
            } else {
                if tail.len() == tail_limit {
                    tail.pop_front();
                }
                tail.push_back(*byte);
            }
        }
    }

    if total_bytes <= limit {
        head.extend(tail);
        return Ok(String::from_utf8_lossy(&head).into_owned());
    }

    let mut output = String::from_utf8_lossy(&head).into_owned();
    output.push_str(&format!(
        "\n\n[{stream} truncated: showing first {head_limit} and last {tail_limit} bytes of {total_bytes} bytes. Narrow the command or ask the user to specify the scope.]\n\n"
    ));
    output.push_str(&String::from_utf8_lossy(tail.make_contiguous()));
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::Duration,
    };

    use super::*;

    #[test]
    fn shell_result_keeps_both_streams_and_status() {
        let result = execute_with_settings(
            "printf out; printf err >&2; exit 7",
            &ExecutionSettings::default(),
        )
        .unwrap();

        assert_eq!(result.stdout, "out");
        assert_eq!(result.stderr, "err");
        assert_eq!(result.status, "exit status: 7");
        assert_eq!(result.exit_code, Some(7));
        assert!(!result.timed_out);
    }

    #[test]
    fn shell_exit_stops_background_descendants_holding_capture_pipes() {
        let started = Instant::now();
        let result =
            execute_with_settings("sleep 10 & printf done", &ExecutionSettings::default()).unwrap();

        assert_eq!(result.stdout, "done");
        assert_eq!(result.status, "exit status: 0");
        assert_eq!(result.exit_code, Some(0));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn shell_exit_leaves_redirected_background_work_running() {
        let marker = std::env::temp_dir().join(format!(
            "sigmarift-background-marker-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&marker);
        let command = format!("(sleep 0.1; touch {}) >/dev/null 2>&1 &", marker.display());

        let result = execute_with_settings(&command, &ExecutionSettings::default()).unwrap();
        thread::sleep(Duration::from_millis(300));

        assert_eq!(result.status, "exit status: 0");
        assert!(marker.exists());
        std::fs::remove_file(marker).unwrap();
    }

    #[test]
    fn large_output_is_bounded_and_explains_how_to_continue() {
        let settings = ExecutionSettings::default();
        let result = execute_with_settings(
            "i=0; while [ $i -lt 20000 ]; do printf x; i=$((i + 1)); done",
            &settings,
        )
        .unwrap();
        let head_bytes = settings.max_stdout_bytes * 2 / 3;
        let tail_bytes = settings.max_stdout_bytes - head_bytes;

        assert!(result.stdout.starts_with(&"x".repeat(head_bytes)));
        assert!(result.stdout.ends_with(&"x".repeat(tail_bytes)));
        assert!(result.stdout.contains(&format!(
            "[stdout truncated: showing first {} and last {} bytes of 20000 bytes.",
            head_bytes, tail_bytes,
        )));
        assert!(result.stdout.len() < 20000);
    }

    #[test]
    fn execution_timeout_stops_the_complete_command() {
        let settings = ExecutionSettings {
            timeout_seconds: 1,
            ..ExecutionSettings::default()
        };
        let result = execute_with_settings("sleep 2; printf late", &settings).unwrap();

        assert_eq!(result.status, "timed out after 1 seconds");
        assert_eq!(result.exit_code, None);
        assert!(result.timed_out);
        assert!(result.duration_ms >= 1_000);
        assert!(!result.stdout.contains("late"));
    }

    #[test]
    fn cancellation_stops_the_complete_command() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&cancelled);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            signal.store(true, Ordering::Relaxed);
        });

        let result = execute_with_settings_and_cancel(
            "sleep 10; printf late",
            &ExecutionSettings::default(),
            &cancelled,
        )
        .unwrap();

        assert_eq!(result.status, "interrupted by user");
        assert!(!result.stdout.contains("late"));
    }

    #[test]
    fn commands_receive_eof_instead_of_tui_input() {
        let result = execute_with_settings(
            "read value || printf detached",
            &ExecutionSettings::default(),
        )
        .unwrap();

        assert_eq!(result.stdout, "detached");
    }

    #[test]
    fn already_finished_process_group_preserves_its_real_status() {
        let mut child = Command::new("sh").arg("-c").arg("exit 7").spawn().unwrap();
        assert_eq!(child.wait().unwrap().code(), Some(7));

        assert!(!kill_process_group(&mut child).unwrap());
    }
}

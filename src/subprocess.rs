//! Guarded subprocess execution.
//!
//! Every external command we shell out to (latexmk, pdftoppm, pdfinfo, synctex)
//! goes through `Guarded`. The guard:
//!
//! 1. Runs the child off the UI thread (callers pick foreground or background).
//! 2. Enforces a per-command timeout. If the child overruns, it is killed and
//!    a `[subprocess]` line is written to stderr so the user can see which
//!    binary misbehaved without us having to invent a UI surface for it.
//! 3. Drains stdout/stderr on dedicated threads so a child that writes a lot
//!    cannot deadlock waiting for its pipe buffer to drain.
//!
//! This is the only sanctioned way to spawn external processes in this crate.
//! If you find yourself reaching for `Command::output` directly, route it
//! through here instead -- otherwise a hung `pdftoppm` will freeze the editor
//! again.

use std::io::Read;
use std::process::{Child, Command, Output, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Outcome of a guarded subprocess run.
#[derive(Debug)]
pub enum GuardOutcome {
    /// Child exited 0.
    Ok(Output),
    /// Child exited non-zero.
    NonZero {
        label: String,
        code: i32,
        stdout: String,
        stderr: String,
    },
    /// Child exceeded the configured timeout and was killed.
    TimedOut {
        label: String,
        after: Duration,
        partial_stderr: String,
    },
    /// Spawn or wait failed at the OS level.
    SpawnFailed { label: String, error: String },
}

impl GuardOutcome {
    /// Pull stdout out on success, or surface the failure as an `anyhow` error
    /// with a message that names the binary, the failure mode, and what we saw
    /// on stderr -- enough for a bug report.
    pub fn into_stdout(self) -> anyhow::Result<Vec<u8>> {
        match self {
            GuardOutcome::Ok(o) => Ok(o.stdout),
            GuardOutcome::NonZero {
                label,
                code,
                stderr,
                ..
            } => Err(anyhow::anyhow!(
                "{label} exit {code}\n{}",
                tail_lines(&stderr, 20)
            )),
            GuardOutcome::TimedOut {
                label,
                after,
                partial_stderr,
            } => Err(anyhow::anyhow!(
                "{label} timed out after {after:?} and was killed\n{}",
                tail_lines(&partial_stderr, 20)
            )),
            GuardOutcome::SpawnFailed { label, error } => {
                Err(anyhow::anyhow!("spawn {label}: {error}"))
            }
        }
    }
}

/// A `Command` with a timeout and a human-readable label.
pub struct Guarded {
    label: String,
    cmd: Command,
    timeout: Duration,
}

impl Guarded {
    pub fn new(label: impl Into<String>, cmd: Command, timeout: Duration) -> Self {
        Self {
            label: label.into(),
            cmd,
            timeout,
        }
    }

    /// Run synchronously on the calling thread. **Never** call this from the
    /// UI thread; use `run_in_thread` for that.
    pub fn run(mut self) -> GuardOutcome {
        let label = self.label.clone();
        self.cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = match self.cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return GuardOutcome::SpawnFailed {
                    label,
                    error: e.to_string(),
                }
            }
        };
        let pid = child.id();

        // Drain stdio concurrently so a chatty child can't deadlock on a full
        // pipe buffer while we're sleeping in the wait loop.
        let stdout_h = child.stdout.take().map(drain);
        let stderr_h = child.stderr.take().map(drain);

        let started = Instant::now();
        let killed = wait_with_timeout(&mut child, self.timeout, &label, pid);

        // Even if killed, join the drainers -- pipes are closed when the child
        // dies so the readers will return EOF promptly.
        let stdout = stdout_h.and_then(|h| h.join().ok()).unwrap_or_default();
        let stderr = stderr_h.and_then(|h| h.join().ok()).unwrap_or_default();

        match killed {
            WaitOutcome::Exited(status) => {
                if status.success() {
                    GuardOutcome::Ok(Output {
                        status,
                        stdout,
                        stderr,
                    })
                } else {
                    GuardOutcome::NonZero {
                        label,
                        code: status.code().unwrap_or(-1),
                        stdout: String::from_utf8_lossy(&stdout).to_string(),
                        stderr: String::from_utf8_lossy(&stderr).to_string(),
                    }
                }
            }
            WaitOutcome::Killed => GuardOutcome::TimedOut {
                label,
                after: started.elapsed(),
                partial_stderr: String::from_utf8_lossy(&stderr).to_string(),
            },
            WaitOutcome::WaitErr(e) => GuardOutcome::SpawnFailed {
                label,
                error: format!("wait: {e}"),
            },
        }
    }
}

enum WaitOutcome {
    Exited(std::process::ExitStatus),
    Killed,
    WaitErr(std::io::Error),
}

fn wait_with_timeout(child: &mut Child, timeout: Duration, label: &str, pid: u32) -> WaitOutcome {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return WaitOutcome::Exited(status),
            Ok(None) => {
                if started.elapsed() >= timeout {
                    eprintln!(
                        "[subprocess] killing runaway: label={} pid={} after={:?} (timeout={:?}); \
                         report this so we can raise the timeout or fix the underlying cause",
                        label,
                        pid,
                        started.elapsed(),
                        timeout
                    );
                    let _ = child.kill();
                    let _ = child.wait();
                    return WaitOutcome::Killed;
                }
                thread::sleep(poll_interval(started.elapsed()));
            }
            Err(e) => return WaitOutcome::WaitErr(e),
        }
    }
}

/// Tight poll for the first 200ms (snappy for fast commands like pdfinfo),
/// then back off to 100ms so a long-running latexmk doesn't burn CPU on
/// `try_wait` syscalls.
fn poll_interval(elapsed: Duration) -> Duration {
    if elapsed < Duration::from_millis(200) {
        Duration::from_millis(10)
    } else {
        Duration::from_millis(100)
    }
}

fn drain<R: Read + Send + 'static>(mut r: R) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = r.read_to_end(&mut buf);
        buf
    })
}

fn tail_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() <= n {
        s.to_string()
    } else {
        lines[lines.len() - n..].join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A child that overruns the timeout must be killed and reported as
    /// `TimedOut`. This is the regression test for the freeze that motivated
    /// this module.
    #[test]
    fn timeout_kills_runaway_child() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("sleep 5");
        let started = Instant::now();
        let outcome = Guarded::new("sleep-test", cmd, Duration::from_millis(200)).run();
        let elapsed = started.elapsed();
        match outcome {
            GuardOutcome::TimedOut { label, .. } => assert_eq!(label, "sleep-test"),
            other => panic!("expected TimedOut, got {other:?}"),
        }
        assert!(
            elapsed < Duration::from_secs(1),
            "guard did not return promptly: {elapsed:?}"
        );
    }

    #[test]
    fn ok_path_returns_stdout() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("printf hello");
        let outcome = Guarded::new("echo-test", cmd, Duration::from_secs(2)).run();
        match outcome {
            GuardOutcome::Ok(o) => assert_eq!(&o.stdout, b"hello"),
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn nonzero_exit_is_reported() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("echo bad >&2; exit 7");
        let outcome = Guarded::new("fail-test", cmd, Duration::from_secs(2)).run();
        match outcome {
            GuardOutcome::NonZero { code, stderr, .. } => {
                assert_eq!(code, 7);
                assert!(stderr.contains("bad"));
            }
            other => panic!("expected NonZero, got {other:?}"),
        }
    }

    /// Children that produce more than a pipe buffer's worth of output must
    /// not deadlock (older naive implementations did exactly that).
    #[test]
    fn large_stdout_does_not_deadlock() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("yes x | head -c 200000");
        let outcome = Guarded::new("yes-test", cmd, Duration::from_secs(5)).run();
        match outcome {
            GuardOutcome::Ok(o) => assert_eq!(o.stdout.len(), 200_000),
            other => panic!("expected Ok, got {other:?}"),
        }
    }
}

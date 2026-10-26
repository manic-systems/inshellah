// SPDX-License-Identifier: EUPL-1.2
//! subprocess runner, unix-only.
//!
//! the child is its own pgid leader; on timeout killpg takes the whole tree so
//! wrapped `--help` and forked grandchildren go too. pipes are non-blocking so
//! poll-then-read never blocks on the next chunk.

use std::env;
use std::ffi::OsString;
use std::io::{self, ErrorKind, Read};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::process::CommandExt as _;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

/// drop DISPLAY-family vars so gui tools don't pop windows during `--help`.
/// cached: indexing spawns thousands of times.
pub fn safe_env_vars() -> &'static [(OsString, OsString)] {
    static CACHE: OnceLock<Vec<(OsString, OsString)>> = OnceLock::new();
    CACHE.get_or_init(|| {
        env::vars_os()
            .filter(|(key, _)| safe_env_key(&key.to_string_lossy()))
            .collect()
    })
}

fn safe_env_key(key: &str) -> bool {
    !matches!(
        key,
        "DISPLAY"
            | "WAYLAND_DISPLAY"
            | "DBUS_SESSION_BUS_ADDRESS"
            | "XAUTHORITY"
            | "MIR_SOCKET"
            | "SWAYSOCK"
            | "I3SOCK"
            | "HYPRLAND_INSTANCE_SIGNATURE"
            | "XDG_CURRENT_DESKTOP"
            | "DESKTOP_SESSION"
    )
}

pub const MAX_CAPTURE_BYTES: usize = 1024 * 1024;

/// scrape runner: stdout+stderr merged, cwd /tmp so `--help` can't read repo state.
pub fn run_cmd(args: &[String], timeout_ms: u64) -> Option<String> {
    run_cmd_inner(args, timeout_ms, false, false, |_| {})
}

pub fn run_cmd_with<F>(args: &[String], timeout_ms: u64, customize: F) -> Option<String>
where
    F: FnOnce(&mut Command),
{
    run_cmd_inner(args, timeout_ms, false, false, customize)
}

/// dynamic runner: stderr dropped (parsers want clean stdout), cwd inherited
/// so `git remote`/`jj log` see the user's repo.
pub fn run_quiet(args: &[String], timeout_ms: u64) -> Option<String> {
    run_cmd_inner(args, timeout_ms, true, true, |_| {})
}

pub fn run_quiet_with<F>(args: &[String], timeout_ms: u64, customize: F) -> Option<String>
where
    F: FnOnce(&mut Command),
{
    run_cmd_inner(args, timeout_ms, true, true, customize)
}

enum Drained {
    Pending,
    Closed,
    Capped,
}

fn drain(reader: &mut dyn Read, chunk: &mut [u8], buf: &mut Vec<u8>) -> Drained {
    loop {
        match reader.read(chunk) {
            Ok(0) => return Drained::Closed,
            Ok(read) => {
                buf.extend_from_slice(&chunk[..read]);
                if buf.len() >= MAX_CAPTURE_BYTES {
                    return Drained::Capped;
                }
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => return Drained::Pending,
            Err(_) => return Drained::Closed,
        }
    }
}

fn set_nonblocking(fd: RawFd) {
    // SAFETY: fd is an open pipe owned by the caller's ChildStdout/ChildStderr.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    // SAFETY: same fd, F_SETFL only changes its status flags.
    unsafe {
        libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
}

#[expect(
    clippy::let_underscore_must_use,
    reason = "reaping the child is best-effort, the captured output is already decided"
)]
fn run_cmd_inner<F>(
    args: &[String],
    timeout_ms: u64,
    discard_stderr: bool,
    inherit_cwd: bool,
    customize: F,
) -> Option<String>
where
    F: FnOnce(&mut Command),
{
    if args.is_empty() {
        return None;
    }
    let mut cmd = Command::new(&args[0]);
    cmd.args(&args[1..]);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    if discard_stderr {
        cmd.stderr(Stdio::null());
    } else {
        cmd.stderr(Stdio::piped());
    }
    cmd.env_clear();
    for (key, value) in safe_env_vars() {
        cmd.env(key, value);
    }
    if !inherit_cwd {
        cmd.current_dir("/tmp");
    }
    cmd.process_group(0);
    customize(&mut cmd);

    let mut child = cmd.spawn().ok()?;
    let pgid = child.id().cast_signed();
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take();
    let stdout_fd = stdout.as_raw_fd();
    let stderr_fd = stderr.as_ref().map(AsRawFd::as_raw_fd);

    set_nonblocking(stdout_fd);
    if let Some(fd) = stderr_fd {
        set_nonblocking(fd);
    }

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    let mut stdout_open = true;
    let mut stderr_open = stderr_fd.is_some();
    let mut timed_out = false;
    let mut capped = false;

    'capture: while stdout_open || stderr_open {
        let now = Instant::now();
        if now >= deadline {
            timed_out = true;
            break;
        }
        let remaining_ms = i32::try_from((deadline - now).as_millis()).unwrap_or(i32::MAX);

        let mut fds: [libc::pollfd; 2] = [
            libc::pollfd {
                fd: if stdout_open { stdout_fd } else { -1 },
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: if stderr_open {
                    stderr_fd.unwrap_or(-1)
                } else {
                    -1
                },
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: fds is a live array of exactly 2 pollfd entries.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), 2, remaining_ms) };
        if ready < 0 {
            if io::Error::last_os_error().kind() == ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if ready == 0 {
            continue;
        }

        for (index, pfd) in fds.iter().enumerate() {
            if pfd.revents == 0 {
                continue;
            }
            let (reader, open): (&mut dyn Read, &mut bool) = if index == 0 {
                (&mut stdout, &mut stdout_open)
            } else {
                match stderr.as_mut() {
                    Some(err_pipe) => (err_pipe, &mut stderr_open),
                    None => continue,
                }
            };
            match drain(reader, &mut chunk, &mut buf) {
                Drained::Pending => {}
                Drained::Closed => *open = false,
                Drained::Capped => {
                    capped = true;
                    break 'capture;
                }
            }
            if pfd.revents & (libc::POLLHUP | libc::POLLERR) != 0 {
                *open = false;
            }
        }
    }

    // a child can close both pipes and keep running, so the deadline still
    // applies after eof.
    while !timed_out && !capped && matches!(child.try_wait(), Ok(None)) {
        if Instant::now() >= deadline {
            timed_out = true;
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }

    if timed_out || capped {
        // SAFETY: killpg takes plain integers, pgid is the child's own group.
        unsafe {
            libc::killpg(pgid, libc::SIGKILL);
        }
    }
    let _: io::Result<ExitStatus> = child.wait();

    if timed_out || capped || buf.is_empty() {
        None
    } else {
        Some(String::from_utf8_lossy(&buf).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_cmd_rejects_capped_output() {
        let out = run_cmd(
            &[
                "head".into(),
                "-c".into(),
                "5000000".into(),
                "/dev/zero".into(),
            ],
            2000,
        );
        assert!(out.is_none(), "capped output must not look parseable");
    }

    #[test]
    fn run_cmd_rejects_timed_out_output() {
        let out = run_cmd(
            &[
                "sh".into(),
                "-c".into(),
                "printf 'Usage: demo\\nOptions:\\n  --partial partial\\n'; sleep 1".into(),
            ],
            20,
        );
        assert!(out.is_none(), "timed-out output must not look parseable");
    }
}

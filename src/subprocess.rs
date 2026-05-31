// SPDX-License-Identifier: EUPL-1.2
//! subprocess runner. unix-only.
//!
//! child is its own pgid leader: on timeout we killpg the whole tree so
//! wrapper scripts and forked grandchildren go too. without that, wrapped
//! `--help` invocations leak. pipes are non-blocking so poll-then-read can
//! drain without ever blocking on the next chunk.

use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// drops DISPLAY-family vars so gui tools don't pop windows during
/// `--help` scrapes. cached: `vars_os` is per-call O(env) and we spawn
/// thousands of times during indexing.
pub fn safe_env_vars() -> &'static [(std::ffi::OsString, std::ffi::OsString)] {
    static CACHE: std::sync::OnceLock<Vec<(std::ffi::OsString, std::ffi::OsString)>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| {
        std::env::vars_os()
            .filter(|(k, _)| {
                let s = k.to_string_lossy();
                !(s == "DISPLAY"
                    || s == "WAYLAND_DISPLAY"
                    || s == "DBUS_SESSION_BUS_ADDRESS"
                    || s == "XAUTHORITY")
            })
            .collect()
    })
}

pub const MAX_CAPTURE_BYTES: usize = 1024 * 1024;

/// scrape-path runner: stdout+stderr merged, cwd forced to /tmp so
/// `--help` invocations can't read repo state.
pub fn run_cmd(args: &[String], timeout_ms: u64) -> Option<String> {
    run_cmd_inner(args, timeout_ms, false, false, |_| {})
}

pub fn run_cmd_with(
    args: &[String],
    timeout_ms: u64,
    customize: impl FnOnce(&mut Command),
) -> Option<String> {
    run_cmd_inner(args, timeout_ms, false, false, customize)
}

/// dynamic-completer runner: stderr dropped (parsers want clean stdout
/// lines), cwd inherited so `git remote` / `jj log` see the user's repo.
pub fn run_quiet(args: &[String], timeout_ms: u64) -> Option<String> {
    run_cmd_inner(args, timeout_ms, true, true, |_| {})
}

pub fn run_quiet_with(
    args: &[String],
    timeout_ms: u64,
    customize: impl FnOnce(&mut Command),
) -> Option<String> {
    run_cmd_inner(args, timeout_ms, true, true, customize)
}

fn run_cmd_inner(
    args: &[String],
    timeout_ms: u64,
    discard_stderr: bool,
    inherit_cwd: bool,
    customize: impl FnOnce(&mut Command),
) -> Option<String> {
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
    for (k, v) in safe_env_vars() {
        cmd.env(k, v);
    }
    if !inherit_cwd {
        cmd.current_dir("/tmp");
    }
    cmd.process_group(0);
    customize(&mut cmd);

    let mut child = cmd.spawn().ok()?;
    let pgid = child.id() as i32;
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take();
    let stdout_fd = stdout.as_raw_fd();
    let stderr_fd = stderr.as_ref().map(|s| s.as_raw_fd());

    unsafe {
        let flags = libc::fcntl(stdout_fd, libc::F_GETFL);
        libc::fcntl(stdout_fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        if let Some(fd) = stderr_fd {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
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
        let remaining_ms = (deadline - now).as_millis().min(i32::MAX as u128) as i32;

        let mut fds = [
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
        let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, remaining_ms) };
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if n == 0 {
            continue;
        }

        for (i, pfd) in fds.iter().enumerate() {
            if pfd.revents == 0 {
                continue;
            }
            let (reader, open): (&mut dyn Read, &mut bool) = if i == 0 {
                (&mut stdout as &mut dyn Read, &mut stdout_open)
            } else {
                match stderr.as_mut() {
                    Some(s) => (s as &mut dyn Read, &mut stderr_open),
                    None => continue,
                }
            };
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => {
                        *open = false;
                        break;
                    }
                    Ok(read) => {
                        buf.extend_from_slice(&chunk[..read]);
                        if buf.len() >= MAX_CAPTURE_BYTES {
                            capped = true;
                            break 'capture;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(_) => {
                        *open = false;
                        break;
                    }
                }
            }
            if pfd.revents & (libc::POLLHUP | libc::POLLERR) != 0 {
                *open = false;
            }
        }
    }

    if timed_out || capped {
        unsafe {
            libc::killpg(pgid, libc::SIGKILL);
        }
    }
    let _ = child.wait();

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

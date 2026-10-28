// SPDX-License-Identifier: EUPL-1.2
//! subprocess runner, unix-only.
//!
//! the child leads its own process group, so on timeout killpg takes the whole
//! tree and wrapped `--help` and forked grandchildren go too. live spawns also
//! lead a new session, so they have no controlling tty for ssh to prompt on.
//! the bulk scrape skips setsid because a `pre_exec` hook makes std fork the
//! whole indexer instead of using `posix_spawn`. pipes are non-blocking so
//! poll-then-read never blocks on the next chunk.

use std::{
    env,
    ffi::OsString,
    io::{
        self,
        ErrorKind,
        Read,
    },
    os::{
        fd::{
            AsRawFd,
            RawFd,
        },
        unix::process::CommandExt as _,
    },
    path::{
        Path,
        PathBuf,
    },
    process::{
        Child,
        Command,
        Stdio,
    },
    sync::OnceLock,
    thread,
    time::{
        Duration,
        Instant,
    },
};

use crate::executable::find_in_path;

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

const SCRAPE_CWD: &str = "/tmp";

/// a scrape cwd that is gone makes every spawn fail like a missing binary.
///
/// # Errors
///
/// fails when the scrape cwd is not a directory.
pub fn check_scrape_cwd() -> io::Result<()> {
    if Path::new(SCRAPE_CWD).is_dir() {
        return Ok(());
    }
    Err(io::Error::new(
        ErrorKind::NotFound,
        format!("scrape working directory {SCRAPE_CWD} is not a directory"),
    ))
}

/// how a captured run ended.
pub enum Run {
    /// exited zero, with what it printed.
    Succeeded(String),
    /// exited nonzero, with what it printed.
    Errored(String),
    /// no process to run, or killed at the deadline or the capture cap.
    Failed,
    /// the binary itself cannot run, as with a missing interpreter or an
    /// executable for another platform. transient spawn errors are `Failed`.
    Unspawned(io::Error),
}

impl Run {
    pub fn printed(self) -> Option<String> {
        match self {
            Self::Succeeded(out) | Self::Errored(out) => (!out.is_empty()).then_some(out),
            Self::Failed | Self::Unspawned(_) => None,
        }
    }
}

/// scrape runner: stdout+stderr merged, cwd /tmp so `--help` can't read repo
/// state.
pub fn run_cmd(args: &[String], timeout_ms: u64) -> Option<String> {
    run_cmd_inner(args, timeout_ms, false, false, Leader::Group, |_| {}).printed()
}

/// `run_cmd` that keeps how the run ended.
pub fn run_cmd_ended(args: &[String], timeout_ms: u64) -> Run {
    run_cmd_inner(args, timeout_ms, false, false, Leader::Group, |_| {})
}

/// scrape runner for live completion, which gets a new session like
/// `run_quiet`.
pub fn run_cmd_live(args: &[String], timeout_ms: u64) -> Option<String> {
    run_cmd_inner(args, timeout_ms, false, false, Leader::Session, |_| {}).printed()
}

pub fn run_cmd_with<F>(args: &[String], timeout_ms: u64, customize: F) -> Option<String>
where
    F: FnOnce(&mut Command),
{
    run_cmd_inner(args, timeout_ms, false, false, Leader::Group, customize).printed()
}

/// dynamic runner: stderr dropped (parsers want clean stdout), cwd inherited
/// so `git remote`/`jj log` see the user's repo.
pub fn run_quiet(args: &[String], timeout_ms: u64) -> Option<String> {
    run_cmd_inner(args, timeout_ms, true, true, Leader::Session, |_| {}).printed()
}

pub fn run_quiet_with<F>(args: &[String], timeout_ms: u64, customize: F) -> Option<String>
where
    F: FnOnce(&mut Command),
{
    run_cmd_inner(args, timeout_ms, true, true, Leader::Session, customize).printed()
}

/// `run_quiet_with` that keeps the exit status and a silent exit.
pub fn run_quiet_ended<F>(args: &[String], timeout_ms: u64, customize: F) -> Run
where
    F: FnOnce(&mut Command),
{
    run_cmd_inner(args, timeout_ms, true, true, Leader::Session, customize)
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
            },
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

fn setsid() -> io::Result<()> {
    // SAFETY: setsid takes no arguments and touches no memory.
    if unsafe { libc::setsid() } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Leader {
    Session,
    Group,
}

fn lead(cmd: &mut Command, leader: Leader) {
    match leader {
        Leader::Session => {
            // SAFETY: setsid is async-signal-safe, so it may run between fork
            // and exec.
            unsafe {
                cmd.pre_exec(setsid);
            }
        },
        Leader::Group => {
            cmd.process_group(0);
        },
    }
}

/// a bare name resolves through the shared lookup, never execvp's own search,
/// which would honour empty and relative `PATH` entries against the child cwd.
fn program_path(arg0: &str) -> Option<PathBuf> {
    if arg0.contains('/') {
        Some(PathBuf::from(arg0))
    } else {
        find_in_path(arg0)
    }
}

fn run_cmd_inner<F>(
    args: &[String],
    timeout_ms: u64,
    discard_stderr: bool,
    inherit_cwd: bool,
    leader: Leader,
    customize: F,
) -> Run
where
    F: FnOnce(&mut Command),
{
    let Some(name) = args.first() else {
        return Run::Failed;
    };
    let Some(program) = program_path(name) else {
        return Run::Unspawned(io::Error::from(ErrorKind::NotFound));
    };
    let mut cmd = Command::new(program);
    cmd.arg0(&args[0]);
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
    cmd.env("SSH_ASKPASS_REQUIRE", "never");
    if !inherit_cwd {
        cmd.current_dir(SCRAPE_CWD);
    }
    lead(&mut cmd, leader);
    customize(&mut cmd);

    match cmd.spawn() {
        Ok(child) => capture(child, timeout_ms),
        Err(err) if binary_cannot_run(&err) => Run::Unspawned(err),
        Err(_) => Run::Failed,
    }
}

fn binary_cannot_run(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(
            libc::ENOENT
                | libc::ENOTDIR
                | libc::EACCES
                | libc::ENOEXEC
                | libc::ELOOP
                | libc::ENAMETOOLONG
                | libc::EISDIR
        )
    )
}

fn capture(mut child: Child, timeout_ms: u64) -> Run {
    let pgid = child.id().cast_signed();
    let Some(mut stdout) = child.stdout.take() else {
        return Run::Failed;
    };
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
                fd:      if stdout_open { stdout_fd } else { -1 },
                events:  libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd:      if stderr_open {
                    stderr_fd.unwrap_or(-1)
                } else {
                    -1
                },
                events:  libc::POLLIN,
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
                Drained::Pending => {},
                Drained::Closed => *open = false,
                Drained::Capped => {
                    capped = true;
                    break 'capture;
                },
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
    let out = String::from_utf8_lossy(&buf).into_owned();
    match child.wait() {
        _ if timed_out || capped => Run::Failed,
        Ok(status) if status.success() => Run::Succeeded(out),
        Ok(_) | Err(_) => Run::Errored(out),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    fn pid_and_stat_field(
        run: fn(&[String], u64) -> Option<String>,
        field: usize,
    ) -> (String, String) {
        let out = run(
            &[
                "sh".into(),
                "-c".into(),
                "printf '%s ' $$; cat /proc/$$/stat".into(),
            ],
            2000,
        )
        .expect("child output");
        let (pid, stat) = out.split_once(' ').expect("pid then stat");
        let value = stat
            .rsplit_once(')')
            .and_then(|(_, fields)| fields.split_whitespace().nth(field))
            .expect("stat field");
        (pid.to_owned(), value.to_owned())
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn live_children_lead_their_own_session() {
        for run in [run_quiet, run_cmd_live] {
            let (pid, session) = pid_and_stat_field(run, 3);
            assert_eq!(session, pid);
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn scrape_children_lead_their_own_group() {
        let (pid, group) = pid_and_stat_field(run_cmd, 2);
        assert_eq!(group, pid);
    }

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

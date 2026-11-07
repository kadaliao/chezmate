//! Running external programs with captured output and an optional time limit.

use std::env;
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

/// Output of a finished process.
#[derive(Debug, Clone)]
pub struct Captured {
    pub status: ExitStatus,
    /// Standard output decoded as UTF-8 (invalid sequences replaced).
    pub stdout: String,
    /// Standard output exactly as the program wrote it.
    pub stdout_bytes: Vec<u8>,
    pub stderr: String,
}

impl Captured {
    pub fn success(&self) -> bool {
        self.status.success()
    }
}

/// Directories where package managers commonly install chezmoi, git and gpg.
///
/// A desktop app launched from Finder or a desktop launcher gets a minimal
/// `PATH` (`/usr/bin:/bin:...`), so chezmoi would not find gpg, git or a
/// password manager CLI. These are appended to `PATH` for child processes.
fn fallback_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join("bin"));
        dirs.push(home.join("go/bin"));
    }
    for dir in [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/home/linuxbrew/.linuxbrew/bin",
        "/snap/bin",
        "/usr/bin",
        "/bin",
    ] {
        dirs.push(PathBuf::from(dir));
    }
    dirs
}

/// The current `PATH` followed by any fallback directory it lacks.
pub fn search_path() -> OsString {
    let mut dirs: Vec<PathBuf> = env::var_os("PATH")
        .map(|path| env::split_paths(&path).collect())
        .unwrap_or_default();
    for dir in fallback_dirs() {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    env::join_paths(dirs).unwrap_or_default()
}

/// Finds an executable by name in `path`.
pub fn find_executable(name: &str, path: &OsStr) -> Option<PathBuf> {
    let candidates: Vec<String> = if cfg!(windows) {
        vec![format!("{name}.exe"), name.to_string()]
    } else {
        vec![name.to_string()]
    };
    env::split_paths(path).find_map(|dir| {
        candidates
            .iter()
            .map(|file| dir.join(file))
            .find(|candidate| is_executable(candidate))
    })
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = path.metadata() else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// A human-readable rendering of a command line for messages.
pub fn display_command(program: &str, args: &[OsString]) -> String {
    let mut text = program.to_string();
    for arg in args {
        text.push(' ');
        let arg = arg.to_string_lossy();
        if arg.contains(char::is_whitespace) {
            text.push_str(&format!("{arg:?}"));
        } else {
            text.push_str(&arg);
        }
    }
    text
}

/// Runs `command` to completion with stdin closed and both output streams
/// captured. With a `timeout`, the process (and on Unix its whole process
/// group, so helpers such as password-manager CLIs do not linger) is killed
/// when the limit passes.
pub fn run(mut command: Command, display: &str, timeout: Option<Duration>) -> Result<Captured> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    if timeout.is_some() {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    let mut child = command.spawn().map_err(|source| Error::Spawn {
        command: display.to_string(),
        source,
    })?;

    let stdout = child.stdout.take().map(read_in_background);
    let stderr = child.stderr.take().map(read_in_background);

    let status = match timeout {
        None => child.wait(),
        Some(limit) => {
            let started = Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => break Ok(status),
                    Ok(None) if started.elapsed() >= limit => {
                        kill_tree(&mut child);
                        return Err(Error::Timeout {
                            command: display.to_string(),
                            seconds: limit.as_secs(),
                        });
                    }
                    Ok(None) => thread::sleep(Duration::from_millis(15)),
                    Err(err) => break Err(err),
                }
            }
        }
    }
    .map_err(|source| Error::Io {
        context: format!("waiting for `{display}`"),
        source,
    })?;

    let collect = |handle: Option<thread::JoinHandle<Vec<u8>>>| {
        handle
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default()
    };
    let stdout_bytes = collect(stdout);
    Ok(Captured {
        status,
        stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
        stdout_bytes,
        stderr: String::from_utf8_lossy(&collect(stderr)).into_owned(),
    })
}

fn read_in_background<R: Read + Send + 'static>(mut reader: R) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = reader.read_to_end(&mut buffer);
        buffer
    })
}

fn kill_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        // The child leads its own process group (see `process_group(0)`).
        let pid = child.id() as libc::pid_t;
        // SAFETY: killpg only sends a signal; an invalid group is reported via errno.
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_quotes_arguments_with_spaces() {
        let args = vec![OsString::from("status"), OsString::from("/a b/c")];
        assert_eq!(
            display_command("chezmoi", &args),
            "chezmoi status \"/a b/c\""
        );
    }

    #[cfg(unix)]
    #[test]
    fn timeout_stops_the_process_group() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 30 & wait"]);
        let started = Instant::now();
        let err = run(command, "sh", Some(Duration::from_millis(300))).unwrap_err();
        assert_eq!(err.kind(), "timeout");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn captures_both_streams_and_status() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "echo out; echo err >&2; exit 3"]);
        let captured = run(command, "sh", None).unwrap();
        assert_eq!(captured.stdout, "out\n");
        assert_eq!(captured.stderr, "err\n");
        assert_eq!(captured.status.code(), Some(3));
    }
}

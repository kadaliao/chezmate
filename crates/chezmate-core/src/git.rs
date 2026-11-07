//! The git repository behind the chezmoi source directory.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::process::{self, Captured};

const LOCAL_TIMEOUT: Duration = Duration::from_secs(60);
/// Network operations fail instead of prompting (`GIT_TERMINAL_PROMPT=0`),
/// so this only guards against a stalled connection.
const NETWORK_TIMEOUT: Duration = Duration::from_secs(180);
/// Git's well-known empty tree, the diff base before the first commit.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
/// Untracked files get a diff each; beyond this only their names are listed.
const MAX_UNTRACKED_DIFFS: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitFileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
    Conflicted,
}

/// One uncommitted change in the source repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitChange {
    /// Path relative to the repository root.
    pub path: String,
    /// The previous path of a rename or copy.
    pub original_path: Option<String>,
    pub status: GitFileStatus,
    /// The change is staged in the index.
    pub staged: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RepoStatus {
    /// `None` when HEAD is detached.
    pub branch: Option<String>,
    /// `None` before the first commit.
    pub head: Option<String>,
    pub upstream: Option<String>,
    /// Commits on the branch that the upstream lacks.
    pub ahead: u32,
    /// Commits on the upstream that the branch lacks.
    pub behind: u32,
    pub changes: Vec<GitChange>,
}

impl RepoStatus {
    pub fn is_clean(&self) -> bool {
        self.changes.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Commit {
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    /// ISO 8601 author date.
    pub date: String,
    pub subject: String,
}

/// Runs git in the chezmoi working tree.
#[derive(Debug, Clone)]
pub struct Git {
    binary: PathBuf,
    repo: PathBuf,
    search_path: OsString,
    env: Vec<(OsString, OsString)>,
}

impl Git {
    pub fn new(
        repo: impl Into<PathBuf>,
        search_path: OsString,
        env: Vec<(OsString, OsString)>,
    ) -> Result<Self> {
        let binary =
            process::find_executable("git", &search_path).ok_or_else(|| Error::GitNotFound {
                searched: search_path.to_string_lossy().into_owned(),
            })?;
        Ok(Self {
            binary,
            repo: repo.into(),
            search_path,
            env,
        })
    }

    pub fn repo(&self) -> &Path {
        &self.repo
    }

    /// Whether the working tree is a git repository.
    pub fn is_repository(&self) -> bool {
        self.exec(&["rev-parse", "--git-dir"], LOCAL_TIMEOUT)
            .map(|c| c.success())
            .unwrap_or(false)
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(&self.binary);
        command
            .arg("-C")
            .arg(&self.repo)
            // Report non-ASCII paths verbatim instead of octal escapes.
            .args(["-c", "core.quotepath=false"])
            .args(args)
            .env("PATH", &self.search_path)
            .env("GIT_TERMINAL_PROMPT", "0")
            .envs(self.env.iter().map(|(k, v)| (k, v)));
        command
    }

    fn exec(&self, args: &[&str], timeout: Duration) -> Result<Captured> {
        let display =
            process::display_command("git", &args.iter().map(OsString::from).collect::<Vec<_>>());
        process::run(self.command(args), &display, Some(timeout))
    }

    fn run(&self, args: &[&str], timeout: Duration) -> Result<String> {
        let captured = self.exec(args, timeout)?;
        if captured.success() {
            Ok(captured.stdout)
        } else {
            Err(failed(args, &captured))
        }
    }

    fn head(&self) -> Result<Option<String>> {
        let captured = self.exec(&["rev-parse", "--verify", "-q", "HEAD"], LOCAL_TIMEOUT)?;
        Ok(captured
            .success()
            .then(|| captured.stdout.trim().to_string()))
    }

    pub fn status(&self) -> Result<RepoStatus> {
        let out = self.run(
            &[
                "status",
                "--porcelain=v2",
                "--branch",
                "-z",
                "--untracked-files=all",
            ],
            LOCAL_TIMEOUT,
        )?;
        parse_status_v2(&out)
    }

    /// The most recent commits on HEAD, newest first. Empty before the
    /// first commit.
    pub fn log(&self, limit: usize) -> Result<Vec<Commit>> {
        if self.head()?.is_none() {
            return Ok(Vec::new());
        }
        let limit = limit.to_string();
        let out = self.run(
            &[
                "log",
                "-n",
                &limit,
                "--format=%H%x1f%h%x1f%an%x1f%aI%x1f%s%x1e",
            ],
            LOCAL_TIMEOUT,
        )?;
        Ok(parse_log(&out))
    }

    /// Uncommitted changes (staged, unstaged and untracked) as a Git diff.
    pub fn diff(&self) -> Result<String> {
        let base = self.head()?.unwrap_or_else(|| EMPTY_TREE.to_string());
        let mut patch = self.run(
            &["diff", "--no-color", "--no-ext-diff", &base],
            LOCAL_TIMEOUT,
        )?;
        let status = self.status()?;
        let untracked = status
            .changes
            .iter()
            .filter(|c| c.status == GitFileStatus::Untracked)
            .take(MAX_UNTRACKED_DIFFS);
        for change in untracked {
            // `--no-index` exits 1 when the files differ, which they always do.
            let captured = self.exec(
                &[
                    "diff",
                    "--no-color",
                    "--no-ext-diff",
                    "--no-index",
                    "--",
                    "/dev/null",
                    &change.path,
                ],
                LOCAL_TIMEOUT,
            )?;
            if captured.status.code() == Some(1) || captured.success() {
                patch.push_str(&captured.stdout);
            } else {
                return Err(failed(&["diff", "--no-index", &change.path], &captured));
            }
        }
        Ok(patch)
    }

    /// Stages everything and commits it. Returns the new commit hash.
    pub fn commit_all(&self, message: &str) -> Result<String> {
        if message.trim().is_empty() {
            return Err(Error::InvalidRequest("commit message is empty".into()));
        }
        if self.status()?.is_clean() {
            return Err(Error::InvalidRequest("nothing to commit".into()));
        }
        self.run(&["add", "--all"], LOCAL_TIMEOUT)?;
        self.run(&["commit", "--quiet", "-m", message], LOCAL_TIMEOUT)?;
        Ok(self.head()?.unwrap_or_default())
    }

    pub fn fetch(&self) -> Result<()> {
        self.run(&["fetch", "--quiet"], NETWORK_TIMEOUT).map(drop)
    }

    /// Pulls with `--rebase --autostash`, like `chezmoi update`, but never
    /// leaves a conflict behind: if the pulled commits conflict with local
    /// commits the rebase is aborted, and if they conflict with uncommitted
    /// changes the pull is undone and the changes restored. Either way the
    /// repository is back where it started and [`is_pull_conflict`] is true
    /// for the error.
    pub fn pull(&self) -> Result<()> {
        let before = self.head()?;
        let args = ["pull", "--rebase", "--autostash", "--quiet"];
        let captured = self.exec(&args, NETWORK_TIMEOUT)?;
        if captured.success() {
            // git reports success when the rebase worked but reapplying the
            // stashed changes conflicted, leaving conflict markers behind.
            let conflicted = self
                .status()?
                .changes
                .iter()
                .any(|c| c.status == GitFileStatus::Conflicted);
            if !conflicted {
                return Ok(());
            }
            let undone = match &before {
                Some(head) => self
                    .run(&["reset", "--quiet", "--hard", head], LOCAL_TIMEOUT)
                    .and_then(|_| self.run(&["stash", "pop", "--quiet"], LOCAL_TIMEOUT))
                    .is_ok(),
                None => false,
            };
            return Err(Error::Failed {
                command: format!("git {}", args.join(" ")),
                code: captured.status.code(),
                stderr: if undone {
                    format!(
                        "{CONFLICT_NOTE} with uncommitted changes. Chezmate undid the pull; the repository is unchanged. Commit or discard those changes, then pull again."
                    )
                } else {
                    format!(
                        "{CONFLICT_NOTE} with uncommitted changes, and Chezmate could not undo it; the changes are in `git stash list`. Resolve it in a terminal."
                    )
                },
            });
        }
        let mut err = failed(&args, &captured);
        if self.rebase_in_progress() {
            let aborted = self.exec(&["rebase", "--abort"], LOCAL_TIMEOUT)?;
            if let Error::Failed { stderr, .. } = &mut err {
                stderr.push('\n');
                stderr.push_str(CONFLICT_NOTE);
                stderr.push_str(if aborted.success() {
                    " with local commits. Chezmate aborted the rebase; the repository is unchanged."
                } else {
                    " with local commits and `git rebase --abort` failed; resolve it in a terminal."
                });
            }
        }
        Err(err)
    }

    fn rebase_in_progress(&self) -> bool {
        ["rebase-merge", "rebase-apply"].iter().any(|dir| {
            self.exec(&["rev-parse", "--git-path", dir], LOCAL_TIMEOUT)
                .ok()
                .filter(Captured::success)
                .map(|c| self.repo.join(c.stdout.trim()).exists())
                .unwrap_or(false)
        })
    }

    pub fn push(&self) -> Result<()> {
        self.run(&["push", "--quiet"], NETWORK_TIMEOUT).map(drop)
    }
}

const CONFLICT_NOTE: &str = "The pull conflicted";

/// The error is [`Git::pull`] refusing a pull that conflicts with changes
/// on this computer.
pub fn is_pull_conflict(err: &Error) -> bool {
    matches!(err, Error::Failed { command, stderr, .. }
        if command.starts_with("git pull") && stderr.contains(CONFLICT_NOTE))
}

fn failed(args: &[&str], captured: &Captured) -> Error {
    Error::Failed {
        command: format!("git {}", args.join(" ")),
        code: captured.status.code(),
        stderr: if captured.stderr.trim().is_empty() {
            captured.stdout.clone()
        } else {
            captured.stderr.clone()
        },
    }
}

fn file_status(code: u8) -> Option<GitFileStatus> {
    Some(match code {
        b'M' => GitFileStatus::Modified,
        b'A' => GitFileStatus::Added,
        b'D' => GitFileStatus::Deleted,
        b'R' => GitFileStatus::Renamed,
        b'C' => GitFileStatus::Copied,
        b'T' => GitFileStatus::TypeChanged,
        b'U' => GitFileStatus::Conflicted,
        _ => return None,
    })
}

/// Parses `git status --porcelain=v2 --branch -z`.
pub fn parse_status_v2(output: &str) -> Result<RepoStatus> {
    let parse_error = |record: &str| Error::Parse {
        what: "git status",
        message: format!("{record:?}"),
    };
    let mut status = RepoStatus::default();
    let mut records = output.split('\0').filter(|r| !r.is_empty());
    while let Some(record) = records.next() {
        if let Some(header) = record.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" if value != "(initial)" => status.head = Some(value.into()),
                "branch.head" if value != "(detached)" => status.branch = Some(value.into()),
                "branch.upstream" => status.upstream = Some(value.into()),
                "branch.ab" => {
                    for part in value.split_whitespace() {
                        if let Some(n) = part.strip_prefix('+') {
                            status.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            status.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let kind = record.as_bytes()[0];
        let change = match kind {
            b'?' => GitChange {
                path: record.get(2..).ok_or_else(|| parse_error(record))?.into(),
                original_path: None,
                status: GitFileStatus::Untracked,
                staged: false,
            },
            b'!' => continue,
            b'1' | b'2' | b'u' => {
                let fields = match kind {
                    b'1' => 9,
                    b'2' => 10,
                    _ => 11,
                };
                let parts: Vec<&str> = record.splitn(fields, ' ').collect();
                if parts.len() != fields {
                    return Err(parse_error(record));
                }
                let xy = parts[1].as_bytes();
                if xy.len() != 2 {
                    return Err(parse_error(record));
                }
                let (index, worktree) = (xy[0], xy[1]);
                let original_path = if kind == b'2' {
                    Some(
                        records
                            .next()
                            .ok_or_else(|| parse_error(record))?
                            .to_string(),
                    )
                } else {
                    None
                };
                let status = if kind == b'u' {
                    GitFileStatus::Conflicted
                } else {
                    file_status(worktree)
                        .or_else(|| file_status(index))
                        .ok_or_else(|| parse_error(record))?
                };
                GitChange {
                    path: parts[fields - 1].into(),
                    original_path,
                    status,
                    staged: kind != b'u' && index != b'.',
                }
            }
            _ => return Err(parse_error(record)),
        };
        status.changes.push(change);
    }
    Ok(status)
}

fn parse_log(output: &str) -> Vec<Commit> {
    output
        .split('\x1e')
        .filter_map(|record| {
            let record = record.trim_start_matches('\n');
            let fields: Vec<&str> = record.split('\x1f').collect();
            let [hash, short_hash, author, date, subject] = fields[..] else {
                return None;
            };
            Some(Commit {
                hash: hash.into(),
                short_hash: short_hash.into(),
                author: author.into(),
                date: date.into(),
                subject: subject.trim_end().into(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_branch_headers_and_entries() {
        let out = "# branch.oid 1234abcd\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
                   1 .M N... 100644 100644 100644 aaa bbb dot_zshrc\0\
                   1 A. N... 000000 100644 100644 000 ccc dot_config/my app/rc\0\
                   2 R. N... 100644 100644 100644 ddd eee R100 dot_new\0dot_old\0\
                   ? run_once_setup.sh\0";
        let status = parse_status_v2(out).unwrap();
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.upstream.as_deref(), Some("origin/main"));
        assert_eq!((status.ahead, status.behind), (2, 1));
        assert_eq!(status.changes.len(), 4);
        assert_eq!(status.changes[0].status, GitFileStatus::Modified);
        assert!(!status.changes[0].staged);
        assert_eq!(status.changes[1].path, "dot_config/my app/rc");
        assert!(status.changes[1].staged);
        assert_eq!(status.changes[2].status, GitFileStatus::Renamed);
        assert_eq!(status.changes[2].original_path.as_deref(), Some("dot_old"));
        assert_eq!(status.changes[3].status, GitFileStatus::Untracked);
    }

    #[test]
    fn parses_initial_and_detached_heads() {
        let status = parse_status_v2("# branch.oid (initial)\0# branch.head main\0").unwrap();
        assert_eq!(status.head, None);
        assert_eq!(status.branch.as_deref(), Some("main"));
        let status = parse_status_v2("# branch.oid abc\0# branch.head (detached)\0").unwrap();
        assert_eq!(status.branch, None);
    }

    #[test]
    fn parses_log_records() {
        let out = "aaa\x1fa\x1fKada\x1f2026-11-07T06:00:00+08:00\x1fAdd zshrc\x1e\nbbb\x1fb\x1fKada\x1f2026-11-06T06:00:00+08:00\x1fInit\x1e\n";
        let commits = parse_log(out);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].subject, "Add zshrc");
        assert_eq!(commits[1].hash, "bbb");
    }
}

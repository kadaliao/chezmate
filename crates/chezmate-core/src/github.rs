//! The GitHub CLI (`gh`), when it is installed and logged in: making a
//! private repository for the shared version in one step.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::process::{self, Captured};

/// `gh` talks to GitHub for most commands.
const TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct GitHub {
    binary: PathBuf,
    search_path: OsString,
    env: Vec<(OsString, OsString)>,
}

impl GitHub {
    /// `gh`, if it is installed. Looked up in the `PATH` from `env` when it
    /// sets one (as chezmoi's own child processes see it), otherwise in
    /// `search_path`.
    pub fn find(search_path: &OsString, env: &[(OsString, OsString)]) -> Option<Self> {
        let search_path = env
            .iter()
            .rev()
            .find(|(key, _)| key == "PATH")
            .map_or(search_path, |(_, path)| path);
        let binary = process::find_executable("gh", search_path)?;
        Some(Self {
            binary,
            search_path: search_path.clone(),
            env: env.to_vec(),
        })
    }

    fn exec(&self, args: &[&str]) -> Result<Captured> {
        let mut command = Command::new(&self.binary);
        command
            .args(args)
            .env("PATH", &self.search_path)
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .env("NO_COLOR", "1")
            .envs(self.env.iter().map(|(k, v)| (k, v)));
        let display =
            process::display_command("gh", &args.iter().map(OsString::from).collect::<Vec<_>>());
        process::run(command, &display, Some(TIMEOUT))
    }

    fn run(&self, args: &[&str]) -> Result<String> {
        let captured = self.exec(args)?;
        if captured.success() {
            return Ok(captured.stdout);
        }
        Err(Error::Failed {
            command: format!("gh {}", args.join(" ")),
            code: captured.status.code(),
            stderr: if captured.stderr.trim().is_empty() {
                captured.stdout
            } else {
                captured.stderr
            },
        })
    }

    /// The GitHub account `gh` is logged in to; `None` when it is not
    /// logged in or GitHub cannot be reached.
    pub fn account(&self) -> Result<Option<String>> {
        let captured = self.exec(&["api", "user", "--jq", ".login"])?;
        let login = captured.stdout.trim();
        Ok((captured.success() && !login.is_empty()).then(|| login.to_string()))
    }

    /// Whether git should talk to GitHub over SSH, as set in `gh`.
    pub fn prefers_ssh(&self) -> bool {
        self.exec(&["config", "get", "git_protocol", "-h", "github.com"])
            .is_ok_and(|c| c.success() && c.stdout.trim() == "ssh")
    }

    /// Creates an empty private repository called `name` in the logged-in
    /// account and returns the address git should push to.
    pub fn create_private_repo(&self, name: &str) -> Result<String> {
        if !is_repo_name(name) {
            return Err(Error::InvalidRequest(format!(
                "{name:?} is not a valid repository name"
            )));
        }
        let out = self.run(&[
            "repo",
            "create",
            name,
            "--private",
            "--description",
            "Dotfiles, synced with Chezmate",
        ])?;
        let full_name = full_name_from_url(&out).ok_or_else(|| Error::Parse {
            what: "gh repo create",
            message: out.trim().to_string(),
        })?;
        Ok(clone_url(&full_name, self.prefers_ssh()))
    }
}

/// GitHub repository names: letters, digits, `.`, `-` and `_`, not
/// starting with `-` (it would be read as an option).
pub fn is_repo_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name.len() <= 100
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// `owner/name` from the `https://github.com/owner/name` that
/// `gh repo create` prints.
fn full_name_from_url(output: &str) -> Option<String> {
    let url = output
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("https://"))?;
    let path = url.split_once("://")?.1.split_once('/')?.1;
    let path = path.trim_end_matches('/').trim_end_matches(".git");
    let (owner, name) = path.split_once('/')?;
    (!owner.is_empty() && is_repo_name(name)).then(|| format!("{owner}/{name}"))
}

/// The address git uses for a GitHub repository.
pub fn clone_url(full_name: &str, ssh: bool) -> String {
    if ssh {
        format!("git@github.com:{full_name}.git")
    } else {
        format!("https://github.com/{full_name}.git")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_created_repository() {
        assert_eq!(
            full_name_from_url("https://github.com/kada/dotfiles\n").as_deref(),
            Some("kada/dotfiles")
        );
        assert_eq!(
            full_name_from_url(
                "✓ Created repository kada/dot on GitHub\n  https://github.com/kada/dot\n"
            )
            .as_deref(),
            Some("kada/dot")
        );
        assert_eq!(full_name_from_url("nothing"), None);
        assert_eq!(
            clone_url("kada/dotfiles", true),
            "git@github.com:kada/dotfiles.git"
        );
        assert_eq!(
            clone_url("kada/dotfiles", false),
            "https://github.com/kada/dotfiles.git"
        );
    }

    #[test]
    fn repository_names() {
        assert!(is_repo_name("dotfiles"));
        assert!(is_repo_name("my.dot-files_2"));
        assert!(!is_repo_name(""));
        assert!(!is_repo_name("a b"));
        assert!(!is_repo_name("../x"));
        assert!(!is_repo_name("--help"), "would be read as an option");
    }
}

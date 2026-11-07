//! A typed client for the `chezmoi` command line.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::doctor::{DoctorCheck, parse_doctor};
use crate::error::{Error, Result};
use crate::git::Git;
use crate::managed::{ManagedEntry, parse_managed};
use crate::process::{self, Captured};
use crate::status::{StatusEntry, parse_status};

/// Read-only commands normally finish in well under a second. The limit only
/// guards against a template helper (a password manager CLI, for example)
/// that waits forever for input chezmoi cannot give it.
const READ_TIMEOUT: Duration = Duration::from_secs(120);
/// `chezmoi doctor` runs `--version` on every tool it knows about; some of
/// them (such as 1Password's `op`) can block waiting for a desktop approval.
const DOCTOR_TIMEOUT: Duration = Duration::from_secs(30);

/// How to invoke chezmoi. Every field is optional; unset fields use
/// chezmoi's own defaults (its config file, source and destination).
#[derive(Debug, Clone, Default)]
pub struct ChezmoiOptions {
    /// Path to the chezmoi executable. Defaults to `$CHEZMATE_CHEZMOI`, then a
    /// search of `PATH` plus common install locations.
    pub binary: Option<PathBuf>,
    pub source: Option<PathBuf>,
    pub destination: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub persistent_state: Option<PathBuf>,
    pub cache: Option<PathBuf>,
    /// Extra environment for chezmoi and the programs it runs.
    pub env: Vec<(OsString, OsString)>,
}

/// Where chezmoi keeps its state, as resolved by chezmoi itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paths {
    pub source_dir: PathBuf,
    pub dest_dir: PathBuf,
    pub working_tree: PathBuf,
    pub config_file: PathBuf,
}

/// A summary of the chezmoi installation that needs no network or decryption.
#[derive(Debug, Clone, Serialize)]
pub struct Info {
    pub binary: PathBuf,
    pub version: String,
    pub paths: Paths,
    pub source_exists: bool,
    pub config_exists: bool,
    pub source_is_git_repo: bool,
}

/// chezmoi entry types for `--include` / `--exclude`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryType {
    Dirs,
    Files,
    Remove,
    Scripts,
    Symlinks,
    Always,
    Encrypted,
    Externals,
    Templates,
}

impl EntryType {
    fn flag_value(self) -> &'static str {
        match self {
            EntryType::Dirs => "dirs",
            EntryType::Files => "files",
            EntryType::Remove => "remove",
            EntryType::Scripts => "scripts",
            EntryType::Symlinks => "symlinks",
            EntryType::Always => "always",
            EntryType::Encrypted => "encrypted",
            EntryType::Externals => "externals",
            EntryType::Templates => "templates",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct DiffOptions {
    /// Entry types to leave out, e.g. `Encrypted` to avoid showing decrypted
    /// secrets.
    pub exclude: Vec<EntryType>,
}

#[derive(Debug, Clone, Default)]
pub struct ApplyOptions {
    /// Report what would change without touching the destination.
    pub dry_run: bool,
    /// Overwrite targets that were edited outside chezmoi.
    pub force: bool,
    pub exclude: Vec<EntryType>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApplyOutcome {
    pub dry_run: bool,
    /// Targets requested; empty means everything.
    pub targets: Vec<String>,
    /// For a dry run, the changes chezmoi reports it would make.
    pub output: String,
}

#[derive(Debug, Clone, Default)]
pub struct AddOptions {
    pub template: bool,
    pub encrypt: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Skipped {
    pub target: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReAddOutcome {
    pub updated: Vec<String>,
    pub skipped: Vec<Skipped>,
}

/// A handle for running chezmoi. Cheap to clone; clones share cached paths.
#[derive(Debug, Clone)]
pub struct Chezmoi {
    binary: PathBuf,
    options: ChezmoiOptions,
    search_path: OsString,
    paths: Arc<OnceLock<Paths>>,
}

impl Chezmoi {
    /// Locates chezmoi and prepares a handle. Does not run chezmoi.
    pub fn new(options: ChezmoiOptions) -> Result<Self> {
        let search_path = process::search_path();
        let binary = match options
            .binary
            .clone()
            .or_else(|| std::env::var_os("CHEZMATE_CHEZMOI").map(PathBuf::from))
        {
            Some(binary) if binary.is_file() => binary,
            Some(binary) => {
                return Err(Error::ChezmoiNotFound {
                    searched: binary.display().to_string(),
                });
            }
            None => process::find_executable("chezmoi", &search_path).ok_or_else(|| {
                Error::ChezmoiNotFound {
                    searched: search_path.to_string_lossy().into_owned(),
                }
            })?,
        };
        Ok(Self {
            binary,
            options,
            search_path,
            paths: Arc::new(OnceLock::new()),
        })
    }

    pub fn binary(&self) -> &Path {
        &self.binary
    }

    pub fn options(&self) -> &ChezmoiOptions {
        &self.options
    }

    /// The `PATH` given to chezmoi and the programs it starts.
    pub fn search_path(&self) -> &OsString {
        &self.search_path
    }

    fn global_args(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = [
            "--no-tty",
            "--no-pager",
            "--color=false",
            "--progress=false",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        let o = &self.options;
        for (flag, value) in [
            ("--source", &o.source),
            ("--destination", &o.destination),
            ("--config", &o.config),
            ("--persistent-state", &o.persistent_state),
            ("--cache", &o.cache),
        ] {
            if let Some(value) = value {
                args.push(flag.into());
                args.push(value.into());
            }
        }
        args
    }

    fn command(&self, args: &[OsString]) -> Command {
        let mut command = Command::new(&self.binary);
        command.args(self.global_args()).args(args);
        command.env("PATH", &self.search_path);
        command.envs(self.options.env.iter().map(|(k, v)| (k, v)));
        command
    }

    fn display(&self, args: &[OsString]) -> String {
        process::display_command("chezmoi", args)
    }

    /// Runs chezmoi and returns its output whatever the exit status.
    fn exec(&self, args: Vec<OsString>, timeout: Option<Duration>) -> Result<Captured> {
        let display = self.display(&args);
        process::run(self.command(&args), &display, timeout)
    }

    /// Runs chezmoi and returns stdout, turning a failure into an error.
    fn run(&self, args: Vec<OsString>, timeout: Option<Duration>) -> Result<String> {
        let display = self.display(&args);
        let captured = self.exec(args, timeout)?;
        if captured.success() {
            return Ok(captured.stdout);
        }
        let conflicts = conflict_paths(&captured);
        if !conflicts.is_empty() {
            return Err(Error::Conflict {
                paths: conflicts,
                stderr: captured.stderr,
            });
        }
        Err(Error::Failed {
            command: display,
            code: captured.status.code(),
            stderr: if captured.stderr.trim().is_empty() {
                captured.stdout
            } else {
                captured.stderr
            },
        })
    }

    /// The chezmoi version string, e.g. `2.73.3`.
    pub fn version(&self) -> Result<String> {
        let mut command = Command::new(&self.binary);
        command.arg("--version").env("PATH", &self.search_path);
        let captured = process::run(command, "chezmoi --version", Some(READ_TIMEOUT))?;
        parse_version(&captured.stdout).ok_or_else(|| Error::Parse {
            what: "chezmoi --version",
            message: captured.stdout.trim().to_string(),
        })
    }

    /// Source, destination, working tree and config file, as chezmoi
    /// resolves them. Cached after the first successful call.
    pub fn paths(&self) -> Result<Paths> {
        if let Some(paths) = self.paths.get() {
            return Ok(paths.clone());
        }
        // Ask for exactly these values: `chezmoi data` would also return the
        // user's template data, which often holds secrets.
        const TEMPLATE: &str = r#"{{ dict "source_dir" .chezmoi.sourceDir "dest_dir" .chezmoi.destDir "working_tree" .chezmoi.workingTree "config_file" .chezmoi.configFile | toJson }}"#;
        let out = self.run(
            vec!["execute-template".into(), TEMPLATE.into()],
            Some(READ_TIMEOUT),
        )?;
        let paths: Paths = serde_json::from_str(out.trim()).map_err(|err| Error::Parse {
            what: "chezmoi paths",
            message: err.to_string(),
        })?;
        Ok(self.paths.get_or_init(|| paths).clone())
    }

    pub fn info(&self) -> Result<Info> {
        let version = self.version()?;
        let paths = self.paths()?;
        Ok(Info {
            binary: self.binary.clone(),
            version,
            source_exists: paths.source_dir.is_dir(),
            config_exists: paths.config_file.is_file(),
            source_is_git_repo: paths.working_tree.join(".git").exists(),
            paths,
        })
    }

    /// Converts targets (relative to the destination, or absolute) to the
    /// absolute paths chezmoi expects. chezmoi itself resolves relative
    /// arguments against the process working directory, which is never what
    /// a GUI or agent means.
    fn target_args(&self, targets: &[String]) -> Result<Vec<OsString>> {
        if targets.is_empty() {
            return Ok(Vec::new());
        }
        let dest = self.paths()?.dest_dir;
        targets
            .iter()
            .map(|target| Ok(self.absolute_target(&dest, target)?.into_os_string()))
            .collect()
    }

    fn absolute_target(&self, dest: &Path, target: &str) -> Result<PathBuf> {
        let path = Path::new(target);
        if target.trim().is_empty() || path.components().any(|c| c == Component::ParentDir) {
            return Err(Error::InvalidRequest(format!(
                "target {target:?} must be a path inside the destination directory"
            )));
        }
        // `~/x` means the destination, as in chezmoi's documentation; agents
        // often send it without a shell to expand it.
        if let Some(rest) = target.strip_prefix("~/") {
            return Ok(dest.join(rest));
        }
        Ok(if path.is_absolute() {
            path.to_path_buf()
        } else {
            dest.join(path)
        })
    }

    /// The target path relative to the destination, as `status` reports it.
    fn relative_target(&self, dest: &Path, target: &str) -> Result<String> {
        let absolute = self.absolute_target(dest, target)?;
        absolute
            .strip_prefix(dest)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .map_err(|_| {
                Error::InvalidRequest(format!(
                    "{target:?} is not inside the destination directory {}",
                    dest.display()
                ))
            })
    }

    /// Targets that differ from the source state. Templates are rendered and
    /// encrypted files decrypted to compute this, so it may invoke gpg, age or
    /// a password manager. Scripts that run on every apply are marked with
    /// [`StatusEntry::runs_every_apply`].
    pub fn status(&self, targets: &[String]) -> Result<Vec<StatusEntry>> {
        let mut args: Vec<OsString> = vec!["status".into()];
        args.extend(self.target_args(targets)?);
        let mut entries = parse_status(&self.run(args, Some(READ_TIMEOUT))?)?;
        if entries.iter().any(StatusEntry::is_script) {
            let always: HashMap<String, bool> = self
                .managed()?
                .into_iter()
                .map(|m| (m.target.clone(), m.runs_every_apply()))
                .collect();
            for entry in entries.iter_mut().filter(|e| e.is_script()) {
                entry.runs_every_apply = always.get(&entry.path).copied().unwrap_or(false);
            }
        }
        Ok(entries)
    }

    /// Every target chezmoi manages, sorted by target path.
    pub fn managed(&self) -> Result<Vec<ManagedEntry>> {
        let args = ["managed", "--path-style", "all", "--format", "json"]
            .map(OsString::from)
            .to_vec();
        parse_managed(&self.run(args, Some(READ_TIMEOUT))?)
    }

    /// A Git-format diff from the current destination to the target state:
    /// what `apply` would change.
    pub fn diff(&self, targets: &[String], options: &DiffOptions) -> Result<String> {
        let mut args: Vec<OsString> = vec!["diff".into(), "--use-builtin-diff".into()];
        push_types(&mut args, "--exclude", &options.exclude);
        args.extend(self.target_args(targets)?);
        self.run(args, Some(READ_TIMEOUT))
    }

    /// Updates the destination to match the source state. Runs scripts.
    ///
    /// Without `force`, a target edited outside chezmoi makes chezmoi stop
    /// with [`Error::Conflict`] instead of prompting.
    pub fn apply(&self, targets: &[String], options: &ApplyOptions) -> Result<ApplyOutcome> {
        let mut args: Vec<OsString> = vec!["apply".into()];
        if options.dry_run {
            args.push("--dry-run".into());
            args.push("--verbose".into());
        }
        if options.force {
            args.push("--force".into());
        }
        push_types(&mut args, "--exclude", &options.exclude);
        args.extend(self.target_args(targets)?);
        // Scripts may legitimately run for a long time (package installs).
        let timeout = options.dry_run.then_some(READ_TIMEOUT);
        let output = self.run(args, timeout)?;
        Ok(ApplyOutcome {
            dry_run: options.dry_run,
            targets: targets.to_vec(),
            output,
        })
    }

    /// Starts managing files. Relative paths are relative to the destination.
    pub fn add(&self, paths: &[String], options: &AddOptions) -> Result<()> {
        if paths.is_empty() {
            return Err(Error::InvalidRequest("add needs at least one path".into()));
        }
        let mut args: Vec<OsString> = vec!["add".into()];
        if options.template {
            args.push("--template".into());
        }
        if options.encrypt {
            args.push("--encrypt".into());
        }
        args.extend(self.target_args(paths)?);
        self.run(args, Some(READ_TIMEOUT)).map(drop)
    }

    /// Copies edits made to targets back into the source state.
    ///
    /// chezmoi does not re-add templates (it cannot invert a template), so
    /// those are reported in [`ReAddOutcome::skipped`] instead of being
    /// silently ignored.
    pub fn re_add(&self, targets: &[String]) -> Result<ReAddOutcome> {
        if targets.is_empty() {
            return Err(Error::InvalidRequest(
                "re-add needs at least one target".into(),
            ));
        }
        let dest = self.paths()?.dest_dir;
        let managed: HashMap<String, ManagedEntry> = self
            .managed()?
            .into_iter()
            .map(|entry| (entry.target.clone(), entry))
            .collect();

        let mut updated = Vec::new();
        let mut skipped = Vec::new();
        for target in targets {
            let relative = self.relative_target(&dest, target)?;
            match managed.get(&relative) {
                None => {
                    return Err(Error::InvalidRequest(format!(
                        "{relative} is not managed by chezmoi"
                    )));
                }
                Some(entry) if entry.attributes.template => skipped.push(Skipped {
                    target: relative,
                    reason: format!(
                        "{} is a template; edit the template in the source directory instead",
                        entry.source_relative
                    ),
                }),
                Some(_) => updated.push(relative),
            }
        }
        if !updated.is_empty() {
            let mut args: Vec<OsString> = vec!["re-add".into()];
            args.extend(self.target_args(&updated)?);
            self.run(args, Some(READ_TIMEOUT))?;
        }
        Ok(ReAddOutcome { updated, skipped })
    }

    /// Stops managing targets. Removes them from the source state only; the
    /// files in the destination stay where they are.
    pub fn forget(&self, targets: &[String]) -> Result<()> {
        if targets.is_empty() {
            return Err(Error::InvalidRequest(
                "forget needs at least one target".into(),
            ));
        }
        let mut args: Vec<OsString> = vec!["forget".into(), "--force".into()];
        args.extend(self.target_args(targets)?);
        self.run(args, Some(READ_TIMEOUT)).map(drop)
    }

    /// The target contents chezmoi would write (templates rendered,
    /// encrypted files decrypted).
    pub fn cat(&self, target: &str) -> Result<String> {
        let mut args: Vec<OsString> = vec!["cat".into()];
        args.extend(self.target_args(&[target.to_string()])?);
        self.run(args, Some(READ_TIMEOUT))
    }

    /// The source file that produces `target`.
    pub fn source_path(&self, target: &str) -> Result<PathBuf> {
        let mut args: Vec<OsString> = vec!["source-path".into()];
        args.extend(self.target_args(&[target.to_string()])?);
        Ok(PathBuf::from(self.run(args, Some(READ_TIMEOUT))?.trim()))
    }

    /// Git for the source directory's working tree. Uses the same `PATH`
    /// and extra environment as chezmoi.
    pub fn git(&self) -> Result<Git> {
        let paths = self.paths()?;
        Git::new(
            paths.working_tree,
            self.search_path.clone(),
            self.options.env.clone(),
        )
    }

    /// `chezmoi doctor --no-network`. Bounded by a timeout because some tool
    /// version checks can block indefinitely.
    pub fn doctor(&self) -> Result<Vec<DoctorCheck>> {
        let args = ["doctor", "--no-network"].map(OsString::from).to_vec();
        let display = self.display(&args);
        let captured = self.exec(args, Some(DOCTOR_TIMEOUT))?;
        let checks = parse_doctor(&captured.stdout);
        if checks.is_empty() && !captured.success() {
            return Err(Error::Failed {
                command: display,
                code: captured.status.code(),
                stderr: captured.stderr,
            });
        }
        Ok(checks)
    }
}

fn push_types(args: &mut Vec<OsString>, flag: &str, types: &[EntryType]) {
    if !types.is_empty() {
        let value: Vec<&str> = types.iter().map(|t| t.flag_value()).collect();
        args.push(flag.into());
        args.push(value.join(",").into());
    }
}

fn parse_version(output: &str) -> Option<String> {
    // "chezmoi version v2.73.3, commit …" or "chezmoi version dev"
    let rest = output.trim().strip_prefix("chezmoi version ")?;
    let version = rest.split([',', ' ']).next()?.trim_start_matches('v');
    (!version.is_empty()).then(|| version.to_string())
}

/// Paths chezmoi refused to overwrite because they changed since it last
/// wrote them. chezmoi prints `<path> has changed since chezmoi last wrote
/// it` as a prompt, then fails reading the answer.
fn conflict_paths(captured: &Captured) -> Vec<String> {
    const MARKER: &str = " has changed since chezmoi last wrote it";
    captured
        .stdout
        .lines()
        .chain(captured.stderr.lines())
        .filter_map(|line| {
            let index = line.find(MARKER)?;
            Some(line[..index].trim().to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_release_and_dev_versions() {
        assert_eq!(
            parse_version("chezmoi version v2.73.3, commit 63093ac, built at 2026\n").as_deref(),
            Some("2.73.3")
        );
        assert_eq!(
            parse_version("chezmoi version dev\n").as_deref(),
            Some("dev")
        );
        assert_eq!(parse_version("something else"), None);
    }
}

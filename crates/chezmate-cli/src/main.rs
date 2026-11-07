//! `chezmate`: chezmoi operations with predictable, machine-readable output.

mod mcp;
mod output;

use std::path::PathBuf;
use std::process::ExitCode;

use chezmate_core::{
    AddOptions, ApplyOptions, Chezmoi, ChezmoiOptions, DiffOptions, EntryType, Error, SyncOptions,
};
use clap::{Args, Parser, Subcommand};
use serde_json::json;

use output::Output;

/// Chezmate: manage dotfiles with chezmoi, for people and AI agents.
///
/// Relative target paths are relative to the destination (your home
/// directory), exactly as `chezmate status` prints them, wherever you run
/// the command. Absolute paths and `~/…` also work.
#[derive(Parser)]
#[command(name = "chezmate", version, about, long_about)]
struct Cli {
    /// Print one JSON object: {"ok":true,"data":…} or {"ok":false,"error":{…}}.
    #[arg(long, global = true)]
    json: bool,

    #[command(flatten)]
    chezmoi: ChezmoiArgs,

    #[command(subcommand)]
    command: Command,
}

#[derive(Args)]
struct ChezmoiArgs {
    /// chezmoi executable (default: $CHEZMATE_CHEZMOI, then PATH).
    #[arg(long, global = true, value_name = "PATH")]
    chezmoi: Option<PathBuf>,
    /// chezmoi source directory.
    #[arg(long, global = true, value_name = "DIR")]
    source: Option<PathBuf>,
    /// chezmoi destination directory.
    #[arg(long, global = true, value_name = "DIR")]
    destination: Option<PathBuf>,
    /// chezmoi config file.
    #[arg(long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,
    /// chezmoi persistent state file.
    #[arg(long, global = true, value_name = "FILE")]
    persistent_state: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Every file that differs between this computer and the shared version,
    /// with one state each, plus setup steps and the shared repository.
    Overview,
    /// Bring this computer and the shared version together: pull, save
    /// changes made here, update this computer, run setup steps, then commit
    /// and push. Files changed on both sides are left for you to decide.
    Sync {
        /// Fetch and show the plan without changing anything.
        #[arg(long, short = 'n')]
        dry_run: bool,
        /// A file changed on this computer not to save this time (repeatable).
        #[arg(long, value_name = "TARGET")]
        skip: Vec<String>,
    },
    /// chezmoi version and the directories it uses.
    Info,
    /// Targets that differ from the source state.
    Status { targets: Vec<String> },
    /// Every target chezmoi manages.
    Managed,
    /// What `apply` would change, as a Git-format diff.
    Diff {
        targets: Vec<String>,
        /// Leave out encrypted files so decrypted secrets are not printed.
        #[arg(long)]
        exclude_encrypted: bool,
    },
    /// Update targets to match the source state (runs scripts).
    Apply {
        targets: Vec<String>,
        /// Show what would change without writing anything.
        #[arg(long, short = 'n')]
        dry_run: bool,
        /// Overwrite targets that were edited outside chezmoi.
        #[arg(long)]
        force: bool,
    },
    /// Start managing files.
    Add {
        #[arg(required = true)]
        paths: Vec<String>,
        /// Add as templates.
        #[arg(long)]
        template: bool,
        /// Encrypt in the source state (needs encryption configured in chezmoi).
        #[arg(long)]
        encrypt: bool,
    },
    /// Copy local edits back into the source state (default: every target
    /// edited outside chezmoi).
    ReAdd { targets: Vec<String> },
    /// Stop managing targets. The files themselves are left in place.
    Forget {
        #[arg(required = true)]
        targets: Vec<String>,
        /// Confirm removal from the source state.
        #[arg(long)]
        yes: bool,
    },
    /// The contents chezmoi would write to a target.
    Cat { target: String },
    /// The source file for a target.
    SourcePath { target: String },
    /// Run `chezmoi doctor` (no network).
    Doctor,
    /// The git repository behind the source directory.
    #[command(subcommand)]
    Git(GitCommand),
    /// Serve the Model Context Protocol over stdio for AI agents.
    ///
    /// Speaks both the 2026-07-28 stateless protocol and the earlier
    /// initialize-based revisions. Logs go to stderr.
    Mcp {
        /// Expose only tools that do not change anything.
        #[arg(long)]
        read_only: bool,
    },
}

#[derive(Subcommand)]
enum GitCommand {
    /// Branch, upstream, ahead/behind and uncommitted changes.
    Status,
    /// Recent commits.
    Log {
        #[arg(long, short = 'n', default_value_t = 20)]
        limit: usize,
    },
    /// Uncommitted changes as a Git diff (includes untracked files).
    Diff,
    /// Stage everything and commit.
    Commit {
        #[arg(long, short = 'm')]
        message: String,
    },
    /// Update remote-tracking branches.
    Fetch,
    /// Pull with --rebase --autostash. Only updates the source directory;
    /// run `chezmate apply` afterwards. A conflicting pull is aborted.
    Pull,
    /// Push the current branch.
    Push,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Command::Mcp { read_only } = cli.command {
        // Start even without chezmoi: tools then report `chezmoi_not_found`.
        return match mcp::serve(connect(cli.chezmoi), read_only) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("chezmate mcp: {err}");
                ExitCode::FAILURE
            }
        };
    }
    let out = Output::new(cli.json);
    match run(cli.command, cli.chezmoi, &out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            out.error(&err);
            ExitCode::FAILURE
        }
    }
}

fn connect(args: ChezmoiArgs) -> Result<Chezmoi, Error> {
    Chezmoi::new(ChezmoiOptions {
        binary: args.chezmoi,
        source: args.source,
        destination: args.destination,
        config: args.config,
        persistent_state: args.persistent_state,
        ..Default::default()
    })
}

fn run_git(command: GitCommand, chezmoi: &Chezmoi, out: &Output) -> Result<(), Error> {
    let git = chezmoi.git()?;
    match command {
        GitCommand::Status => {
            let status = git.status()?;
            out.data(&status, || output::repo_status(&status));
        }
        GitCommand::Log { limit } => {
            let commits = git.log(limit)?;
            out.data(&commits, || output::log(&commits));
        }
        GitCommand::Diff => {
            let diff = git.diff()?;
            out.data(&json!({ "diff": diff }), || diff.clone());
        }
        GitCommand::Commit { message } => {
            let hash = git.commit_all(&message)?;
            out.data(&json!({ "commit": hash }), || {
                format!("Committed {}", &hash[..hash.len().min(10)])
            });
        }
        GitCommand::Fetch => {
            git.fetch()?;
            let status = git.status()?;
            out.data(&status, || output::repo_status(&status));
        }
        GitCommand::Pull => {
            git.pull()?;
            let status = chezmoi.status(&[])?;
            out.data(&json!({ "pending": status }), || {
                format!("Pulled. {}", output::status(&status))
            });
        }
        GitCommand::Push => {
            git.push()?;
            out.data(&json!({ "pushed": true }), || "Pushed.".to_string());
        }
    }
    Ok(())
}

fn run(command: Command, args: ChezmoiArgs, out: &Output) -> Result<(), Error> {
    let chezmoi = connect(args)?;
    match command {
        Command::Overview => {
            let overview = chezmoi.overview()?;
            out.data(&overview, || output::overview(&overview));
        }
        Command::Sync { dry_run, skip } => {
            let report = chezmoi.sync(&SyncOptions { dry_run, skip })?;
            out.data(&report, || output::sync(&report));
        }
        Command::Info => {
            let info = chezmoi.info()?;
            out.data(&info, || output::info(&info));
        }
        Command::Status { targets } => {
            let status = chezmoi.status(&targets)?;
            out.data(&status, || output::status(&status));
        }
        Command::Managed => {
            let managed = chezmoi.managed()?;
            out.data(&managed, || output::managed(&managed));
        }
        Command::Diff {
            targets,
            exclude_encrypted,
        } => {
            let options = DiffOptions {
                exclude: exclude_encrypted
                    .then_some(EntryType::Encrypted)
                    .into_iter()
                    .collect(),
                ..Default::default()
            };
            let diff = chezmoi.diff(&targets, &options)?;
            out.data(&json!({ "diff": diff }), || diff.clone());
        }
        Command::Apply {
            targets,
            dry_run,
            force,
        } => {
            let outcome = chezmoi.apply(
                &targets,
                &ApplyOptions {
                    dry_run,
                    force,
                    ..Default::default()
                },
            )?;
            out.data(&outcome, || output::apply(&outcome));
        }
        Command::Add {
            paths,
            template,
            encrypt,
        } => {
            chezmoi.add(&paths, &AddOptions { template, encrypt })?;
            out.data(&json!({ "added": paths }), || {
                format!("Added {}", paths.join(", "))
            });
        }
        Command::ReAdd { targets } => {
            let targets = if targets.is_empty() {
                chezmoi
                    .status(&[])?
                    .into_iter()
                    .filter(|e| e.has_local_changes() && !e.is_script())
                    .map(|e| e.path)
                    .collect()
            } else {
                targets
            };
            if targets.is_empty() {
                let empty = json!({ "updated": [], "skipped": [] });
                out.data(&empty, || "No local edits to re-add".to_string());
            } else {
                let outcome = chezmoi.re_add(&targets)?;
                out.data(&outcome, || output::re_add(&outcome));
            }
        }
        Command::Forget { targets, yes } => {
            if !yes {
                return Err(Error::InvalidRequest(
                    "forget removes targets from the source state; pass --yes to confirm".into(),
                ));
            }
            chezmoi.forget(&targets)?;
            out.data(&json!({ "forgotten": targets }), || {
                format!("No longer managing {}", targets.join(", "))
            });
        }
        Command::Cat { target } => {
            let contents = chezmoi.cat(&target)?;
            out.data(&json!({ "contents": contents }), || contents.clone());
        }
        Command::SourcePath { target } => {
            let path = chezmoi.source_path(&target)?;
            out.data(&json!({ "source_path": path }), || {
                path.display().to_string()
            });
        }
        Command::Doctor => {
            let checks = chezmoi.doctor()?;
            out.data(&checks, || output::doctor(&checks));
        }
        Command::Git(command) => run_git(command, &chezmoi, out)?,
        Command::Mcp { .. } => unreachable!("handled in main"),
    }
    Ok(())
}

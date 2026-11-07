//! Typed, testable access to [chezmoi](https://www.chezmoi.io/) for Chezmate.
//!
//! Everything here runs the real `chezmoi` and `git` executables and parses
//! their output. [`Overview`] and [`SyncPlan`] are the simplified model the
//! apps show to people: one state per file and one sync action. Nothing is cached on disk and nothing is simulated:
//! the GUI, CLI and MCP server all see exactly what chezmoi reports.

mod chezmoi;
mod doctor;
mod error;
mod git;
mod managed;
pub mod process;
mod status;
mod sync;

pub use chezmoi::{
    AddOptions, ApplyOptions, ApplyOutcome, Chezmoi, ChezmoiOptions, DiffOptions, EntryType, Info,
    Paths, ReAddOutcome, Skipped,
};
pub use doctor::{CheckResult, DoctorCheck, parse_doctor};
pub use error::{Error, Result};
pub use git::{Commit, Git, GitChange, GitFileStatus, RepoStatus, parse_status_v2};
pub use managed::{EntryKind, ManagedEntry, SourceAttributes, classify, parse_managed};
pub use status::{Change, StatusEntry, parse_status};
pub use sync::{
    FileItem, FileState, Overview, SaveBlocker, ScriptOrder, ScriptRuns, SetupStep, SharedStatus,
    SyncOptions, SyncOutcome, SyncPlan, SyncReport,
};

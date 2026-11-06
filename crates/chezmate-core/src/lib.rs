//! Typed, testable access to [chezmoi](https://www.chezmoi.io/) for Chezmate.
//!
//! Everything here runs the real `chezmoi` (and later `git`) executables and
//! parses their output. Nothing is cached on disk and nothing is simulated:
//! the GUI, CLI and MCP server all see exactly what chezmoi reports.

mod chezmoi;
mod doctor;
mod error;
mod managed;
pub mod process;
mod status;

pub use chezmoi::{
    AddOptions, ApplyOptions, ApplyOutcome, Chezmoi, ChezmoiOptions, DiffOptions, EntryType, Info,
    Paths, ReAddOutcome, Skipped,
};
pub use doctor::{CheckResult, DoctorCheck, parse_doctor};
pub use error::{Error, Result};
pub use managed::{EntryKind, ManagedEntry, SourceAttributes, classify, parse_managed};
pub use status::{Change, StatusEntry, parse_status};

use std::io;

/// Errors from running chezmoi or git and interpreting their output.
///
/// Every variant maps to a stable [`Error::kind`] string so the CLI and MCP
/// server can report machine-readable failures.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("chezmoi executable not found; searched: {searched}")]
    ChezmoiNotFound { searched: String },

    #[error("git executable not found; searched: {searched}")]
    GitNotFound { searched: String },

    #[error("failed to start `{command}`: {source}")]
    Spawn {
        command: String,
        #[source]
        source: io::Error,
    },

    #[error("`{command}` did not finish within {seconds}s and was stopped")]
    Timeout { command: String, seconds: u64 },

    /// A target was modified outside chezmoi since chezmoi last wrote it.
    /// Re-run with `force` to overwrite, or re-add the local version first.
    #[error("{} changed since chezmoi last wrote it; apply with force to overwrite", paths.join(", "))]
    Conflict { paths: Vec<String>, stderr: String },

    #[error("`{command}` failed ({}): {}", exit_label(*code), stderr.trim())]
    Failed {
        command: String,
        code: Option<i32>,
        stderr: String,
    },

    #[error("could not parse {what}: {message}")]
    Parse { what: &'static str, message: String },

    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: io::Error,
    },
}

fn exit_label(code: Option<i32>) -> String {
    match code {
        Some(code) => format!("exit {code}"),
        None => "terminated by signal".to_string(),
    }
}

impl Error {
    /// A stable identifier for the error category.
    pub fn kind(&self) -> &'static str {
        match self {
            Error::ChezmoiNotFound { .. } => "chezmoi_not_found",
            Error::GitNotFound { .. } => "git_not_found",
            Error::Spawn { .. } => "spawn_failed",
            Error::Timeout { .. } => "timeout",
            Error::Conflict { .. } => "conflict",
            Error::Failed { .. } => "command_failed",
            Error::Parse { .. } => "parse_error",
            Error::InvalidRequest(_) => "invalid_request",
            Error::Io { .. } => "io_error",
        }
    }

    /// Captured standard error of the failing command, if any.
    pub fn stderr(&self) -> Option<&str> {
        match self {
            Error::Failed { stderr, .. } | Error::Conflict { stderr, .. } => Some(stderr),
            _ => None,
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

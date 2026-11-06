//! `chezmoi status` output.

use serde::Serialize;

use crate::error::{Error, Result};

/// One column of `chezmoi status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    None,
    Added,
    Deleted,
    Modified,
    /// A script that `chezmoi apply` will run.
    Run,
}

impl Change {
    fn from_code(code: char) -> Option<Self> {
        Some(match code {
            ' ' => Change::None,
            'A' => Change::Added,
            'D' => Change::Deleted,
            'M' => Change::Modified,
            'R' => Change::Run,
            _ => return None,
        })
    }

    pub fn is_none(self) -> bool {
        self == Change::None
    }
}

/// A target that differs from what chezmoi last wrote or will write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusEntry {
    /// Target path relative to the destination directory, `/`-separated.
    pub path: String,
    /// First column: how the file on disk differs from what chezmoi last
    /// wrote. Anything other than `None` means the target was edited outside
    /// chezmoi, and applying would overwrite that edit.
    pub local: Change,
    /// Second column: what `chezmoi apply` would do to the target.
    pub apply: Change,
}

impl StatusEntry {
    /// The file on disk was changed outside chezmoi.
    pub fn has_local_changes(&self) -> bool {
        !self.local.is_none()
    }

    /// `chezmoi apply` would change this target or run this script.
    pub fn needs_apply(&self) -> bool {
        !self.apply.is_none()
    }

    pub fn is_script(&self) -> bool {
        self.apply == Change::Run
    }
}

/// Parses `chezmoi status` output: two status columns, a space, the path.
pub fn parse_status(output: &str) -> Result<Vec<StatusEntry>> {
    let mut entries = Vec::new();
    for (number, line) in output.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let mut chars = line.chars();
        let (Some(first), Some(second), Some(' ')) = (chars.next(), chars.next(), chars.next())
        else {
            return Err(parse_error(number, line));
        };
        let (Some(local), Some(apply)) = (Change::from_code(first), Change::from_code(second))
        else {
            return Err(parse_error(number, line));
        };
        let path = chars.as_str();
        if path.is_empty() {
            return Err(parse_error(number, line));
        }
        entries.push(StatusEntry {
            path: path.to_string(),
            local,
            apply,
        });
    }
    Ok(entries)
}

fn parse_error(number: usize, line: &str) -> Error {
    Error::Parse {
        what: "chezmoi status",
        message: format!("line {}: {line:?}", number + 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_columns() {
        let entries = parse_status(" A .hello\nMM .config/my app/rc\n R install.sh\n").unwrap();
        assert_eq!(
            entries,
            vec![
                StatusEntry {
                    path: ".hello".into(),
                    local: Change::None,
                    apply: Change::Added
                },
                StatusEntry {
                    path: ".config/my app/rc".into(),
                    local: Change::Modified,
                    apply: Change::Modified
                },
                StatusEntry {
                    path: "install.sh".into(),
                    local: Change::None,
                    apply: Change::Run
                },
            ]
        );
        assert!(entries[1].has_local_changes());
        assert!(entries[2].is_script());
    }

    #[test]
    fn empty_output_means_clean() {
        assert!(parse_status("").unwrap().is_empty());
    }

    #[test]
    fn rejects_unknown_codes() {
        assert_eq!(parse_status("XY foo\n").unwrap_err().kind(), "parse_error");
        assert_eq!(parse_status("M\n").unwrap_err().kind(), "parse_error");
    }
}

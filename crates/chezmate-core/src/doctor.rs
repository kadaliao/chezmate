//! `chezmoi doctor` output.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckResult {
    Ok,
    Info,
    Warning,
    Error,
    Failed,
    Skipped,
    Unknown,
}

impl CheckResult {
    fn parse(word: &str) -> Self {
        match word {
            "ok" => CheckResult::Ok,
            "info" => CheckResult::Info,
            "warning" => CheckResult::Warning,
            "error" => CheckResult::Error,
            "failed" => CheckResult::Failed,
            "skipped" => CheckResult::Skipped,
            _ => CheckResult::Unknown,
        }
    }

    pub fn is_problem(self) -> bool {
        matches!(self, CheckResult::Error | CheckResult::Failed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DoctorCheck {
    pub result: CheckResult,
    pub check: String,
    pub message: String,
}

/// Parses the `RESULT  CHECK  MESSAGE` table printed by `chezmoi doctor`.
pub fn parse_doctor(output: &str) -> Vec<DoctorCheck> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim_end();
            let (result, rest) = line.split_once(char::is_whitespace)?;
            let rest = rest.trim_start();
            let (check, message) = rest
                .split_once(char::is_whitespace)
                .map(|(check, message)| (check, message.trim()))
                .unwrap_or((rest, ""));
            if result == "RESULT" || check.is_empty() {
                return None;
            }
            Some(DoctorCheck {
                result: CheckResult::parse(result),
                check: check.to_string(),
                message: message.to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_table() {
        let out = "RESULT    CHECK                       MESSAGE\n\
                   ok        version                     v2.73.3, commit abc\n\
                   warning   edit-command                nvim not found in $PATH\n\
                   info      diff-command                not set\n";
        let checks = parse_doctor(out);
        assert_eq!(checks.len(), 3);
        assert_eq!(checks[0].check, "version");
        assert_eq!(checks[0].message, "v2.73.3, commit abc");
        assert_eq!(checks[1].result, CheckResult::Warning);
        assert!(!checks[1].result.is_problem());
    }
}

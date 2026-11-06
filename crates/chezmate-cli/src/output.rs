//! Human and JSON rendering for CLI results.

use std::fmt::Write as _;

use chezmate_core::{
    ApplyOutcome, Change, DoctorCheck, Error, Info, ManagedEntry, ReAddOutcome, StatusEntry,
};
use serde::Serialize;
use serde_json::json;

pub struct Output {
    json: bool,
}

impl Output {
    pub fn new(json: bool) -> Self {
        Self { json }
    }

    /// Prints a successful result: the JSON envelope, or `human()`.
    pub fn data<T: Serialize>(&self, data: &T, human: impl FnOnce() -> String) {
        if self.json {
            println!("{}", json!({ "ok": true, "data": data }));
        } else {
            let text = human();
            if text.ends_with('\n') || text.is_empty() {
                print!("{text}");
            } else {
                println!("{text}");
            }
        }
    }

    pub fn error(&self, err: &Error) {
        if self.json {
            let mut error = json!({ "kind": err.kind(), "message": err.to_string() });
            if let Some(stderr) = err.stderr() {
                error["stderr"] = json!(stderr);
            }
            if let Error::Conflict { paths, .. } = err {
                error["paths"] = json!(paths);
            }
            println!("{}", json!({ "ok": false, "error": error }));
        } else {
            eprintln!("error: {err}");
            if let Error::Conflict { .. } = err {
                eprintln!(
                    "hint: keep your edit with `chezmate re-add <target>`, or overwrite it with `chezmate apply --force <target>`"
                );
            }
        }
    }
}

pub fn info(info: &Info) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "chezmoi      {} ({})",
        info.version,
        info.binary.display()
    );
    let mut source = info.paths.source_dir.display().to_string();
    if !info.source_exists {
        source.push_str("  (missing; run `chezmoi init`)");
    } else if info.source_is_git_repo {
        source.push_str("  (git)");
    }
    let _ = writeln!(s, "source       {source}");
    let _ = writeln!(s, "destination  {}", info.paths.dest_dir.display());
    let config = info.paths.config_file.display().to_string();
    let _ = writeln!(
        s,
        "config       {config}{}",
        if info.config_exists { "" } else { "  (none)" }
    );
    s
}

fn apply_label(change: Change) -> &'static str {
    match change {
        Change::None => "unchanged",
        Change::Added => "create",
        Change::Deleted => "delete",
        Change::Modified => "update",
        Change::Run => "run script",
    }
}

pub fn status(entries: &[StatusEntry]) -> String {
    if entries.is_empty() {
        return "Everything is up to date.".into();
    }
    let width = entries.iter().map(|e| e.path.len()).max().unwrap_or(0);
    let mut s = String::new();
    for entry in entries {
        let mut line = format!("{:width$}  {}", entry.path, apply_label(entry.apply));
        if entry.has_local_changes() {
            line.push_str("  (edited locally)");
        }
        let _ = writeln!(s, "{}", line.trim_end());
    }
    s
}

pub fn managed(entries: &[ManagedEntry]) -> String {
    let width = entries.iter().map(|e| e.target.len()).max().unwrap_or(0);
    let mut s = String::new();
    for entry in entries {
        let _ = writeln!(s, "{:width$}  {}", entry.target, entry.source_relative);
    }
    s
}

pub fn apply(outcome: &ApplyOutcome) -> String {
    if outcome.dry_run {
        if outcome.output.trim().is_empty() {
            "Dry run: nothing would change.".into()
        } else {
            outcome.output.clone()
        }
    } else if outcome.targets.is_empty() {
        "Applied all changes.".into()
    } else {
        format!("Applied {}", outcome.targets.join(", "))
    }
}

pub fn re_add(outcome: &ReAddOutcome) -> String {
    let mut s = String::new();
    if !outcome.updated.is_empty() {
        let _ = writeln!(s, "Updated source for {}", outcome.updated.join(", "));
    }
    for skipped in &outcome.skipped {
        let _ = writeln!(s, "Skipped {}: {}", skipped.target, skipped.reason);
    }
    s
}

pub fn doctor(checks: &[DoctorCheck]) -> String {
    let width = checks.iter().map(|c| c.check.len()).max().unwrap_or(0);
    let mut s = String::new();
    for check in checks {
        let result = serde_json::to_value(check.result)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        let _ = writeln!(s, "{result:8} {:width$}  {}", check.check, check.message);
    }
    s
}

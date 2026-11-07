//! Human and JSON rendering for CLI results.

use std::fmt::Write as _;

use chezmate_core::{
    ApplyOutcome, Change, Commit, DoctorCheck, Error, FileItem, FileState, GitFileStatus, Info,
    ManagedEntry, Overview, ReAddOutcome, RepoStatus, SaveBlocker, SharedStatus, StatusEntry,
    SyncPlan, SyncReport,
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
    let mut s = String::new();
    let (pending, always): (Vec<_>, Vec<_>) = entries.iter().partition(|e| e.is_pending());
    if pending.is_empty() {
        s.push_str("Everything is up to date.\n");
    }
    if !always.is_empty() {
        let names: Vec<&str> = always.iter().map(|e| e.path.as_str()).collect();
        let _ = writeln!(s, "Runs on every apply: {}", names.join(", "));
    }
    let width = pending.iter().map(|e| e.path.len()).max().unwrap_or(0);
    for entry in pending {
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

pub fn repo_status(status: &RepoStatus) -> String {
    let mut s = String::new();
    let branch = status.branch.as_deref().unwrap_or("(detached)");
    match &status.upstream {
        Some(upstream) => {
            let _ = writeln!(
                s,
                "{branch} → {upstream}  ({} to push, {} to pull)",
                status.ahead, status.behind
            );
        }
        None => {
            let _ = writeln!(s, "{branch}  (no upstream)");
        }
    }
    if status.changes.is_empty() {
        let _ = writeln!(s, "No uncommitted changes.");
    }
    for change in &status.changes {
        let code = match change.status {
            GitFileStatus::Modified => "M",
            GitFileStatus::Added => "A",
            GitFileStatus::Deleted => "D",
            GitFileStatus::Renamed => "R",
            GitFileStatus::Copied => "C",
            GitFileStatus::TypeChanged => "T",
            GitFileStatus::Untracked => "?",
            GitFileStatus::Conflicted => "U",
        };
        match &change.original_path {
            Some(from) => {
                let _ = writeln!(s, "{code} {from} → {}", change.path);
            }
            None => {
                let _ = writeln!(s, "{code} {}", change.path);
            }
        }
    }
    s
}

pub fn log(commits: &[Commit]) -> String {
    let mut s = String::new();
    for commit in commits {
        let date = commit.date.split('T').next().unwrap_or_default();
        let _ = writeln!(s, "{} {date} {}", commit.short_hash, commit.subject);
    }
    s
}

/// How a file differs, in a few words.
fn file_label(file: &FileItem) -> String {
    let base = match file.state {
        FileState::ChangedHere => "changed here".to_string(),
        FileState::UpdateAvailable => match file.incoming {
            Change::Added => "new in the shared version".to_string(),
            Change::Deleted => "deleted in the shared version".to_string(),
            _ => "updated in the shared version".to_string(),
        },
        FileState::BothChanged if file.never_synced => {
            "this computer has its own version, never synced".to_string()
        }
        FileState::BothChanged => "changed here and in the shared version".to_string(),
        FileState::DeletedHere => "deleted here".to_string(),
    };
    match file.cannot_save {
        Some(SaveBlocker::Template) if file.state != FileState::DeletedHere => {
            format!("{base}; generated from a template, edit the template to keep it")
        }
        Some(SaveBlocker::Script) if file.state != FileState::DeletedHere => {
            format!("{base}; produced by a script, cannot be saved")
        }
        _ => base,
    }
}

fn file_lines(s: &mut String, title: &str, files: &[&FileItem]) {
    if files.is_empty() {
        return;
    }
    let _ = writeln!(s, "{title}");
    let width = files.iter().map(|f| f.target.len() + 2).max().unwrap_or(0);
    for file in files {
        let name = format!("~/{}", file.target);
        let _ = writeln!(s, "  {name:width$}  {}", file_label(file));
    }
}

fn shared_line(shared: Option<&SharedStatus>) -> String {
    let Some(shared) = shared else {
        return "Shared version: the source directory is not a git repository; sync stays on this computer.".into();
    };
    let mut parts = Vec::new();
    match &shared.upstream {
        Some(upstream) => parts.push(format!(
            "{} → {upstream}",
            shared.branch.as_deref().unwrap_or("(detached)")
        )),
        None => parts.push("no remote; changes are committed on this computer only".into()),
    }
    if shared.uncommitted > 0 {
        parts.push(format!("{} uncommitted", shared.uncommitted));
    }
    if shared.upstream.is_some() && shared.ahead > 0 {
        parts.push(format!(
            "{} to upload",
            plural(shared.ahead as usize, "commit")
        ));
    }
    if shared.behind > 0 {
        parts.push(format!(
            "{} to pull",
            plural(shared.behind as usize, "commit")
        ));
    }
    format!("Shared version: {}", parts.join(" · "))
}

fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

pub fn overview(overview: &Overview) -> String {
    let mut s = String::new();
    if overview.files.is_empty() && overview.setup_steps.is_empty() {
        let _ = writeln!(
            s,
            "All {} synced on this computer.",
            plural(overview.managed_files, "file")
        );
    }
    let decide: Vec<&FileItem> = overview.decisions().collect();
    let by_state = |state| -> Vec<&FileItem> {
        overview
            .files
            .iter()
            .filter(|f| f.state == state && !f.needs_decision())
            .collect()
    };
    file_lines(
        &mut s,
        "Needs a decision (sync leaves these alone):",
        &decide,
    );
    file_lines(
        &mut s,
        "Changed on this computer (sync saves these):",
        &by_state(FileState::ChangedHere),
    );
    file_lines(
        &mut s,
        "Updates from the shared version (sync applies these):",
        &by_state(FileState::UpdateAvailable),
    );
    if !overview.setup_steps.is_empty() {
        let names: Vec<&str> = overview
            .setup_steps
            .iter()
            .map(|s| s.target.as_str())
            .collect();
        let _ = writeln!(s, "Setup steps (sync runs these): {}", names.join(", "));
    }
    if !overview.every_apply.is_empty() {
        let names: Vec<&str> = overview
            .every_apply
            .iter()
            .map(|s| s.target.as_str())
            .collect();
        let _ = writeln!(
            s,
            "Runs whenever sync updates this computer: {}",
            names.join(", ")
        );
    }
    let _ = writeln!(s, "{}", shared_line(overview.shared.as_ref()));
    s
}

fn plan_lines(s: &mut String, plan: &SyncPlan) {
    let list = |s: &mut String, verb: &str, targets: &[String], tilde: bool| {
        if !targets.is_empty() {
            let names: Vec<String> = targets
                .iter()
                .map(|t| if tilde { format!("~/{t}") } else { t.clone() })
                .collect();
            let _ = writeln!(s, "  {verb} {}", names.join(", "));
        }
    };
    list(s, "save", &plan.save, true);
    list(s, "update", &plan.update, true);
    list(s, "run", &plan.run, false);
    list(s, "also run", &plan.every_apply, false);
    if plan.upload {
        let _ = writeln!(s, "  upload the shared version");
    }
    list(s, "not saving", &plan.skipped, true);
    list(s, "leave for you to decide", &plan.decide, true);
}

pub fn sync(report: &SyncReport) -> String {
    let mut s = String::new();
    for warning in &report.warnings {
        let _ = writeln!(s, "warning: {warning}");
    }
    let Some(outcome) = &report.outcome else {
        if report.plan.is_empty() {
            let _ = writeln!(s, "Dry run: nothing to sync.");
        } else {
            let _ = writeln!(s, "Dry run. Sync would:");
        }
        plan_lines(&mut s, &report.plan);
        if report.incoming_commits > 0 {
            let _ = writeln!(
                s,
                "{} from elsewhere will be pulled first; the plan may change.",
                plural(report.incoming_commits as usize, "commit")
            );
        }
        return s;
    };
    if report.incoming_commits > 0 {
        let _ = writeln!(
            s,
            "Pulled {}.",
            plural(report.incoming_commits as usize, "commit")
        );
    }
    let tilde = |targets: &[String]| -> String {
        targets
            .iter()
            .map(|t| format!("~/{t}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if !outcome.saved.is_empty() {
        let _ = writeln!(s, "Saved {}", tilde(&outcome.saved));
    }
    if !outcome.updated.is_empty() {
        let _ = writeln!(s, "Updated {}", tilde(&outcome.updated));
    }
    if !outcome.ran.is_empty() {
        let _ = writeln!(s, "Ran {}", outcome.ran.join(", "));
    }
    if let Some(commit) = &outcome.commit {
        let _ = writeln!(s, "Committed {}", &commit[..commit.len().min(10)]);
    }
    if outcome.pushed {
        let _ = writeln!(s, "Uploaded.");
    }
    if let Some(err) = &outcome.push_error {
        let _ = writeln!(
            s,
            "Upload failed: {err}\nEverything on this computer is done; sync again to upload."
        );
    }
    for skipped in &outcome.skipped {
        let _ = writeln!(s, "Skipped ~/{}: {}", skipped.target, skipped.reason);
    }
    if !outcome.decide.is_empty() {
        let _ = writeln!(s, "Needs a decision: {}", tilde(&outcome.decide));
    }
    if s.is_empty() {
        s.push_str("Already in sync.\n");
    }
    s
}

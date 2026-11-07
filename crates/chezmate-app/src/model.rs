//! The app's view of chezmoi: one retained entity that loads state on a
//! background thread and runs operations one at a time.

use std::collections::HashMap;
use std::sync::Arc;

use chezmate_core::{
    AddOptions, ApplyOptions, Chezmoi, ChezmoiOptions, Commit, DiffOptions, DoctorCheck, FileState,
    Info, ManagedEntry, Overview, RepoStatus, StatusEntry, SyncOutcome, SyncPlan,
};
use gpui_kit::component::diff::{DiffFile, DiffParseOptions};
use gpui_kit::{Context, EventEmitter, SharedString, Task};

use crate::i18n::tr;

/// Everything the Changes and Files pages show, read in one refresh.
pub struct Snapshot {
    pub info: Info,
    /// Pending changes: what applying would resolve.
    pub status: Vec<StatusEntry>,
    /// Scripts that run on every apply. chezmoi always lists them, so they
    /// are kept apart from pending changes.
    pub every_apply: Vec<StatusEntry>,
    pub managed: Vec<ManagedEntry>,
    managed_index: HashMap<String, usize>,
    pub diffs: HashMap<SharedString, DiffFile>,
    /// The diff could not be read or parsed; status is still valid.
    pub diff_error: Option<String>,
    /// The source directory's git repository; `None` if it is not one.
    pub repo: Option<Result<Repo, String>>,
    /// The simplified model shown on the Overview page.
    pub overview: Overview,
    /// Diffs from the shared version to this computer, for files changed
    /// here: added lines are this computer's edits.
    pub local_diffs: HashMap<SharedString, DiffFile>,
}

/// Local git state of the source directory (no network access).
pub struct Repo {
    pub status: RepoStatus,
    pub log: Vec<Commit>,
    /// Uncommitted changes, in repository order.
    pub diffs: Vec<DiffFile>,
    pub diff_error: Option<String>,
}

const HISTORY_LENGTH: usize = 30;

impl Snapshot {
    pub fn managed_entry(&self, target: &str) -> Option<&ManagedEntry> {
        self.managed_index.get(target).map(|&ix| &self.managed[ix])
    }

    pub fn diff(&self, target: &str) -> Option<&DiffFile> {
        self.diffs.get(target)
    }

    pub fn local_diff(&self, target: &str) -> Option<&DiffFile> {
        self.local_diffs.get(target)
    }
}

/// The outcome of an operation, shown as a notification.
pub enum DotfilesEvent {
    Finished(Result<String, String>),
    /// A commit succeeded; the message input can be cleared.
    Committed,
    /// The shared version is up to date; the sync runs once someone
    /// confirms the plan made from this overview.
    SyncReady(Arc<Overview>),
}

pub enum DoctorState {
    Idle,
    Running,
    Done(Vec<DoctorCheck>),
    Failed(String),
}

pub struct Dotfiles {
    chezmoi: Result<Chezmoi, String>,
    snapshot: Option<Arc<Snapshot>>,
    loading: bool,
    error: Option<String>,
    busy: Option<SharedString>,
    doctor: DoctorState,
    revision: usize,
    _refresh: Task<()>,
    _operation: Task<()>,
    _doctor: Task<()>,
}

impl EventEmitter<DotfilesEvent> for Dotfiles {}

impl Dotfiles {
    pub fn new(options: ChezmoiOptions, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            chezmoi: Chezmoi::new(options).map_err(|err| err.to_string()),
            snapshot: None,
            loading: false,
            error: None,
            busy: None,
            doctor: DoctorState::Idle,
            revision: 0,
            _refresh: Task::ready(()),
            _operation: Task::ready(()),
            _doctor: Task::ready(()),
        };
        this.refresh(cx);
        this
    }

    pub fn snapshot(&self) -> Option<&Arc<Snapshot>> {
        self.snapshot.as_ref()
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The label of the running operation, if any.
    pub fn busy(&self) -> Option<&SharedString> {
        self.busy.as_ref()
    }

    pub fn doctor(&self) -> &DoctorState {
        &self.doctor
    }

    /// Reloads status, managed files and the pending diff. Keeps the previous
    /// snapshot visible until the new one arrives; a newer refresh discards
    /// the results of an older one.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let chezmoi = match &self.chezmoi {
            Ok(chezmoi) => chezmoi.clone(),
            Err(err) => {
                self.error = Some(err.clone());
                cx.notify();
                return;
            }
        };
        self.revision += 1;
        let revision = self.revision;
        self.loading = true;
        cx.notify();
        self._refresh = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { load(&chezmoi) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.revision != revision {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(snapshot) => {
                        this.snapshot = Some(Arc::new(snapshot));
                        this.error = None;
                    }
                    Err(err) => this.error = Some(err),
                }
                cx.notify();
            });
        });
    }

    /// Runs one mutating operation in the background, then refreshes.
    /// Ignored while another operation is running.
    fn run(
        &mut self,
        label: impl Into<SharedString>,
        operation: impl FnOnce(&Chezmoi) -> Result<String, String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.run_then(label, operation, None, cx)
    }

    /// Like `run`, emitting `on_success` as well when the operation succeeds.
    fn run_then(
        &mut self,
        label: impl Into<SharedString>,
        operation: impl FnOnce(&Chezmoi) -> Result<String, String> + Send + 'static,
        on_success: Option<DotfilesEvent>,
        cx: &mut Context<Self>,
    ) {
        self.run_with(
            label,
            operation,
            move |_, result, cx| {
                let succeeded = result.is_ok();
                cx.emit(DotfilesEvent::Finished(result));
                if succeeded && let Some(event) = on_success {
                    cx.emit(event);
                }
            },
            cx,
        );
    }

    /// Runs `operation` in the background, hands its result to `done`, then
    /// refreshes. Ignored while another operation is running.
    fn run_with<T: Send + 'static>(
        &mut self,
        label: impl Into<SharedString>,
        operation: impl FnOnce(&Chezmoi) -> T + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let Ok(chezmoi) = self.chezmoi.clone() else {
            return;
        };
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(label.into());
        cx.notify();
        self._operation = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { operation(&chezmoi) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                done(this, result, cx);
                this.refresh(cx);
            });
        });
    }

    /// Step one of a sync: brings the shared version up to date (fetch and
    /// pull; nothing on this computer changes) and plans the sync. Emits
    /// [`DotfilesEvent::SyncReady`] when there is something to confirm.
    pub fn prepare_sync(&mut self, cx: &mut Context<Self>) {
        self.run_with(
            tr("Checking the shared version", "正在检查共享版本"),
            |chezmoi| -> Result<(u32, Overview), String> {
                let pulled = chezmoi.pull_shared().map_err(|err| err.to_string())?;
                let overview = chezmoi.overview().map_err(|err| err.to_string())?;
                Ok((pulled, overview))
            },
            |_, result, cx| match result {
                Ok((pulled, overview)) => {
                    let plan = SyncPlan::new(&overview, &[]);
                    if !plan.is_empty() {
                        cx.emit(DotfilesEvent::SyncReady(Arc::new(overview)));
                        return;
                    }
                    let mut message = if pulled > 0 {
                        tr(
                            format!("Got {pulled} new commits; nothing to update on this computer"),
                            format!("已获取 {pulled} 个新提交，这台电脑无需更新"),
                        )
                    } else {
                        tr("Everything is in sync".to_string(), "已是最新".to_string())
                    };
                    if !plan.decide.is_empty() {
                        message.push_str(&decide_note(plan.decide.len()));
                    }
                    cx.emit(DotfilesEvent::Finished(Ok(message)));
                }
                Err(err) => cx.emit(DotfilesEvent::Finished(Err(err))),
            },
            cx,
        );
    }

    /// Step two: carries out a confirmed plan.
    pub fn execute_sync(&mut self, plan: SyncPlan, cx: &mut Context<Self>) {
        self.run(
            tr("Syncing", "正在同步"),
            move |chezmoi| {
                let outcome = chezmoi.execute_sync(&plan).map_err(|err| err.to_string())?;
                describe_sync(&outcome)
            },
            cx,
        );
    }

    /// Saves this computer's version of a file to the shared version.
    pub fn keep_here(&mut self, target: String, cx: &mut Context<Self>) {
        self.run(
            tr(format!("Saving ~/{target}"), format!("正在保存 ~/{target}")),
            move |chezmoi| {
                let outcome = chezmoi
                    .re_add(std::slice::from_ref(&target))
                    .map_err(|err| err.to_string())?;
                if let Some(skipped) = outcome.skipped.first() {
                    return Err(skipped.reason.clone());
                }
                Ok(tr(
                    format!("Saved ~/{target} to the shared version. The next sync uploads it."),
                    format!("已把 ~/{target} 存到共享版本，下次同步时上传"),
                ))
            },
            cx,
        );
    }

    /// Writes the shared version to this computer. `targets` must include
    /// the directories that contain the file (see `Overview::apply_targets`);
    /// `force` replaces changes made here.
    pub fn take_shared(
        &mut self,
        target: String,
        targets: Vec<String>,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        self.run(
            tr(
                format!("Updating ~/{target}"),
                format!("正在更新 ~/{target}"),
            ),
            move |chezmoi| {
                let options = ApplyOptions {
                    force,
                    ..Default::default()
                };
                chezmoi
                    .apply(&targets, &options)
                    .map(|_| {
                        tr(
                            format!("~/{target} now matches the shared version"),
                            format!("~/{target} 已和共享版本一致"),
                        )
                    })
                    .map_err(|err| err.to_string())
            },
            cx,
        );
    }

    /// Stops syncing a file: removes it from the shared version only.
    pub fn stop_syncing(&mut self, target: String, cx: &mut Context<Self>) {
        self.run(
            tr(
                format!("Removing ~/{target}"),
                format!("正在移除 ~/{target}"),
            ),
            move |chezmoi| {
                chezmoi
                    .forget(std::slice::from_ref(&target))
                    .map(|_| {
                        tr(
                            format!("~/{target} is no longer synced. The next sync uploads this."),
                            format!("~/{target} 不再同步，下次同步时上传这个改动"),
                        )
                    })
                    .map_err(|err| err.to_string())
            },
            cx,
        );
    }

    /// Runs one setup step now.
    pub fn run_step(&mut self, target: String, cx: &mut Context<Self>) {
        self.run(
            tr(format!("Running {target}"), format!("正在运行 {target}")),
            move |chezmoi| {
                chezmoi
                    .apply(std::slice::from_ref(&target), &ApplyOptions::default())
                    .map(|_| tr(format!("Ran {target}"), format!("已运行 {target}")))
                    .map_err(|err| err.to_string())
            },
            cx,
        );
    }

    /// Applies `targets` (all when empty). `force` overwrites local edits.
    pub fn apply(&mut self, targets: Vec<String>, force: bool, cx: &mut Context<Self>) {
        let label = describe("Applying", &targets);
        self.run(
            label,
            move |chezmoi| {
                let options = ApplyOptions {
                    force,
                    ..Default::default()
                };
                chezmoi
                    .apply(&targets, &options)
                    .map(|_| match targets.as_slice() {
                        [] => "Applied all changes".to_string(),
                        [one] => format!("Applied {one}"),
                        many => format!("Applied {} files", many.len()),
                    })
                    .map_err(|err| err.to_string())
            },
            cx,
        );
    }

    /// Copies local edits back into the source state.
    pub fn re_add(&mut self, targets: Vec<String>, cx: &mut Context<Self>) {
        let label = describe("Keeping local version of", &targets);
        self.run(
            label,
            move |chezmoi| {
                let outcome = chezmoi.re_add(&targets).map_err(|err| err.to_string())?;
                if let Some(skipped) = outcome.skipped.first() {
                    return Err(skipped.reason.clone());
                }
                Ok(format!(
                    "Saved local version of {} to your dotfiles",
                    outcome.updated.join(", ")
                ))
            },
            cx,
        );
    }

    pub fn forget(&mut self, target: String, cx: &mut Context<Self>) {
        let label = format!("Removing {target}");
        self.run(
            label,
            move |chezmoi| {
                chezmoi
                    .forget(std::slice::from_ref(&target))
                    .map(|_| format!("Stopped managing {target}"))
                    .map_err(|err| err.to_string())
            },
            cx,
        );
    }

    pub fn add(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        let label = describe("Adding", &paths);
        self.run(
            label,
            move |chezmoi| {
                chezmoi
                    .add(&paths, &AddOptions::default())
                    .map(|_| format!("Added {}", paths.join(", ")))
                    .map_err(|err| err.to_string())
            },
            cx,
        );
    }

    /// Stages every change in the source repository and commits it.
    pub fn commit(&mut self, message: String, cx: &mut Context<Self>) {
        self.run_then(
            "Committing",
            move |chezmoi| {
                let hash = chezmoi
                    .git()
                    .and_then(|git| git.commit_all(&message))
                    .map_err(|err| err.to_string())?;
                Ok(format!("Committed {}", &hash[..hash.len().min(7)]))
            },
            Some(DotfilesEvent::Committed),
            cx,
        );
    }

    pub fn fetch(&mut self, cx: &mut Context<Self>) {
        self.run(
            "Fetching",
            |chezmoi| {
                let git = chezmoi.git().map_err(|err| err.to_string())?;
                git.fetch().map_err(|err| err.to_string())?;
                let status = git.status().map_err(|err| err.to_string())?;
                Ok(match status.behind {
                    0 => "No new commits on the remote".to_string(),
                    1 => "1 new commit to pull".to_string(),
                    n => format!("{n} new commits to pull"),
                })
            },
            cx,
        );
    }

    /// Pulls into the source directory. Nothing is applied: incoming
    /// changes show up on the Changes page for review.
    pub fn pull(&mut self, cx: &mut Context<Self>) {
        self.run(
            "Pulling",
            |chezmoi| {
                let git = chezmoi.git().map_err(|err| err.to_string())?;
                git.pull().map_err(|err| err.to_string())?;
                let pending = chezmoi.status(&[]).map_err(|err| err.to_string())?;
                Ok(match pending.iter().filter(|e| e.is_pending()).count() {
                    0 => "Pulled. Your home directory is up to date".to_string(),
                    n => format!("Pulled. {n} incoming changes are ready to review in Changes"),
                })
            },
            cx,
        );
    }

    pub fn push(&mut self, cx: &mut Context<Self>) {
        self.run(
            "Pushing",
            |chezmoi| {
                chezmoi
                    .git()
                    .and_then(|git| git.push())
                    .map(|_| "Pushed your dotfiles".to_string())
                    .map_err(|err| err.to_string())
            },
            cx,
        );
    }

    /// Runs `chezmoi doctor` on demand; it can be slow (see Core).
    pub fn run_doctor(&mut self, cx: &mut Context<Self>) {
        let Ok(chezmoi) = self.chezmoi.clone() else {
            return;
        };
        if matches!(self.doctor, DoctorState::Running) {
            return;
        }
        self.doctor = DoctorState::Running;
        cx.notify();
        self._doctor = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { chezmoi.doctor() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.doctor = match result {
                    Ok(checks) => DoctorState::Done(checks),
                    Err(err) => DoctorState::Failed(err.to_string()),
                };
                cx.notify();
            });
        });
    }
}

fn decide_note(count: usize) -> String {
    tr(
        if count == 1 {
            ". 1 file needs your decision.".to_string()
        } else {
            format!(". {count} files need your decision.")
        },
        format!("。还有 {count} 个文件需要你决定。"),
    )
}

/// The notification for a finished sync. A failed upload is an error: the
/// work on this computer is done, but the other computers do not have it.
fn describe_sync(outcome: &SyncOutcome) -> Result<String, String> {
    let (saved, updated, ran) = (
        outcome.saved.len(),
        outcome.updated.len(),
        outcome.ran.len(),
    );
    let mut parts = Vec::new();
    if saved > 0 {
        parts.push(tr(format!("saved {saved}"), format!("保存 {saved} 个")));
    }
    if updated > 0 {
        parts.push(tr(
            format!("updated {updated}"),
            format!("更新 {updated} 个"),
        ));
    }
    if ran > 0 {
        parts.push(tr(
            if ran == 1 {
                "ran 1 setup step".to_string()
            } else {
                format!("ran {ran} setup steps")
            },
            format!("运行 {ran} 个设置步骤"),
        ));
    }
    if outcome.pushed {
        parts.push(tr("uploaded".to_string(), "已上传".to_string()));
    }
    let mut message = if parts.is_empty() {
        tr("Synced".to_string(), "同步完成".to_string())
    } else {
        tr(
            format!("Synced: {}", parts.join(", ")),
            format!("同步完成：{}", parts.join("、")),
        )
    };
    if !outcome.decide.is_empty() {
        message.push_str(&decide_note(outcome.decide.len()));
    }
    match &outcome.push_error {
        Some(err) => Err(tr(
            format!("{message}. Uploading failed, sync again later: {err}"),
            format!("{message}。但上传失败，请稍后再同步一次：{err}"),
        )),
        None => Ok(message),
    }
}

fn describe(verb: &str, targets: &[String]) -> String {
    match targets {
        [] => format!("{verb} all changes"),
        [one] => format!("{verb} {one}"),
        many => format!("{verb} {} files", many.len()),
    }
}

fn load(chezmoi: &Chezmoi) -> Result<Snapshot, String> {
    let info = chezmoi.info().map_err(|err| err.to_string())?;
    if !info.source_exists {
        return Err(format!(
            "The chezmoi source directory {} does not exist. Set up chezmoi with `chezmoi init` first.",
            info.paths.source_dir.display()
        ));
    }
    let managed = chezmoi.managed().map_err(|err| err.to_string())?;
    let all_status = chezmoi
        .status_with(&managed)
        .map_err(|err| err.to_string())?;
    let repo = info.source_is_git_repo.then(|| load_repo(chezmoi));
    let repo_status = match &repo {
        Some(Ok(repo)) => Some(&repo.status),
        _ => None,
    };
    let overview = chezmoi
        .overview_from(&all_status, &managed, repo_status)
        .map_err(|err| err.to_string())?;
    let (status, every_apply): (Vec<_>, Vec<_>) =
        all_status.into_iter().partition(StatusEntry::is_pending);
    let managed_index = managed
        .iter()
        .enumerate()
        .map(|(ix, entry)| (entry.target.clone(), ix))
        .collect();

    let (diffs, diff_error) = if status.is_empty() {
        (HashMap::new(), None)
    } else {
        match chezmoi.diff(&[], &DiffOptions::default()) {
            Ok(patch) => match DiffFile::parse_with(&patch, DiffParseOptions::new()) {
                Ok(files) => (
                    files
                        .into_iter()
                        .map(|file| (file.path().clone(), file))
                        .collect(),
                    None,
                ),
                Err(err) => (
                    HashMap::new(),
                    Some(format!("Could not read the diff: {err}")),
                ),
            },
            Err(err) => (HashMap::new(), Some(err.to_string())),
        }
    };

    let changed_here: Vec<String> = overview
        .files
        .iter()
        .filter(|f| f.state == FileState::ChangedHere)
        .map(|f| f.target.clone())
        .collect();
    let local_diffs = if changed_here.is_empty() {
        HashMap::new()
    } else {
        let options = DiffOptions {
            reverse: true,
            ..Default::default()
        };
        chezmoi
            .diff(&changed_here, &options)
            .ok()
            .and_then(|patch| DiffFile::parse_with(&patch, DiffParseOptions::new()).ok())
            .map(|files| {
                files
                    .into_iter()
                    .map(|file| (file.path().clone(), file))
                    .collect()
            })
            .unwrap_or_default()
    };

    Ok(Snapshot {
        info,
        status,
        every_apply,
        managed,
        managed_index,
        diffs,
        diff_error,
        repo,
        overview,
        local_diffs,
    })
}

fn load_repo(chezmoi: &Chezmoi) -> Result<Repo, String> {
    let git = chezmoi.git().map_err(|err| err.to_string())?;
    let status = git.status().map_err(|err| err.to_string())?;
    let log = git.log(HISTORY_LENGTH).map_err(|err| err.to_string())?;
    let (diffs, diff_error) = if status.is_clean() {
        (Vec::new(), None)
    } else {
        match git.diff() {
            Ok(patch) => match DiffFile::parse_with(&patch, DiffParseOptions::new()) {
                Ok(files) => (files, None),
                Err(err) => (Vec::new(), Some(format!("Could not read the diff: {err}"))),
            },
            Err(err) => (Vec::new(), Some(err.to_string())),
        }
    };
    Ok(Repo {
        status,
        log,
        diffs,
        diff_error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chezmate_core::ApplyOptions;
    use std::fs;

    /// chezmoi's diff includes directories (header only), symlinks, scripts
    /// and templates; all of them must parse and line up with status paths.
    #[test]
    fn diff_of_every_entry_kind_parses() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        let (home, source, config) = (base.join("home"), base.join("source"), base.join("config"));
        for dir in [&home, &source, &config, &source.join("private_dot_ssh")] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(config.join("chezmoi.toml"), "").unwrap();
        fs::write(source.join("dot_hello"), "hello\n").unwrap();
        fs::write(source.join("dot_profile.tmpl"), "os={{ .chezmoi.os }}\n").unwrap();
        fs::write(source.join("private_dot_ssh/config"), "Host *\n").unwrap();
        fs::write(source.join("symlink_dot_link"), ".hello\n").unwrap();
        fs::write(source.join("run_once_setup.sh"), "#!/bin/sh\ntrue\n").unwrap();
        fs::write(source.join("run_after_always.sh"), "#!/bin/sh\ntrue\n").unwrap();
        fs::write(source.join("executable_dot_tool"), "#!/bin/sh\n").unwrap();
        fs::write(source.join("empty_dot_keep"), "").unwrap();
        let chezmoi = Chezmoi::new(ChezmoiOptions {
            source: Some(source.clone()),
            destination: Some(home.clone()),
            config: Some(config.join("chezmoi.toml")),
            persistent_state: Some(config.join("state.boltdb")),
            env: vec![("HOME".into(), home.clone().into())],
            ..Default::default()
        })
        .unwrap();

        let snapshot = load(&chezmoi).unwrap();
        assert_eq!(snapshot.diff_error, None);
        for entry in &snapshot.status {
            assert!(
                snapshot.diff(&entry.path).is_some(),
                "no diff for {}",
                entry.path
            );
        }

        // Modified, deleted-locally and mode-changed targets.
        chezmoi.apply(&[], &ApplyOptions::default()).unwrap();
        fs::write(source.join("dot_hello"), "hello\nmore\n").unwrap();
        fs::remove_file(home.join(".profile")).unwrap();
        fs::rename(source.join("executable_dot_tool"), source.join("dot_tool")).unwrap();
        let snapshot = load(&chezmoi).unwrap();
        assert_eq!(snapshot.diff_error, None);
        // run_once ran; the every-apply script is never pending.
        assert!(
            snapshot.status.iter().all(|e| !e.is_script()),
            "{:?}",
            snapshot.status
        );
        assert!(snapshot.diff(".hello").is_some_and(|d| d.additions() == 1));
        assert!(snapshot.diff(".profile").is_some());
    }

    /// Read-only check against the developer's real chezmoi setup. Prints
    /// counts only, never file contents. Run with
    /// `cargo test -p chezmate-app real_setup -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_setup_loads() {
        let chezmoi = Chezmoi::new(ChezmoiOptions::default()).unwrap();
        let snapshot = load(&chezmoi).unwrap();
        println!(
            "status {} · managed {} · diffs {} · diff error {:?}",
            snapshot.status.len(),
            snapshot.managed.len(),
            snapshot.diffs.len(),
            snapshot.diff_error
        );
        for entry in &snapshot.status {
            println!(
                "{} has diff: {}",
                entry.path,
                snapshot.diff(&entry.path).is_some()
            );
        }
        assert_eq!(snapshot.diff_error, None);
    }
}

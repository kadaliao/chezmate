//! The app's view of chezmoi: one retained entity that loads state on a
//! background thread and runs operations one at a time.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chezmate_core::{
    AddOptions, ApplyOptions, Chezmoi, ChezmoiOptions, Commit, ConfigAnswer, ConfigQuestion,
    DiffOptions, DoctorCheck, Error, FileState, GitHub, Info, ManagedEntry, Overview,
    RemoteProblem, RepoStatus, StatusEntry, Suggestion, SyncOutcome, SyncPlan, is_pull_conflict,
    remote_problem,
};
use gpui_kit::component::diff::{DiffFile, DiffParseOptions};
use gpui_kit::{Context, EventEmitter, SharedString, Task};

use crate::i18n::tr;
use crate::language::language_for;

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
    /// Common dotfiles on this computer, offered while nothing is synced.
    pub suggestions: Vec<Suggestion>,
}

impl Snapshot {
    /// Nothing is synced yet and nothing waits: time to pick files.
    pub fn is_empty(&self) -> bool {
        self.overview.managed_files == 0
            && self.overview.files.is_empty()
            && self.overview.setup_steps.is_empty()
    }

    /// Files are synced, but only on this computer: not a git repository,
    /// or one without any remote. A repository whose state could not be
    /// read does not count.
    pub fn needs_remote(&self) -> bool {
        self.overview.managed_files > 0
            && match &self.repo {
                None => true,
                Some(Ok(repo)) => repo.remotes.is_empty(),
                Some(Err(_)) => false,
            }
    }
}

/// What this computer still needs before the Overview makes sense.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Setup {
    /// chezmoi is not installed.
    NoChezmoi(String),
    /// chezmoi has never been used here: no shared version yet.
    Start { source_dir: PathBuf },
    /// The shared version's config template has questions to answer.
    Questions(Vec<ConfigQuestion>),
}

/// Whether the GitHub CLI can create a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitHubState {
    Unknown,
    Checking,
    /// `gh` is not installed, or not logged in.
    Unavailable,
    LoggedIn(String),
}

enum Loaded {
    Ready(Box<Snapshot>),
    Setup(Setup),
}

/// Local git state of the source directory (no network access).
pub struct Repo {
    pub status: RepoStatus,
    /// Names of the configured remotes.
    pub remotes: Vec<String>,
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

    /// The diff to show for a file: for a file changed here, from the shared
    /// version to this computer; otherwise what a sync would do here.
    pub fn review_diff(&self, target: &str) -> Option<&DiffFile> {
        let changed_here = self
            .overview
            .file(target)
            .is_some_and(|f| f.state == FileState::ChangedHere);
        changed_here
            .then(|| self.local_diff(target))
            .flatten()
            .or_else(|| self.diff(target))
    }
}

/// Step one of a sync, done: what to show for confirmation.
pub struct Prepared {
    pub overview: Overview,
    /// The remote could not be reached; the sync can still cover this
    /// computer, but uploading will fail.
    pub offline: Option<String>,
}

/// The outcome of an operation, shown as a notification.
pub enum DotfilesEvent {
    Finished(Result<String, String>),
    /// A commit succeeded; the message input can be cleared.
    Committed,
    /// The shared version now has a remote.
    Connected,
    /// The shared version is up to date; the sync runs once someone
    /// confirms the plan made from this overview.
    SyncReady(Arc<Prepared>),
}

pub enum DoctorState {
    Idle,
    Running,
    Done(Vec<DoctorCheck>),
    Failed(String),
}

pub struct Dotfiles {
    options: ChezmoiOptions,
    chezmoi: Result<Chezmoi, String>,
    snapshot: Option<Arc<Snapshot>>,
    setup: Option<Setup>,
    /// Why the last setup step failed, shown on the setup page.
    setup_error: Option<String>,
    /// Questions chezmoi asked that the template did not reveal.
    asked: Vec<ConfigQuestion>,
    github: GitHubState,
    /// Why connecting to a remote failed, shown in the connect dialog.
    connect_error: Option<String>,
    loading: bool,
    error: Option<String>,
    busy: Option<SharedString>,
    doctor: DoctorState,
    revision: usize,
    _refresh: Task<()>,
    _operation: Task<()>,
    _doctor: Task<()>,
    _github: Task<()>,
}

impl EventEmitter<DotfilesEvent> for Dotfiles {}

impl Dotfiles {
    pub fn new(options: ChezmoiOptions, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            chezmoi: Chezmoi::new(options.clone()).map_err(|err| err.to_string()),
            options,
            snapshot: None,
            setup: None,
            setup_error: None,
            asked: Vec::new(),
            github: GitHubState::Unknown,
            connect_error: None,
            loading: false,
            error: None,
            busy: None,
            doctor: DoctorState::Idle,
            revision: 0,
            _refresh: Task::ready(()),
            _operation: Task::ready(()),
            _doctor: Task::ready(()),
            _github: Task::ready(()),
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

    /// What this computer needs before the Overview, if anything.
    pub fn setup(&self) -> Option<&Setup> {
        self.setup.as_ref()
    }

    pub fn setup_error(&self) -> Option<&str> {
        self.setup_error.as_deref()
    }

    pub fn github(&self) -> &GitHubState {
        &self.github
    }

    pub fn connect_error(&self) -> Option<&str> {
        self.connect_error.as_deref()
    }

    /// Sets the GitHub CLI's state without asking it, for screenshots.
    #[allow(dead_code)]
    pub fn set_github(&mut self, state: GitHubState, cx: &mut Context<Self>) {
        self.github = state;
        cx.notify();
    }

    /// Reloads status, managed files and the pending diff. Keeps the previous
    /// snapshot visible until the new one arrives; a newer refresh discards
    /// the results of an older one.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        // chezmoi may have been installed since the last look.
        if self.chezmoi.is_err() {
            self.chezmoi = Chezmoi::new(self.options.clone()).map_err(|err| err.to_string());
        }
        let chezmoi = match &self.chezmoi {
            Ok(chezmoi) => chezmoi.clone(),
            Err(err) => {
                self.snapshot = None;
                self.setup = Some(Setup::NoChezmoi(err.clone()));
                cx.notify();
                return;
            }
        };
        let asked = self.asked.clone();
        self.revision += 1;
        let revision = self.revision;
        self.loading = true;
        cx.notify();
        self._refresh = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { load(&chezmoi, &asked) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.revision != revision {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(Loaded::Ready(snapshot)) => {
                        this.snapshot = Some(Arc::new(*snapshot));
                        this.setup = None;
                        // A setup step that failed after moving things on
                        // (a config template that could not be completed,
                        // say) is reported here, not dropped with the page.
                        if let Some(err) = this.setup_error.take() {
                            cx.emit(DotfilesEvent::Finished(Err(err)));
                        }
                        this.asked.clear();
                        this.error = None;
                    }
                    Ok(Loaded::Setup(setup)) => {
                        this.snapshot = None;
                        this.setup = Some(setup);
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
            |chezmoi| -> Result<(u32, Prepared), String> {
                // Offline, the sync can still save and update on this
                // computer. A pull that fails for another reason (a
                // conflict) stops the sync.
                let (pulled, offline) = match chezmoi.fetch_shared() {
                    Ok(0) => (0, None),
                    Ok(behind) => {
                        chezmoi.git().and_then(|git| git.pull()).map_err(|err| {
                            if is_pull_conflict(&err) {
                                tr(
                                    "Another computer and this one changed the same file in the shared version, so it cannot be merged automatically. Nothing was changed. Resolve it under Advanced › Repository or in a terminal.".to_string(),
                                    "另一台电脑和这台电脑都改了共享版本里的同一个文件，无法自动合并，什么都没有改变。请在「高级 › Repository」或终端里处理。".to_string(),
                                )
                            } else {
                                tr(
                                    format!("Could not get the shared version: {err}"),
                                    format!("获取共享版本失败：{err}"),
                                )
                            }
                        })?;
                        (behind, None)
                    }
                    Err(err) => (0, Some(first_line(&err.to_string()))),
                };
                let overview = chezmoi.overview().map_err(|err| err.to_string())?;
                Ok((pulled, Prepared { overview, offline }))
            },
            |_, result, cx| match result {
                Ok((pulled, prepared)) => {
                    let plan = SyncPlan::new(&prepared.overview, &[]);
                    if !plan.is_empty() {
                        cx.emit(DotfilesEvent::SyncReady(Arc::new(prepared)));
                        return;
                    }
                    if let Some(err) = prepared.offline {
                        cx.emit(DotfilesEvent::Finished(Err(tr(
                            format!("Could not reach the remote: {err}"),
                            format!("连不上远端：{err}"),
                        ))));
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

    /// Writes the shared version of one file to this computer, touching
    /// nothing else. `force` replaces changes made here.
    pub fn take_shared(&mut self, target: String, force: bool, cx: &mut Context<Self>) {
        self.run(
            tr(
                format!("Updating ~/{target}"),
                format!("正在更新 ~/{target}"),
            ),
            move |chezmoi| {
                chezmoi
                    .take_shared(std::slice::from_ref(&target), force)
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

    /// Writes `contents` to a file in the home or source directory, or to
    /// chezmoi's config file, creating its folder if needed. `shown` names
    /// the file in notifications.
    pub fn save_file(
        &mut self,
        path: PathBuf,
        contents: String,
        shown: String,
        cx: &mut Context<Self>,
    ) {
        let label = tr(format!("Saving {shown}"), format!("正在保存 {shown}"));
        self.run(
            label,
            move |_| {
                path.parent()
                    .map_or(Ok(()), std::fs::create_dir_all)
                    .and_then(|_| std::fs::write(&path, contents))
                    .map(|_| tr(format!("Saved {shown}"), format!("已保存 {shown}")))
                    .map_err(|err| {
                        tr(
                            format!("Could not save {shown}: {err}"),
                            format!("保存 {shown} 失败：{err}"),
                        )
                    })
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

    /// A fresh handle after setup steps: chezmoi's paths may come from the
    /// config file that was just made, and the handle caches them.
    fn reopen(&mut self) {
        self.chezmoi = Chezmoi::new(self.options.clone()).map_err(|err| err.to_string());
    }

    /// Runs a setup step; a failure is shown on the setup page.
    fn run_setup(
        &mut self,
        label: impl Into<SharedString>,
        operation: impl FnOnce(&Chezmoi) -> Result<(), String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.setup_error = None;
        self.run_with(
            label,
            operation,
            |this, result, _| {
                this.setup_error = result.err();
                this.reopen();
            },
            cx,
        );
    }

    /// Starts a new shared version on this computer.
    pub fn start_new(&mut self, cx: &mut Context<Self>) {
        self.run_setup(
            tr("Setting up", "正在设置"),
            |chezmoi| chezmoi.start_new().map_err(|err| err.to_string()),
            cx,
        );
    }

    /// Fetches the shared version from `repo` (a URL or a GitHub user or
    /// `user/repo`). Nothing in the home directory changes.
    pub fn start_from(&mut self, repo: String, cx: &mut Context<Self>) {
        self.setup_error = None;
        self.run_with(
            tr("Getting the shared version", "正在取回共享版本"),
            move |chezmoi| -> Result<Option<ConfigQuestion>, String> {
                let ssh = GitHub::find(chezmoi.search_path(), &chezmoi.options().env)
                    .is_some_and(|gh| gh.prefers_ssh());
                chezmoi
                    .start_from(&repo, ssh)
                    .map_err(|err| remote_message(&err))?;
                // Without questions in sight, make the config right away;
                // chezmoi may still ask one whose text the template computes.
                if chezmoi.needs_config().unwrap_or(false)
                    && chezmoi.config_questions().is_ok_and(|q| q.is_empty())
                {
                    return chezmoi.init_config(&[]).map_err(|err| {
                        tr(
                            format!("Got the shared version, but chezmoi could not make this computer's settings from its template: {err}"),
                            format!("已取回共享版本，但 chezmoi 无法用其中的模板生成这台电脑的设置：{err}"),
                        )
                    });
                }
                Ok(None)
            },
            |this, result, _| {
                match result {
                    Ok(Some(question)) => this.asked.push(question),
                    Ok(None) => {}
                    Err(err) => this.setup_error = Some(err),
                }
                this.reopen();
            },
            cx,
        );
    }

    /// Makes the config file from the template with these answers.
    pub fn answer_config(&mut self, answers: Vec<ConfigAnswer>, cx: &mut Context<Self>) {
        self.setup_error = None;
        self.run_with(
            tr("Saving the settings", "正在保存设置"),
            move |chezmoi| chezmoi.init_config(&answers),
            |this, result, _| {
                match result {
                    Ok(None) => {}
                    Ok(Some(question)) => {
                        this.setup_error = Some(tr(
                            format!("One more question: {}", question.prompt),
                            format!("还有一个问题：{}", question.prompt),
                        ));
                        if !this.asked.iter().any(|q| q.prompt == question.prompt) {
                            this.asked.push(question);
                        }
                    }
                    Err(err) => this.setup_error = Some(err.to_string()),
                }
                this.reopen();
            },
            cx,
        );
    }

    /// Leaves the config file empty instead of answering the template.
    pub fn skip_config(&mut self, cx: &mut Context<Self>) {
        self.run_setup(
            tr("Saving the settings", "正在保存设置"),
            |chezmoi| chezmoi.skip_config().map_err(|err| err.to_string()),
            cx,
        );
    }

    /// Starts syncing files that are on this computer.
    pub fn start_syncing(&mut self, targets: Vec<String>, cx: &mut Context<Self>) {
        if targets.is_empty() {
            return;
        }
        self.run(
            tr("Adding files", "正在添加文件"),
            move |chezmoi| {
                chezmoi
                    .add(&targets, &AddOptions::default())
                    .map_err(|err| err.to_string())?;
                Ok(tr(
                    if targets.len() == 1 {
                        "Added 1 file to the shared version".to_string()
                    } else {
                        format!("Added {} files to the shared version", targets.len())
                    },
                    format!("已把 {} 项加入共享版本", targets.len()),
                ))
            },
            cx,
        );
    }

    /// Checks whether the GitHub CLI is installed and logged in. Once it
    /// is, the answer is kept; otherwise each call looks again (it may have
    /// been installed or logged in since).
    pub fn check_github(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.github,
            GitHubState::Checking | GitHubState::LoggedIn(_)
        ) {
            return;
        }
        let Ok(chezmoi) = self.chezmoi.clone() else {
            return;
        };
        self.github = GitHubState::Checking;
        cx.notify();
        self._github = cx.spawn(async move |this, cx| {
            let state = cx
                .background_executor()
                .spawn(async move {
                    GitHub::find(chezmoi.search_path(), &chezmoi.options().env)
                        .and_then(|gh| gh.account().ok().flatten())
                        .map_or(GitHubState::Unavailable, GitHubState::LoggedIn)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.github = state;
                cx.notify();
            });
        });
    }

    /// Connects the shared version to an empty repository at `url` and
    /// uploads it.
    pub fn connect(&mut self, url: String, cx: &mut Context<Self>) {
        self.run_connect(
            tr("Uploading", "正在上传"),
            move |chezmoi| chezmoi.connect_shared(&url),
            cx,
        );
    }

    /// Creates a private GitHub repository called `name`, then connects to
    /// it and uploads.
    pub fn create_github_repo(&mut self, name: String, cx: &mut Context<Self>) {
        self.run_connect(
            tr("Creating the repository", "正在创建仓库"),
            move |chezmoi| {
                let gh = GitHub::find(chezmoi.search_path(), &chezmoi.options().env).ok_or_else(
                    || Error::InvalidRequest("the GitHub CLI (gh) is not installed".into()),
                )?;
                let url = gh.create_private_repo(&name).map_err(|err| {
                    if err.stderr().is_some_and(|e| e.contains("already exists")) {
                        Error::InvalidRequest(tr(
                            format!("Your GitHub account already has a repository called {name}. Choose another name; if you created it just now, paste its address below instead."),
                            format!("你的 GitHub 账号里已经有叫 {name} 的仓库。请换个名字；如果是刚才创建的，把它的地址粘贴到下面即可。"),
                        ))
                    } else {
                        err
                    }
                })?;
                chezmoi.connect_shared(&url)
            },
            cx,
        );
    }

    fn run_connect(
        &mut self,
        label: impl Into<SharedString>,
        operation: impl FnOnce(&Chezmoi) -> chezmate_core::Result<()> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.connect_error = None;
        self.run_with(
            label,
            move |chezmoi| operation(chezmoi).map_err(|err| remote_message(&err)),
            |this, result, cx| match result {
                Ok(()) => {
                    cx.emit(DotfilesEvent::Finished(Ok(tr(
                        "Connected. Your other computers can now get these files.".to_string(),
                        "已连接并上传，其他电脑现在可以取回这些文件了".to_string(),
                    ))));
                    cx.emit(DotfilesEvent::Connected);
                }
                Err(err) => this.connect_error = Some(err),
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

/// The gist of a failed command: its first line of output, without the
/// "`git fetch` failed (exit 128): " prefix.
fn first_line(message: &str) -> String {
    let line = message.lines().next().unwrap_or_default();
    line.split_once("): ")
        .map_or(line, |(_, rest)| rest)
        .trim()
        .to_string()
}

/// A failed clone or upload, explained.
fn remote_message(err: &Error) -> String {
    // Git's own explanation, not the "Cloning into …" line before it.
    let detail = err
        .stderr()
        .and_then(|stderr| {
            stderr.lines().map(str::trim).find(|line| {
                ["fatal:", "error:", "ERROR", "remote:"]
                    .iter()
                    .any(|p| line.starts_with(p))
                    || line.contains("Permission denied")
                    || line.contains("[rejected]")
            })
        })
        .map(str::to_string)
        .unwrap_or_else(|| first_line(&err.to_string()));
    let hint = match remote_problem(err) {
        Some(RemoteProblem::NotFound) => tr(
            "The repository was not found, or this computer has no access to it. Check the address; for a private repository, log in to GitHub on this computer first (for example with `gh auth login`).",
            "找不到这个仓库，或者这台电脑没有权限访问。请检查地址；私有仓库需要先在这台电脑上登录 GitHub（例如运行 `gh auth login`）。",
        ),
        Some(RemoteProblem::AccessDenied) => tr(
            "The server refused access. Check that this computer's SSH key or login is added to your account.",
            "服务器拒绝访问。请确认这台电脑的 SSH 密钥或登录信息已经加到你的账号里。",
        ),
        Some(RemoteProblem::Offline) => tr(
            "Could not reach the server. Check the network and try again.",
            "连不上服务器，请检查网络后再试。",
        ),
        Some(RemoteProblem::NotEmpty) => tr(
            "This repository already has files. Use an empty repository, or start this computer from that repository instead.",
            "这个仓库里已经有内容。请换一个空仓库；如果它就是你在别的电脑上的配置，应该用它来开始。",
        ),
        None => {
            return match err {
                Error::InvalidRequest(message) => message.clone(),
                _ => err.to_string(),
            };
        }
    };
    format!("{hint} ({detail})")
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

fn load(chezmoi: &Chezmoi, asked: &[ConfigQuestion]) -> Result<Loaded, String> {
    let info = chezmoi.info().map_err(|err| err.to_string())?;
    if !info.source_exists {
        // With a config file, chezmoi was set up here and the shared
        // version has gone missing (moved, or on a drive that is not
        // connected). Starting over would replace that config.
        if info.config_exists {
            let (source, config) = (
                info.paths.source_dir.display(),
                info.paths.config_file.display(),
            );
            return Err(tr(
                format!(
                    "The shared version's folder {source} is missing, but chezmoi is set up on this computer ({config}). If it is on an external or network drive, connect it and press ⌘R. If it was moved, set sourceDir in the config file."
                ),
                format!(
                    "找不到共享版本的文件夹 {source}，但这台电脑上已经设置过 chezmoi（{config}）。如果它在外接硬盘或网络盘上，请连接后按 ⌘R 刷新；如果它被移动了，请在配置文件里修改 sourceDir。"
                ),
            ));
        }
        return Ok(Loaded::Setup(Setup::Start {
            source_dir: info.paths.source_dir.clone(),
        }));
    }
    // A template with questions and no config made from it yet. One with
    // nothing to ask does not hold anything up.
    if info.config_template.is_some() && !info.config_exists {
        let mut questions = chezmoi.config_questions().map_err(|err| err.to_string())?;
        for question in asked {
            if !questions.iter().any(|q| q.prompt == question.prompt) {
                questions.push(question.clone());
            }
        }
        if !questions.is_empty() {
            return Ok(Loaded::Setup(Setup::Questions(questions)));
        }
    }
    load_snapshot(chezmoi, info).map(|snapshot| Loaded::Ready(Box::new(snapshot)))
}

fn load_snapshot(chezmoi: &Chezmoi, info: Info) -> Result<Snapshot, String> {
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
                Ok(files) => (by_path(files), None),
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
            .map(by_path)
            .unwrap_or_default()
    };

    let suggestions = if overview.managed_files == 0 {
        chezmoi.suggestions(&managed).unwrap_or_default()
    } else {
        Vec::new()
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
        suggestions,
    })
}

/// Diffs by target path, each highlighted in its file's language.
fn by_path(files: Vec<DiffFile>) -> HashMap<SharedString, DiffFile> {
    files
        .into_iter()
        .map(|file| {
            let language = language_for(file.path(), "");
            (file.path().clone(), file.with_language(language))
        })
        .collect()
}

fn load_repo(chezmoi: &Chezmoi) -> Result<Repo, String> {
    let git = chezmoi.git().map_err(|err| err.to_string())?;
    let status = git.status().map_err(|err| err.to_string())?;
    let remotes = git.remotes().map_err(|err| err.to_string())?;
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
        remotes,
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

    fn ready(chezmoi: &Chezmoi) -> Snapshot {
        match load(chezmoi, &[]).unwrap() {
            Loaded::Ready(snapshot) => *snapshot,
            Loaded::Setup(setup) => panic!("setup needed: {setup:?}"),
        }
    }

    #[test]
    fn first_line_drops_the_command() {
        assert_eq!(
            first_line(
                "`git fetch --quiet` failed (exit 128): ssh: Could not resolve hostname github.com\nfatal: Could not read"
            ),
            "ssh: Could not resolve hostname github.com"
        );
        assert_eq!(first_line("plain"), "plain");
    }

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

        let snapshot = ready(&chezmoi);
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
        let snapshot = ready(&chezmoi);
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
        let snapshot = ready(&chezmoi);
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

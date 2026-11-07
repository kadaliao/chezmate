//! The simplified model: configuration files kept in sync between this
//! computer and the shared version (the chezmoi source directory and the git
//! remote behind it).
//!
//! Every file that differs has exactly one [`FileState`]. A [`SyncPlan`] says
//! what one sync does; [`Chezmoi::execute_sync`] carries it out. chezmoi is
//! still the engine: saving is `re-add`, updating is `apply`, uploading is a
//! commit and a push.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::chezmoi::{ApplyOptions, Chezmoi, Skipped, Written};
use crate::error::Result;
use crate::git::{Git, RepoStatus};
use crate::managed::{EntryKind, ManagedEntry};
use crate::status::{Change, StatusEntry};

/// How one file differs between this computer and the shared version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileState {
    /// Changed on this computer; the shared version did not change. A sync
    /// saves the change to the shared version.
    ChangedHere,
    /// The shared version changed, usually on another computer. A sync
    /// updates this computer.
    UpdateAvailable,
    /// Changed on this computer and in the shared version, or this computer
    /// has its own version that was never synced. Needs a decision.
    BothChanged,
    /// Deleted on this computer. Needs a decision: restore it or stop
    /// syncing it.
    DeletedHere,
}

/// Why a change on this computer cannot be saved to the shared version
/// automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveBlocker {
    /// Generated from a template, often with different contents on each
    /// computer. Edit the template instead.
    Template,
    /// Produced by a `modify_` script or a `remove_` entry.
    Script,
    /// A directory; only the files in it are saved.
    Directory,
}

/// A target that differs between this computer and the shared version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileItem {
    /// Path relative to the home (destination) directory.
    pub target: String,
    pub state: FileState,
    /// What updating from the shared version does to this computer's file:
    /// `added`, `modified` or `deleted`.
    pub incoming: Change,
    pub kind: EntryKind,
    pub template: bool,
    pub encrypted: bool,
    /// Set when this computer's version cannot be saved automatically.
    pub cannot_save: Option<SaveBlocker>,
    /// This computer has its own version, which chezmoi never wrote (typical
    /// on a newly set up computer). Updating would replace it.
    pub never_synced: bool,
}

impl FileItem {
    /// A sync leaves the file alone until someone decides what to keep.
    pub fn needs_decision(&self) -> bool {
        match self.state {
            FileState::BothChanged | FileState::DeletedHere => true,
            FileState::ChangedHere => self.cannot_save.is_some(),
            FileState::UpdateAvailable => false,
        }
    }

    /// This computer's version can be saved to the shared version.
    pub fn can_save(&self) -> bool {
        self.cannot_save.is_none() && self.state != FileState::DeletedHere
    }

    /// A directory that is missing here, so the files in it can only be
    /// written after it.
    fn must_exist_first(&self) -> bool {
        match self.state {
            FileState::UpdateAvailable => self.incoming == Change::Added,
            FileState::DeletedHere => true,
            _ => false,
        }
    }
}

/// When chezmoi runs a script.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptRuns {
    /// `run_once_`: once per computer.
    Once,
    /// `run_onchange_`: again whenever its contents change.
    OnChange,
    /// Plain `run_`: on every apply.
    Always,
}

/// Whether a script runs before, together with, or after the files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptOrder {
    Before,
    During,
    After,
}

/// A script that chezmoi would run now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SetupStep {
    pub target: String,
    pub runs: ScriptRuns,
    pub order: ScriptOrder,
}

impl SetupStep {
    fn new(target: &str, managed: Option<&ManagedEntry>) -> Self {
        let attributes = managed.map(|m| m.attributes.clone()).unwrap_or_default();
        Self {
            target: target.to_string(),
            runs: if attributes.once {
                ScriptRuns::Once
            } else if attributes.onchange {
                ScriptRuns::OnChange
            } else {
                ScriptRuns::Always
            },
            order: if attributes.before {
                ScriptOrder::Before
            } else if attributes.after {
                ScriptOrder::After
            } else {
                ScriptOrder::During
            },
        }
    }
}

/// The git repository that holds the shared version.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SharedStatus {
    pub branch: Option<String>,
    /// `None`: the shared version exists only on this computer.
    pub upstream: Option<String>,
    /// Commits on this computer not uploaded yet.
    pub ahead: u32,
    /// Commits from elsewhere not included yet, as of the last fetch.
    pub behind: u32,
    /// Uncommitted changes in the source directory, uploaded by the next
    /// sync.
    pub uncommitted: usize,
}

impl SharedStatus {
    fn from_repo(repo: &RepoStatus) -> Self {
        Self {
            branch: repo.branch.clone(),
            upstream: repo.upstream.clone(),
            ahead: repo.ahead,
            behind: repo.behind,
            uncommitted: repo.changes.len(),
        }
    }

    /// Something on this computer waits to be uploaded.
    pub fn has_upload(&self) -> bool {
        self.uncommitted > 0 || (self.upstream.is_some() && self.ahead > 0)
    }
}

/// Everything a person needs to see: which files differ and how, setup
/// steps, and the state of the shared version.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Overview {
    /// Files that differ, sorted by target.
    pub files: Vec<FileItem>,
    /// Directories that change together with files listed inside them. They
    /// are applied along with those files and not shown on their own.
    pub directories: Vec<FileItem>,
    /// `run_once_` and `run_onchange_` scripts waiting to run.
    pub setup_steps: Vec<SetupStep>,
    /// Scripts chezmoi runs on every apply. A sync runs them when it updates
    /// this computer; they are never pending on their own.
    pub every_apply: Vec<SetupStep>,
    /// `None` when the source directory is not a git repository.
    pub shared: Option<SharedStatus>,
    /// Managed targets that are not directories or scripts.
    pub managed_files: usize,
}

impl Overview {
    pub fn file(&self, target: &str) -> Option<&FileItem> {
        self.files.iter().find(|f| f.target == target)
    }

    /// Nothing differs and nothing waits to be uploaded or pulled.
    pub fn is_synced(&self) -> bool {
        self.files.is_empty()
            && self.setup_steps.is_empty()
            && self
                .shared
                .as_ref()
                .is_none_or(|s| !s.has_upload() && s.behind == 0)
    }

    /// Files a sync leaves alone until someone decides.
    pub fn decisions(&self) -> impl Iterator<Item = &FileItem> {
        self.files.iter().filter(|f| f.needs_decision())
    }

    /// `targets` plus the directories containing them that do not exist on
    /// this computer yet (new, or deleted here). chezmoi cannot write a file
    /// into a missing directory, so those must be applied in the same run,
    /// and without recursing: applying a directory recursively would also
    /// overwrite every other file in it.
    pub fn apply_targets(&self, targets: &[String]) -> Vec<String> {
        let directories: Vec<&str> = self
            .directories
            .iter()
            .chain(&self.files)
            .filter(|item| item.kind == EntryKind::Directory && item.must_exist_first())
            .map(|item| item.target.as_str())
            .collect();
        let mut all = BTreeSet::new();
        for target in targets {
            for dir in &directories {
                if is_inside(target, dir) {
                    all.insert(dir.to_string());
                }
            }
            all.insert(target.clone());
        }
        all.into_iter().collect()
    }
}

fn is_inside(path: &str, dir: &str) -> bool {
    path.len() > dir.len() && path.starts_with(dir) && path.as_bytes()[dir.len()] == b'/'
}

/// What one sync does.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SyncPlan {
    /// Changes on this computer to save to the shared version.
    pub save: Vec<String>,
    /// Files to update on this computer from the shared version.
    pub update: Vec<String>,
    /// Setup steps to run.
    pub run: Vec<String>,
    /// Scripts that run because this sync updates this computer.
    pub every_apply: Vec<String>,
    /// Changes on this computer left out of this sync on request.
    pub skipped: Vec<String>,
    /// Files left alone until someone decides what to keep.
    pub decide: Vec<String>,
    /// Commit the shared version's changes (if any) and push them (if there
    /// is a remote).
    pub upload: bool,
}

impl SyncPlan {
    /// Everything that can be done safely, minus the saves in `skip`.
    pub fn new(overview: &Overview, skip: &[String]) -> Self {
        let skip: HashSet<&str> = skip.iter().map(String::as_str).collect();
        let mut plan = SyncPlan::default();
        for file in &overview.files {
            let target = file.target.clone();
            if file.needs_decision() {
                plan.decide.push(target);
            } else if file.state == FileState::ChangedHere {
                if skip.contains(target.as_str()) {
                    plan.skipped.push(target);
                } else {
                    plan.save.push(target);
                }
            } else if file.state == FileState::UpdateAvailable {
                plan.update.push(target);
            }
        }
        plan.run = overview
            .setup_steps
            .iter()
            .map(|s| s.target.clone())
            .collect();
        if !plan.update.is_empty() || !plan.run.is_empty() {
            plan.every_apply = overview
                .every_apply
                .iter()
                .map(|s| s.target.clone())
                .collect();
        }
        plan.upload = overview
            .shared
            .as_ref()
            .is_some_and(|s| !plan.save.is_empty() || s.has_upload());
        plan
    }

    /// The sync would not change anything.
    pub fn is_empty(&self) -> bool {
        self.save.is_empty() && self.update.is_empty() && self.run.is_empty() && !self.upload
    }
}

/// What a sync did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SyncOutcome {
    pub saved: Vec<String>,
    pub updated: Vec<String>,
    pub ran: Vec<String>,
    /// Planned items that changed before the sync ran and were left alone.
    pub skipped: Vec<Skipped>,
    /// The commit that recorded saved changes.
    pub commit: Option<String>,
    pub pushed: bool,
    /// Everything on this computer is done, but uploading failed (offline,
    /// or the remote moved on). The next sync uploads again.
    pub push_error: Option<String>,
    /// Files still waiting for a decision.
    pub decide: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SyncOptions {
    /// Fetch and plan only.
    pub dry_run: bool,
    /// Changes on this computer not to save this time.
    pub skip: Vec<String>,
}

/// The result of [`Chezmoi::sync`]. A remote that cannot be reached is a
/// warning, not an error.
#[derive(Debug, Clone, Serialize)]
pub struct SyncReport {
    pub dry_run: bool,
    /// Commits from elsewhere. A sync pulls them before planning; a dry run
    /// only fetches, so the plan does not include them yet.
    pub incoming_commits: u32,
    pub plan: SyncPlan,
    /// `None` for a dry run.
    pub outcome: Option<SyncOutcome>,
    pub warnings: Vec<String>,
}

/// How the shared version relates to what chezmoi last wrote to a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Since {
    /// chezmoi has no record of writing the target.
    NeverWritten,
    /// The shared version still renders to what chezmoi last wrote.
    Unchanged,
    /// The shared version changed since then, or it could not be compared.
    Changed,
}

fn file_state(entry: &StatusEntry, since: Since, kind: EntryKind) -> (FileState, bool) {
    match entry.local {
        Change::Deleted => (FileState::DeletedHere, false),
        Change::None => {
            // chezmoi reports a file it never wrote as only needing an
            // update (or, in an `exact_` directory, a deletion), and apply
            // then overwrites or deletes it without asking.
            let own_version = since == Since::NeverWritten
                && match entry.apply {
                    Change::Modified => matches!(kind, EntryKind::File | EntryKind::Symlink),
                    // `remove_` entries are meant to delete what is there.
                    Change::Deleted => kind != EntryKind::Remove,
                    _ => false,
                };
            if own_version {
                (FileState::BothChanged, true)
            } else {
                (FileState::UpdateAvailable, false)
            }
        }
        _ => match since {
            Since::Unchanged => (FileState::ChangedHere, false),
            Since::Changed | Since::NeverWritten => (FileState::BothChanged, false),
        },
    }
}

fn save_blocker(kind: EntryKind, managed: Option<&ManagedEntry>) -> Option<SaveBlocker> {
    match kind {
        EntryKind::Directory => Some(SaveBlocker::Directory),
        EntryKind::Modify | EntryKind::Remove | EntryKind::Script => Some(SaveBlocker::Script),
        _ if managed.is_some_and(|m| m.attributes.template) => Some(SaveBlocker::Template),
        _ => None,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl Chezmoi {
    /// The overview, reading status, managed targets and the shared
    /// repository's local state (no network access).
    pub fn overview(&self) -> Result<Overview> {
        let managed = self.managed()?;
        let status = self.status_with(&managed)?;
        let repo = self.shared_git()?.map(|git| git.status()).transpose()?;
        self.overview_from(&status, &managed, repo.as_ref())
    }

    /// The overview from already loaded state. May run `chezmoi state dump`
    /// and `chezmoi cat` to tell apart "changed here" from "changed on both
    /// sides", which `status` reports the same way.
    pub fn overview_from(
        &self,
        status: &[StatusEntry],
        managed: &[ManagedEntry],
        repo: Option<&RepoStatus>,
    ) -> Result<Overview> {
        let by_target: HashMap<&str, &ManagedEntry> =
            managed.iter().map(|m| (m.target.as_str(), m)).collect();
        let needs_record = status
            .iter()
            .any(|e| !e.is_script() && (e.has_local_changes() || e.apply == Change::Modified));
        let written = if needs_record {
            self.last_written()?
        } else {
            HashMap::new()
        };
        let dest = if needs_record {
            Some(self.paths()?.dest_dir)
        } else {
            None
        };

        let mut overview = Overview {
            shared: repo.map(SharedStatus::from_repo),
            managed_files: managed
                .iter()
                .filter(|m| !matches!(m.kind, EntryKind::Directory | EntryKind::Script))
                .count(),
            ..Default::default()
        };
        let mut items = Vec::new();
        for entry in status {
            let managed = by_target.get(entry.path.as_str()).copied();
            if entry.is_script() {
                let step = SetupStep::new(&entry.path, managed);
                if entry.runs_every_apply {
                    overview.every_apply.push(step);
                } else {
                    overview.setup_steps.push(step);
                }
                continue;
            }
            let kind = managed.map(|m| m.kind).unwrap_or(EntryKind::File);
            let record = dest
                .as_ref()
                .and_then(|dest| written.get(&*dest.join(&entry.path).to_string_lossy()));
            let since = match record {
                None => Since::NeverWritten,
                // Only a changed file needs comparing; a deleted one is
                // "deleted here" whatever the shared version did.
                Some(_) if matches!(entry.local, Change::None | Change::Deleted) => {
                    Since::Unchanged
                }
                Some(record) => self.compare(entry, kind, record),
            };
            let (state, never_synced) = file_state(entry, since, kind);
            items.push(FileItem {
                target: entry.path.clone(),
                state,
                incoming: entry.apply,
                kind,
                template: managed.is_some_and(|m| m.attributes.template),
                encrypted: managed.is_some_and(|m| m.attributes.encrypted),
                cannot_save: save_blocker(kind, managed),
                never_synced,
            });
        }
        // A new file in a folder deleted here is part of that deletion.
        let deleted_dirs: Vec<String> = items
            .iter()
            .filter(|i| i.kind == EntryKind::Directory && i.state == FileState::DeletedHere)
            .map(|i| i.target.clone())
            .collect();
        for item in &mut items {
            if item.state == FileState::UpdateAvailable
                && deleted_dirs.iter().any(|dir| is_inside(&item.target, dir))
            {
                item.state = FileState::DeletedHere;
            }
        }
        // A directory that is created or restored along with files listed
        // inside it is not shown on its own. One changed here is: it needs
        // a decision.
        let (directories, files): (Vec<_>, Vec<_>) = items.iter().cloned().partition(|item| {
            item.kind == EntryKind::Directory
                && item.must_exist_first()
                && items
                    .iter()
                    .any(|other| is_inside(&other.target, &item.target))
        });
        overview.files = files;
        overview.directories = directories;
        Ok(overview)
    }

    /// Whether the shared version still renders to what chezmoi last wrote
    /// to a target that was changed on this computer.
    fn compare(&self, entry: &StatusEntry, kind: EntryKind, record: &Written) -> Since {
        if kind == EntryKind::Directory {
            // Only its permissions can change here. Shown as changed here
            // (it cannot be saved, so it needs a decision either way).
            return Since::Unchanged;
        }
        let Some(expected) = &record.contents_sha256 else {
            return Since::Changed;
        };
        if !matches!(
            kind,
            EntryKind::File | EntryKind::Symlink | EntryKind::Modify
        ) {
            return Since::Changed;
        }
        let Ok(mut contents) = self.cat_bytes(&entry.path) else {
            return Since::Changed;
        };
        // chezmoi records a symlink's target without the newline `cat` adds.
        if record.kind == "symlink" && contents.last() == Some(&b'\n') {
            contents.pop();
        }
        if &sha256_hex(&contents) == expected {
            Since::Unchanged
        } else {
            Since::Changed
        }
    }

    /// Git for the source directory, or `None` if it is not a repository.
    fn shared_git(&self) -> Result<Option<Git>> {
        let paths = self.paths()?;
        if !paths.working_tree.join(".git").exists() {
            return Ok(None);
        }
        let git = self.git()?;
        Ok(git.is_repository().then_some(git))
    }

    /// Fetches the shared version's remote. Returns the number of commits
    /// not included yet; 0 without a repository or remote.
    pub fn fetch_shared(&self) -> Result<u32> {
        let Some(git) = self.shared_git()? else {
            return Ok(0);
        };
        if git.status()?.upstream.is_none() {
            return Ok(0);
        }
        git.fetch()?;
        Ok(git.status()?.behind)
    }

    /// Fetches, then pulls if the remote has new commits. Only the source
    /// directory changes; nothing is applied. A conflicting pull is aborted
    /// and reported as an error. Returns the number of commits pulled.
    pub fn pull_shared(&self) -> Result<u32> {
        let behind = self.fetch_shared()?;
        if behind > 0 {
            self.git()?.pull()?;
        }
        Ok(behind)
    }

    /// Writes the shared version of `targets` to this computer, creating
    /// missing directories they are in but touching nothing else in them.
    /// `force` replaces changes made here.
    pub fn take_shared(&self, targets: &[String], force: bool) -> Result<()> {
        let targets = self.overview()?.apply_targets(targets);
        let options = ApplyOptions {
            force,
            non_recursive: true,
            ..Default::default()
        };
        self.apply(&targets, &options).map(drop)
    }

    /// Carries out a plan. Every item is checked against the current state
    /// first; items that changed since the plan was made are skipped.
    pub fn execute_sync(&self, plan: &SyncPlan) -> Result<SyncOutcome> {
        self.execute_with(plan, &self.overview()?)
    }

    fn execute_with(&self, plan: &SyncPlan, current: &Overview) -> Result<SyncOutcome> {
        let state: HashMap<&str, &FileItem> = current
            .files
            .iter()
            .map(|f| (f.target.as_str(), f))
            .collect();
        let mut outcome = SyncOutcome::default();
        let mut keep =
            |target: &String, wanted: fn(&FileItem) -> bool| match state.get(target.as_str()) {
                Some(item) if wanted(item) => true,
                _ => {
                    outcome.skipped.push(Skipped {
                        target: target.clone(),
                        reason: "it changed after the sync was planned".into(),
                    });
                    false
                }
            };
        let save: Vec<String> = plan
            .save
            .iter()
            .filter(|t| keep(t, |i| i.state == FileState::ChangedHere && i.can_save()))
            .cloned()
            .collect();
        let update: Vec<String> = plan
            .update
            .iter()
            .filter(|t| keep(t, |i| i.state == FileState::UpdateAvailable))
            .cloned()
            .collect();
        let steps: HashMap<&str, &SetupStep> = current
            .setup_steps
            .iter()
            .map(|s| (s.target.as_str(), s))
            .collect();
        let (run, gone): (Vec<String>, Vec<String>) = plan
            .run
            .iter()
            .cloned()
            .partition(|t| steps.contains_key(t.as_str()));
        outcome
            .skipped
            .extend(gone.into_iter().map(|target| Skipped {
                target,
                reason: "it no longer needs to run".into(),
            }));

        if !save.is_empty() {
            let readded = self.re_add(&save)?;
            outcome.skipped.extend(readded.skipped);
            outcome.saved = readded.updated;
        }

        if !update.is_empty() || !run.is_empty() {
            // chezmoi ignores `before_` and `after_` when given targets and
            // runs everything in name order, so apply in three rounds.
            let scripts = run
                .iter()
                .filter_map(|t| steps.get(t.as_str()).copied())
                .chain(&current.every_apply);
            let mut rounds: [Vec<String>; 3] = Default::default();
            rounds[1] = current.apply_targets(&update);
            for script in scripts {
                rounds[script.order as usize].push(script.target.clone());
            }
            let options = ApplyOptions {
                non_recursive: true,
                ..Default::default()
            };
            for round in rounds.iter_mut().filter(|r| !r.is_empty()) {
                round.sort();
                self.apply(round, &options)?;
            }
        }
        outcome.updated = update;
        outcome.ran = run;

        if plan.upload
            && let Some(git) = self.shared_git()?
        {
            let status = git.status()?;
            if !status.is_clean() {
                outcome.commit = Some(git.commit_all(&commit_message(&outcome.saved))?);
            }
            if status.upstream.is_some() && (outcome.commit.is_some() || status.ahead > 0) {
                match git.push() {
                    Ok(()) => outcome.pushed = true,
                    Err(err) => outcome.push_error = Some(err.to_string()),
                }
            }
        }
        outcome.decide = current.decisions().map(|f| f.target.clone()).collect();
        Ok(outcome)
    }

    /// One sync, for the CLI and agents: pull, plan, then carry out the
    /// plan. A dry run fetches and plans only.
    pub fn sync(&self, options: &SyncOptions) -> Result<SyncReport> {
        let dest = self.paths()?.dest_dir;
        let skip = options
            .skip
            .iter()
            .map(|t| self.relative_target(&dest, t))
            .collect::<Result<Vec<_>>>()?;
        let mut warnings = Vec::new();
        // Offline, a sync still saves and updates on this computer; the
        // commit is uploaded by a later sync. A failed pull (a conflict)
        // stops it.
        let incoming_commits = match self.fetch_shared() {
            Ok(behind) => {
                if behind > 0 && !options.dry_run {
                    self.git()?.pull()?;
                }
                behind
            }
            Err(err) => {
                warnings.push(format!(
                    "Could not reach the remote, so only this computer is synced: {err}"
                ));
                0
            }
        };
        let overview = self.overview()?;
        let plan = SyncPlan::new(&overview, &skip);
        let outcome = if options.dry_run {
            None
        } else {
            Some(self.execute_with(&plan, &overview)?)
        };
        Ok(SyncReport {
            dry_run: options.dry_run,
            incoming_commits,
            plan,
            outcome,
            warnings,
        })
    }
}

fn commit_message(saved: &[String]) -> String {
    let subject = match saved {
        [] => "Update dotfiles".to_string(),
        [one] => format!("Update ~/{one}"),
        many => format!("Update {} files", many.len()),
    };
    let mut message = format!("{subject}\n\nSynced with Chezmate.\n");
    if saved.len() > 1 {
        message.push('\n');
        for target in saved {
            message.push_str(&format!("- ~/{target}\n"));
        }
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, local: Change, apply: Change) -> StatusEntry {
        StatusEntry {
            path: path.into(),
            local,
            apply,
            runs_every_apply: false,
        }
    }

    fn item(target: &str, state: FileState, kind: EntryKind) -> FileItem {
        FileItem {
            target: target.into(),
            state,
            incoming: Change::Modified,
            kind,
            template: false,
            encrypted: false,
            cannot_save: None,
            never_synced: false,
        }
    }

    #[test]
    fn one_state_per_status_combination() {
        use Change::*;
        let file = EntryKind::File;
        let state = |local, apply, since| file_state(&entry(".a", local, apply), since, file);
        assert_eq!(
            state(None, Added, Since::NeverWritten),
            (FileState::UpdateAvailable, false)
        );
        assert_eq!(
            state(None, Modified, Since::Unchanged),
            (FileState::UpdateAvailable, false)
        );
        assert_eq!(
            state(None, Modified, Since::NeverWritten),
            (FileState::BothChanged, true),
            "a file chezmoi never wrote is this computer's own version"
        );
        assert_eq!(
            state(Modified, Modified, Since::Unchanged),
            (FileState::ChangedHere, false)
        );
        assert_eq!(
            state(Modified, Modified, Since::Changed),
            (FileState::BothChanged, false)
        );
        assert_eq!(
            state(Deleted, Added, Since::Unchanged),
            (FileState::DeletedHere, false)
        );
        // A file chezmoi never wrote, which an `exact_` directory would delete.
        assert_eq!(
            state(None, Deleted, Since::NeverWritten),
            (FileState::BothChanged, true)
        );
        assert_eq!(
            file_state(
                &entry(".old", None, Deleted),
                Since::NeverWritten,
                EntryKind::Remove
            ),
            (FileState::UpdateAvailable, false),
            "remove_ is meant to delete"
        );
        // A directory chezmoi never wrote only gets its permissions fixed.
        assert_eq!(
            file_state(
                &entry(".ssh", None, Modified),
                Since::NeverWritten,
                EntryKind::Directory
            ),
            (FileState::UpdateAvailable, false)
        );
    }

    #[test]
    fn plan_leaves_decisions_and_skips_alone() {
        let mut template = item(".t", FileState::ChangedHere, EntryKind::File);
        template.cannot_save = Some(SaveBlocker::Template);
        let overview = Overview {
            files: vec![
                item(".a", FileState::ChangedHere, EntryKind::File),
                item(".b", FileState::ChangedHere, EntryKind::File),
                item(".c", FileState::UpdateAvailable, EntryKind::File),
                item(".d", FileState::BothChanged, EntryKind::File),
                item(".e", FileState::DeletedHere, EntryKind::File),
                template,
            ],
            setup_steps: vec![SetupStep::new("install.sh", None)],
            every_apply: vec![SetupStep::new("always.sh", None)],
            shared: Some(SharedStatus::default()),
            ..Default::default()
        };
        let plan = SyncPlan::new(&overview, &[".b".into()]);
        assert_eq!(plan.save, vec![".a"]);
        assert_eq!(plan.skipped, vec![".b"]);
        assert_eq!(plan.update, vec![".c"]);
        assert_eq!(plan.decide, vec![".d", ".e", ".t"]);
        assert_eq!(plan.run, vec!["install.sh"]);
        assert_eq!(plan.every_apply, vec!["always.sh"]);
        assert!(plan.upload);

        // Nothing to update: scripts that run on every apply stay out.
        let quiet = Overview {
            files: vec![item(".a", FileState::ChangedHere, EntryKind::File)],
            every_apply: vec![SetupStep::new("always.sh", None)],
            ..Default::default()
        };
        let plan = SyncPlan::new(&quiet, &[]);
        assert!(plan.every_apply.is_empty());
        assert!(!plan.upload, "no repository, nothing to upload");
        assert!(!plan.is_empty());
        assert!(SyncPlan::new(&quiet, &[".a".into()]).is_empty());
    }

    #[test]
    fn apply_targets_include_missing_directories_only() {
        let new_dir = |target: &str| FileItem {
            incoming: Change::Added,
            ..item(target, FileState::UpdateAvailable, EntryKind::Directory)
        };
        let overview = Overview {
            files: vec![
                item(".config/new/a", FileState::UpdateAvailable, EntryKind::File),
                item(".configx", FileState::UpdateAvailable, EntryKind::File),
            ],
            directories: vec![
                // Exists, only its permissions differ: never applied along.
                item(".config", FileState::UpdateAvailable, EntryKind::Directory),
                new_dir(".config/new"),
            ],
            ..Default::default()
        };
        assert_eq!(
            overview.apply_targets(&[".config/new/a".into(), ".configx".into()]),
            vec![".config/new", ".config/new/a", ".configx"]
        );
    }

    #[test]
    fn commit_messages_name_the_files() {
        assert!(commit_message(&[".zshrc".into()]).starts_with("Update ~/.zshrc\n\n"));
        let many = commit_message(&[".a".into(), ".b".into()]);
        assert!(many.starts_with("Update 2 files\n"));
        assert!(many.contains("- ~/.b\n"));
        assert!(commit_message(&[]).starts_with("Update dotfiles"));
    }

    #[test]
    fn hashes_like_chezmoi() {
        // `printf .a | shasum -a 256`
        assert_eq!(
            sha256_hex(b".a"),
            "36797c3aba1ef6ae33ece2e5a184ba8061d29c4fb479a4ef2ddedad1b4ca1200"
        );
    }
}

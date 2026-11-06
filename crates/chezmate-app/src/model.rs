//! The app's view of chezmoi: one retained entity that loads state on a
//! background thread and runs operations one at a time.

use std::collections::HashMap;
use std::sync::Arc;

use chezmate_core::{
    AddOptions, ApplyOptions, Chezmoi, ChezmoiOptions, DiffOptions, DoctorCheck, Info,
    ManagedEntry, StatusEntry,
};
use gpui_kit::component::diff::{DiffFile, DiffParseOptions};
use gpui_kit::{Context, EventEmitter, SharedString, Task};

/// Everything the Changes and Files pages show, read in one refresh.
pub struct Snapshot {
    pub info: Info,
    pub status: Vec<StatusEntry>,
    pub managed: Vec<ManagedEntry>,
    managed_index: HashMap<String, usize>,
    pub diffs: HashMap<SharedString, DiffFile>,
    /// The diff could not be read or parsed; status is still valid.
    pub diff_error: Option<String>,
}

impl Snapshot {
    pub fn managed_entry(&self, target: &str) -> Option<&ManagedEntry> {
        self.managed_index.get(target).map(|&ix| &self.managed[ix])
    }

    pub fn diff(&self, target: &str) -> Option<&DiffFile> {
        self.diffs.get(target)
    }
}

/// The outcome of an operation, shown as a notification.
pub enum DotfilesEvent {
    Finished(Result<String, String>),
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
                cx.emit(DotfilesEvent::Finished(result));
                this.refresh(cx);
            });
        });
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
    let status = chezmoi.status(&[]).map_err(|err| err.to_string())?;
    let managed = chezmoi.managed().map_err(|err| err.to_string())?;
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

    Ok(Snapshot {
        info,
        status,
        managed,
        managed_index,
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
            assert!(snapshot.diff(&entry.path).is_some(), "no diff for {}", entry.path);
        }

        // Modified, deleted-locally and mode-changed targets.
        chezmoi.apply(&[], &ApplyOptions::default()).unwrap();
        fs::write(source.join("dot_hello"), "hello\nmore\n").unwrap();
        fs::remove_file(home.join(".profile")).unwrap();
        fs::rename(source.join("executable_dot_tool"), source.join("dot_tool")).unwrap();
        let snapshot = load(&chezmoi).unwrap();
        assert_eq!(snapshot.diff_error, None);
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
            println!("{} has diff: {}", entry.path, snapshot.diff(&entry.path).is_some());
        }
        assert_eq!(snapshot.diff_error, None);
    }
}

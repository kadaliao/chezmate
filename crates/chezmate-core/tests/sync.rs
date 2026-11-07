//! The simplified model end to end: two computers, each with its own
//! sandboxed home, source clone and chezmoi state, sharing a local bare
//! repository as the remote.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use chezmate_core::{
    ApplyOptions, Change, EntryKind, FileState, SaveBlocker, ScriptOrder, ScriptRuns, SyncOptions,
    SyncPlan,
};
use common::Sandbox;

type Env = Vec<(std::ffi::OsString, std::ffi::OsString)>;

fn git(dir: &Path, env: &Env, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// Two computers and the remote they share. Keep it alive for the test.
struct World {
    _dir: tempfile::TempDir,
    env: Env,
    remote: PathBuf,
}

impl World {
    /// A remote holding `files` (source names and contents).
    fn new(files: &[(&str, &str)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let config = base.join("gitconfig");
        fs::write(
            &config,
            "[user]\n\tname = Test\n\temail = test@example.com\n[commit]\n\tgpgsign = false\n[init]\n\tdefaultBranch = main\n[pull]\n\trebase = false\n",
        )
        .unwrap();
        let env: Env = vec![
            ("GIT_CONFIG_GLOBAL".into(), config.into()),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        ];
        let remote = base.join("remote.git");
        git(
            &base,
            &env,
            &["init", "--bare", "-q", remote.to_str().unwrap()],
        );
        let seed = base.join("seed");
        fs::create_dir_all(&seed).unwrap();
        git(&seed, &env, &["init", "-q"]);
        for (path, contents) in files {
            let path = seed.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
        git(&seed, &env, &["add", "-A"]);
        git(&seed, &env, &["commit", "-q", "-m", "Initial dotfiles"]);
        git(
            &seed,
            &env,
            &["push", "-q", remote.to_str().unwrap(), "main"],
        );
        Self {
            _dir: dir,
            env,
            remote,
        }
    }

    /// A computer with the shared version cloned. `applied` runs a first
    /// `chezmoi apply`, as on a computer that has been in use for a while.
    fn computer(&self, applied: bool) -> Sandbox {
        let sb = Sandbox::with_env(self.env.clone());
        git(
            &sb.source,
            &self.env,
            &["clone", "-q", self.remote.to_str().unwrap(), "."],
        );
        if applied {
            sb.chezmoi.apply(&[], &ApplyOptions::default()).unwrap();
        }
        sb
    }
}

fn sync(sb: &Sandbox) -> chezmate_core::SyncOutcome {
    sb.chezmoi
        .sync(&SyncOptions::default())
        .unwrap()
        .outcome
        .unwrap()
}

#[test]
fn changes_travel_between_computers() {
    let world = World::new(&[("dot_hello", "hello\n"), ("dot_profile", "profile\n")]);
    let laptop = world.computer(true);
    let desktop = world.computer(true);
    assert!(laptop.chezmoi.overview().unwrap().is_synced());

    laptop.write_home(".hello", "hello from laptop\n");
    let overview = laptop.chezmoi.overview().unwrap();
    let file = overview.file(".hello").unwrap();
    assert_eq!(file.state, FileState::ChangedHere);
    assert!(file.can_save() && !file.needs_decision());
    let plan = SyncPlan::new(&overview, &[]);
    assert_eq!(plan.save, vec![".hello"]);
    assert!(plan.upload);

    let outcome = sync(&laptop);
    assert_eq!(outcome.saved, vec![".hello"]);
    assert!(outcome.commit.is_some());
    assert!(outcome.pushed, "{:?}", outcome.push_error);
    assert_eq!(laptop.read_source("dot_hello"), "hello from laptop\n");
    assert!(laptop.chezmoi.overview().unwrap().is_synced());
    let subject = git(
        &world.remote,
        &world.env,
        &["log", "-1", "--format=%s", "main"],
    );
    assert_eq!(subject.trim(), "Update ~/.hello");

    // The desktop has not heard of it yet; a dry run fetches and says so.
    assert!(desktop.chezmoi.overview().unwrap().is_synced());
    let dry = desktop
        .chezmoi
        .sync(&SyncOptions {
            dry_run: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(dry.incoming_commits, 1);
    assert!(dry.outcome.is_none());
    assert_eq!(desktop.read_home(".hello"), "hello\n");

    let outcome = sync(&desktop);
    assert_eq!(outcome.updated, vec![".hello"]);
    assert!(!outcome.pushed, "nothing to upload");
    assert_eq!(desktop.read_home(".hello"), "hello from laptop\n");
    assert!(desktop.chezmoi.overview().unwrap().is_synced());
}

#[test]
fn edits_on_both_computers_wait_for_a_decision() {
    let world = World::new(&[("dot_hello", "hello\n")]);
    let laptop = world.computer(true);
    let desktop = world.computer(true);

    laptop.write_home(".hello", "laptop\n");
    sync(&laptop);
    desktop.write_home(".hello", "desktop\n");

    // Before pulling, the desktop only knows about its own edit.
    let before = desktop.chezmoi.overview().unwrap();
    assert_eq!(before.file(".hello").unwrap().state, FileState::ChangedHere);

    // A sync pulls first, notices both sides changed, and touches nothing.
    let outcome = sync(&desktop);
    assert!(outcome.saved.is_empty() && outcome.updated.is_empty());
    assert_eq!(outcome.decide, vec![".hello"]);
    assert_eq!(desktop.read_home(".hello"), "desktop\n");
    assert_eq!(desktop.read_source("dot_hello"), "laptop\n");
    let overview = desktop.chezmoi.overview().unwrap();
    let file = overview.file(".hello").unwrap();
    assert_eq!(file.state, FileState::BothChanged);
    assert!(file.needs_decision() && file.can_save());

    // Keep the desktop's version, then sync it to the laptop.
    desktop.chezmoi.re_add(&[".hello".into()]).unwrap();
    let outcome = sync(&desktop);
    assert!(outcome.pushed, "{:?}", outcome.push_error);
    let outcome = sync(&laptop);
    assert_eq!(outcome.updated, vec![".hello"]);
    assert_eq!(laptop.read_home(".hello"), "desktop\n");
}

#[test]
fn a_new_computer_keeps_its_own_files() {
    let world = World::new(&[("dot_hello", "hello\n"), ("dot_profile", "profile\n")]);
    let fresh = world.computer(false);
    fresh.write_home(".hello", "my own hello\n");

    let overview = fresh.chezmoi.overview().unwrap();
    let hello = overview.file(".hello").unwrap();
    assert_eq!(hello.state, FileState::BothChanged);
    assert!(hello.never_synced);
    let profile = overview.file(".profile").unwrap();
    assert_eq!(profile.state, FileState::UpdateAvailable);
    assert_eq!(profile.incoming, Change::Added);

    let outcome = sync(&fresh);
    assert_eq!(outcome.updated, vec![".profile"]);
    assert_eq!(outcome.decide, vec![".hello"]);
    assert_eq!(
        fresh.read_home(".hello"),
        "my own hello\n",
        "never overwritten"
    );
    assert_eq!(fresh.read_home(".profile"), "profile\n");
}

#[test]
fn skipped_and_deleted_files_stay_as_they_are() {
    let world = World::new(&[("dot_a", "a\n"), ("dot_b", "b\n"), ("dot_c", "c\n")]);
    let laptop = world.computer(true);
    laptop.write_home(".a", "a2\n");
    laptop.write_home(".b", "b2\n");
    fs::remove_file(laptop.home.join(".c")).unwrap();

    let overview = laptop.chezmoi.overview().unwrap();
    assert_eq!(overview.file(".c").unwrap().state, FileState::DeletedHere);
    let report = laptop
        .chezmoi
        .sync(&SyncOptions {
            dry_run: false,
            skip: vec!["~/.b".into()],
        })
        .unwrap();
    assert_eq!(report.plan.skipped, vec![".b"]);
    let outcome = report.outcome.unwrap();
    assert_eq!(outcome.saved, vec![".a"]);
    assert_eq!(outcome.decide, vec![".c"]);
    assert_eq!(laptop.read_source("dot_b"), "b\n");
    assert!(
        !laptop.home.join(".c").exists(),
        "not restored behind your back"
    );
    let overview = laptop.chezmoi.overview().unwrap();
    assert_eq!(overview.file(".b").unwrap().state, FileState::ChangedHere);
}

#[test]
fn templates_and_setup_steps() {
    let world = World::new(&[
        ("dot_t.tmpl", "os={{ .chezmoi.os }}\n"),
        ("dot_hello", "hello\n"),
    ]);
    let laptop = world.computer(true);
    let log = laptop.root.path().canonicalize().unwrap().join("log");
    laptop.write_home(".t", "edited\n");
    laptop.write_source(
        "run_once_install.sh",
        &format!("#!/bin/sh\necho once >> {}\n", log.display()),
    );
    laptop.write_source(
        "run_onchange_before_zfirst.sh",
        &format!("#!/bin/sh\necho before >> {}\n", log.display()),
    );
    laptop.write_source(
        "run_after_always.sh",
        &format!("#!/bin/sh\necho always >> {}\n", log.display()),
    );

    let overview = laptop.chezmoi.overview().unwrap();
    let t = overview.file(".t").unwrap();
    assert_eq!(t.state, FileState::ChangedHere);
    assert_eq!(t.cannot_save, Some(SaveBlocker::Template));
    assert!(t.needs_decision());
    // In chezmoi's order: `before_` scripts first.
    let steps: Vec<_> = overview
        .setup_steps
        .iter()
        .map(|s| (s.target.as_str(), s.runs, s.order))
        .collect();
    assert_eq!(
        steps,
        vec![
            ("zfirst.sh", ScriptRuns::OnChange, ScriptOrder::Before),
            ("install.sh", ScriptRuns::Once, ScriptOrder::During),
        ]
    );
    assert_eq!(overview.every_apply[0].target, "always.sh");
    assert_eq!(overview.every_apply[0].order, ScriptOrder::After);

    let outcome = sync(&laptop);
    assert_eq!(outcome.ran, vec!["zfirst.sh", "install.sh"]);
    assert_eq!(outcome.decide, vec![".t"]);
    assert!(outcome.commit.is_some(), "the new scripts are uploaded");
    assert_eq!(fs::read_to_string(&log).unwrap(), "before\nonce\nalways\n");
    assert_eq!(laptop.read_home(".t"), "edited\n");

    // Nothing left to update: the every-apply script does not run again.
    laptop.write_home(".hello", "hi\n");
    sync(&laptop);
    assert_eq!(fs::read_to_string(&log).unwrap(), "before\nonce\nalways\n");
}

#[test]
fn new_directories_come_with_their_files() {
    let world = World::new(&[("dot_hello", "hello\n")]);
    let laptop = world.computer(true);
    let desktop = world.computer(true);
    laptop.write_source("dot_config/new/private_file", "secret\n");
    let outcome = sync(&laptop);
    assert!(outcome.pushed, "uncommitted source changes are uploaded");

    desktop.chezmoi.pull_shared().unwrap();
    let overview = desktop.chezmoi.overview().unwrap();
    let files: Vec<&str> = overview.files.iter().map(|f| f.target.as_str()).collect();
    assert_eq!(files, vec![".config/new/file"]);
    let dirs: Vec<(&str, EntryKind)> = overview
        .directories
        .iter()
        .map(|f| (f.target.as_str(), f.kind))
        .collect();
    assert_eq!(
        dirs,
        vec![
            (".config", EntryKind::Directory),
            (".config/new", EntryKind::Directory)
        ]
    );
    let outcome = sync(&desktop);
    assert_eq!(outcome.updated, vec![".config/new/file"]);
    assert_eq!(desktop.read_home(".config/new/file"), "secret\n");
}

#[test]
fn without_a_repository_sync_stays_on_this_computer() {
    let sb = Sandbox::new();
    sb.write_source("dot_hello", "hello\n");
    sb.chezmoi.apply(&[], &ApplyOptions::default()).unwrap();
    sb.write_home(".hello", "edit\n");
    let overview = sb.chezmoi.overview().unwrap();
    assert!(overview.shared.is_none());
    let outcome = sync(&sb);
    assert_eq!(outcome.saved, vec![".hello"]);
    assert!(outcome.commit.is_none() && !outcome.pushed);
    assert_eq!(sb.read_source("dot_hello"), "edit\n");
}

#[test]
fn a_stale_plan_does_not_overwrite_new_edits() {
    let world = World::new(&[("dot_hello", "hello\n")]);
    let laptop = world.computer(true);
    let desktop = world.computer(true);
    laptop.write_home(".hello", "laptop\n");
    sync(&laptop);

    desktop.chezmoi.pull_shared().unwrap();
    let plan = SyncPlan::new(&desktop.chezmoi.overview().unwrap(), &[]);
    assert_eq!(plan.update, vec![".hello"]);
    // The file is edited after the plan was shown.
    desktop.write_home(".hello", "typed meanwhile\n");
    let outcome = desktop.chezmoi.execute_sync(&plan).unwrap();
    assert!(outcome.updated.is_empty());
    assert_eq!(outcome.skipped[0].target, ".hello");
    assert_eq!(desktop.read_home(".hello"), "typed meanwhile\n");
}

#[test]
fn offline_sync_saves_here_and_uploads_later() {
    let world = World::new(&[("dot_hello", "hello\n")]);
    let laptop = world.computer(true);
    let moved = world.remote.with_extension("away");
    fs::rename(&world.remote, &moved).unwrap();
    laptop.write_home(".hello", "on a plane\n");

    let report = laptop.chezmoi.sync(&SyncOptions::default()).unwrap();
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    let outcome = report.outcome.unwrap();
    assert_eq!(outcome.saved, vec![".hello"]);
    assert!(outcome.commit.is_some());
    assert!(!outcome.pushed && outcome.push_error.is_some());

    // Back online: the next sync uploads the waiting commit.
    fs::rename(&moved, &world.remote).unwrap();
    let outcome = sync(&laptop);
    assert!(outcome.pushed, "{:?}", outcome.push_error);
    assert_eq!(
        git(&world.remote, &world.env, &["show", "main:dot_hello"]),
        "on a plane\n"
    );
}

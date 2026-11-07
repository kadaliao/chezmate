//! UI integration tests: the real window, real GPUI Kit components and the
//! real chezmoi, run against a sandboxed home and source directory.

use std::fs;
use std::path::PathBuf;

use chezmate_core::{ApplyOptions, Chezmoi, ChezmoiOptions};
use gpui_kit::component::WindowExt as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, SharedString, TestAppContext, WindowBounds,
    WindowOptions, px, size,
};
use tempfile::TempDir;

use crate::app::{Chezmate, Page};
use crate::model::DotfilesEvent;
use crate::overview::Selection;
use chezmate_core::FileState;

struct Sandbox {
    _root: TempDir,
    home: PathBuf,
    source: PathBuf,
    options: ChezmoiOptions,
}

impl Sandbox {
    fn new(files: &[(&str, &str)]) -> Self {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        let (home, source, config) = (base.join("home"), base.join("source"), base.join("config"));
        for dir in [&home, &source, &config] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(config.join("chezmoi.toml"), "").unwrap();
        for (path, contents) in files {
            fs::write(source.join(path), contents).unwrap();
        }
        let options = ChezmoiOptions {
            source: Some(source.clone()),
            destination: Some(home.clone()),
            config: Some(config.join("chezmoi.toml")),
            persistent_state: Some(config.join("state.boltdb")),
            cache: Some(base.join("cache")),
            // A system-only PATH keeps `chezmoi doctor` from probing slow tools.
            env: vec![
                ("HOME".into(), home.clone().into()),
                ("PATH".into(), "/usr/bin:/bin".into()),
            ],
            ..Default::default()
        };
        Self {
            _root: root,
            home,
            source,
            options,
        }
    }

    fn chezmoi(&self) -> Chezmoi {
        Chezmoi::new(self.options.clone()).unwrap()
    }

    fn home(&self, path: &str) -> String {
        fs::read_to_string(self.home.join(path)).unwrap()
    }
}

fn open(cx: &mut TestAppContext, sandbox: &Sandbox) -> (AnyWindowHandle, Entity<Chezmate>) {
    cx.update(gpui_kit::init);
    cx.update(crate::app::init);
    let options = sandbox.options.clone();
    let opened = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    Default::default(),
                    size(px(1100.), px(700.)),
                ))),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| Chezmate::new(options, window, cx)),
        )
        .unwrap()
    });
    cx.run_until_parked();
    opened
}

fn show(cx: &mut TestAppContext, view: &Entity<Chezmate>, page: Page) {
    cx.update(|cx| view.update(cx, |view, cx| view.set_page(page, cx)));
}

/// (target, state) for every file on the Overview page.
fn overview_files(view: &Entity<Chezmate>, cx: &mut TestAppContext) -> Vec<(String, FileState)> {
    cx.read(|cx| {
        let model = view.read(cx).model().read(cx);
        assert!(model.error().is_none(), "load failed: {:?}", model.error());
        model
            .snapshot()
            .expect("snapshot loaded")
            .overview
            .files
            .iter()
            .map(|f| (f.target.clone(), f.state))
            .collect()
    })
}

/// Clicks the elements with these IDs, in order, in one frame.
fn click(cx: &mut TestAppContext, handle: AnyWindowHandle, ids: &[&str]) {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        for id in ids {
            window.click(SharedString::from(id.to_string()), cx);
        }
    })
    .unwrap();
    cx.run_until_parked();
}

fn statuses(view: &Entity<Chezmate>, cx: &mut TestAppContext) -> Vec<String> {
    cx.read(|cx| {
        let model = view.read(cx).model().read(cx);
        assert!(model.error().is_none(), "load failed: {:?}", model.error());
        model
            .snapshot()
            .expect("snapshot loaded")
            .status
            .iter()
            .map(|e| e.path.clone())
            .collect()
    })
}

#[gpui_kit::test]
fn applies_a_new_file_from_the_list(cx: &mut TestAppContext) {
    let sb = Sandbox::new(&[("dot_hello", "hello\n"), ("dot_profile", "profile\n")]);
    let (handle, view) = open(cx, &sb);
    show(cx, &view, Page::Changes);
    assert_eq!(statuses(&view, cx), vec![".hello", ".profile"]);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click(SharedString::new(".profile"), cx);
        let changes = view.read(cx).changes().read(cx);
        assert_eq!(changes.selected(), Some(".profile"));
        window.click("apply-selected", cx);
    })
    .unwrap();
    cx.run_until_parked();

    assert_eq!(sb.home(".profile"), "profile\n");
    assert!(
        !sb.home.join(".hello").exists(),
        "only the selected file is applied"
    );
    assert_eq!(statuses(&view, cx), vec![".hello"]);
}

#[gpui_kit::test]
fn overwriting_local_edits_needs_confirmation(cx: &mut TestAppContext) {
    let sb = Sandbox::new(&[("dot_hello", "hello\n")]);
    sb.chezmoi().apply(&[], &ApplyOptions::default()).unwrap();
    fs::write(sb.home.join(".hello"), "my edit\n").unwrap();
    fs::write(sb.source.join("dot_hello"), "from dotfiles\n").unwrap();

    let (handle, view) = open(cx, &sb);
    show(cx, &view, Page::Changes);
    assert_eq!(statuses(&view, cx), vec![".hello"]);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("apply-selected", cx);
        assert!(window.has_active_dialog(cx), "overwrite must ask first");
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        sb.home(".hello"),
        "my edit\n",
        "nothing changes before confirming"
    );

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("ok", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(sb.home(".hello"), "from dotfiles\n");
    assert!(statuses(&view, cx).is_empty());
}

#[gpui_kit::test]
fn keep_local_saves_edits_to_the_source(cx: &mut TestAppContext) {
    let sb = Sandbox::new(&[("dot_hello", "hello\n")]);
    sb.chezmoi().apply(&[], &ApplyOptions::default()).unwrap();
    fs::write(sb.home.join(".hello"), "my edit\n").unwrap();

    let (handle, view) = open(cx, &sb);
    show(cx, &view, Page::Changes);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("keep-local", cx);
    })
    .unwrap();
    cx.run_until_parked();

    assert_eq!(
        fs::read_to_string(sb.source.join("dot_hello")).unwrap(),
        "my edit\n"
    );
    assert!(statuses(&view, cx).is_empty());
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("refresh-empty").is_some(),
            "shows the up-to-date state"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn environment_runs_diagnostics(cx: &mut TestAppContext) {
    let sb = Sandbox::new(&[("dot_hello", "hello\n")]);
    let (handle, view) = open(cx, &sb);
    cx.update(|cx| view.update(cx, |view, cx| view.set_page(Page::Environment, cx)));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("run-doctor", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let model = view.read(cx).model().read(cx);
        match model.doctor() {
            crate::model::DoctorState::Done(checks) => {
                assert!(checks.iter().any(|c| c.check == "version"))
            }
            _ => panic!("doctor did not finish"),
        }
    });
}

#[gpui_kit::test]
fn files_filter_review_and_stop_managing(cx: &mut TestAppContext) {
    let sb = Sandbox::new(&[("dot_hello", "hello\n"), ("dot_profile", "profile\n")]);
    sb.chezmoi().apply(&[], &ApplyOptions::default()).unwrap();
    fs::write(sb.source.join("dot_profile"), "profile v2\n").unwrap();
    let (handle, view) = open(cx, &sb);
    cx.update(|cx| view.update(cx, |view, cx| view.set_page(Page::Files, cx)));

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(view.read(cx).files().read(cx).visible_count(), 2);
        window.click("file-filter", cx);
        window.input("prof", cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(view.read(cx).files().read(cx).visible_count(), 1);
        window.click(SharedString::new("file:.profile"), cx);
        assert_eq!(view.read(cx).files().read(cx).selected(), Some(".profile"));
        // .profile has a pending change: jump to it on the Changes page.
        window.click("review-change", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(view.read(cx).page(), Page::Changes);
        assert_eq!(
            view.read(cx).changes().read(cx).selected(),
            Some(".profile")
        );
    });

    cx.update(|cx| view.update(cx, |view, cx| view.set_page(Page::Files, cx)));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("forget", cx);
        assert!(window.has_active_dialog(cx));
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        sb.source.join("dot_profile").exists(),
        "nothing removed before confirming"
    );
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("ok", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(!sb.source.join("dot_profile").exists());
    assert_eq!(sb.home(".profile"), "profile\n", "the home file stays");
    cx.read(|cx| {
        let snapshot = view.read(cx).model().read(cx).snapshot().cloned().unwrap();
        assert!(snapshot.managed.iter().all(|e| e.target != ".profile"));
    });
}

#[gpui_kit::test]
fn adding_a_file_starts_managing_it(cx: &mut TestAppContext) {
    let sb = Sandbox::new(&[("dot_hello", "hello\n")]);
    fs::write(sb.home.join(".newrc"), "set x\n").unwrap();
    let (_handle, view) = open(cx, &sb);
    // The picker is native UI; drive the same model command it calls.
    let path = sb.home.join(".newrc").to_string_lossy().into_owned();
    cx.update(|cx| {
        let model = view.read(cx).model().clone();
        model.update(cx, |model, cx| model.add(vec![path], cx));
    });
    cx.run_until_parked();
    assert_eq!(
        fs::read_to_string(sb.source.join("dot_newrc")).unwrap(),
        "set x\n"
    );
    cx.read(|cx| {
        let snapshot = view.read(cx).model().read(cx).snapshot().cloned().unwrap();
        assert!(snapshot.managed.iter().any(|e| e.target == ".newrc"));
    });
}

/// Runs git with an isolated global config (identity, no signing).
fn git(dir: &std::path::Path, config: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[gpui_kit::test]
fn repository_commits_pushes_and_pulls(cx: &mut TestAppContext) {
    let mut sb = Sandbox::new(&[("dot_hello", "hello\n")]);
    let base = sb.home.parent().unwrap().to_path_buf();
    let config = base.join("gitconfig");
    fs::write(
        &config,
        "[user]\n\tname = Test\n\temail = test@example.com\n[commit]\n\tgpgsign = false\n[init]\n\tdefaultBranch = main\n",
    )
    .unwrap();
    sb.options
        .env
        .push(("GIT_CONFIG_GLOBAL".into(), config.clone().into()));
    sb.options
        .env
        .push(("GIT_CONFIG_NOSYSTEM".into(), "1".into()));
    let remote = base.join("remote.git");
    git(
        &base,
        &config,
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );
    git(&sb.source, &config, &["init", "-q"]);
    git(&sb.source, &config, &["add", "-A"]);
    git(&sb.source, &config, &["commit", "-q", "-m", "Initial"]);
    git(
        &sb.source,
        &config,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&sb.source, &config, &["push", "-q", "-u", "origin", "main"]);
    sb.chezmoi().apply(&[], &ApplyOptions::default()).unwrap();
    // An edit to the source waiting to be committed.
    fs::write(sb.source.join("dot_hello"), "hello\nmore\n").unwrap();

    let (handle, view) = open(cx, &sb);
    cx.update(|cx| view.update(cx, |view, cx| view.set_page(Page::Repository, cx)));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // Disabled until a message is typed.
        window.click("commit", cx);
        window.click("commit-message", cx);
        window.input("Add a line", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        git(&sb.source, &config, &["log", "-1", "--format=%s"]).trim(),
        "Initial"
    );
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("commit", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        git(&sb.source, &config, &["log", "-1", "--format=%s"]).trim(),
        "Add a line"
    );
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("commit-message").value(),
            Some(""),
            "message cleared"
        );
        window.click("push", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        git(&remote, &config, &["log", "-1", "--format=%s", "main"]).trim(),
        "Add a line"
    );

    // Another machine pushes; fetch then pull it here.
    let other = base.join("other");
    git(
        &base,
        &config,
        &[
            "clone",
            "-q",
            remote.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    fs::write(other.join("dot_hello"), "from laptop\n").unwrap();
    git(&other, &config, &["commit", "-q", "-am", "Laptop"]);
    git(&other, &config, &["push", "-q"]);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("fetch", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let snapshot = view.read(cx).model().read(cx).snapshot().cloned().unwrap();
        let Some(Ok(repo)) = &snapshot.repo else {
            panic!("repo state missing")
        };
        assert_eq!(repo.status.behind, 1);
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("pull", cx);
    })
    .unwrap();
    cx.run_until_parked();

    assert_eq!(
        fs::read_to_string(sb.source.join("dot_hello")).unwrap(),
        "from laptop\n"
    );
    assert_eq!(sb.home(".hello"), "hello\n", "pull never applies");
    assert_eq!(
        statuses(&view, cx),
        vec![".hello"],
        "the incoming change waits in Changes"
    );
}

#[gpui_kit::test]
fn every_apply_scripts_are_not_pending_changes(cx: &mut TestAppContext) {
    let sb = Sandbox::new(&[
        ("dot_hello", "hello\n"),
        ("run_after_ensure.sh", "#!/bin/sh\ntrue\n"),
    ]);
    sb.chezmoi().apply(&[], &ApplyOptions::default()).unwrap();
    let (handle, view) = open(cx, &sb);
    show(cx, &view, Page::Changes);
    assert!(statuses(&view, cx).is_empty());
    cx.read(|cx| {
        let snapshot = view.read(cx).model().read(cx).snapshot().cloned().unwrap();
        assert_eq!(snapshot.every_apply.len(), 1);
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("refresh-empty").is_some(),
            "shows Up to date"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn overview_saves_a_change_made_here(cx: &mut TestAppContext) {
    let sb = Sandbox::new(&[("dot_hello", "hello\n"), ("dot_profile", "profile\n")]);
    sb.chezmoi().apply(&[], &ApplyOptions::default()).unwrap();
    fs::write(sb.home.join(".hello"), "my edit\n").unwrap();
    fs::write(sb.source.join("dot_profile"), "profile v2\n").unwrap();

    let (handle, view) = open(cx, &sb);
    cx.read(|cx| assert_eq!(view.read(cx).page(), Page::Overview, "the home page"));
    assert_eq!(
        overview_files(&view, cx),
        vec![
            (".hello".into(), FileState::ChangedHere),
            (".profile".into(), FileState::UpdateAvailable),
        ]
    );
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).overview().read(cx).selected(),
            Some(&Selection::File(".hello".into()))
        );
    });

    click(cx, handle, &["keep-here"]);
    assert_eq!(
        fs::read_to_string(sb.source.join("dot_hello")).unwrap(),
        "my edit\n"
    );
    // Now only the update is left; it is selected.
    click(cx, handle, &["item:.profile", "take-shared"]);
    assert_eq!(sb.home(".profile"), "profile v2\n");
    assert!(overview_files(&view, cx).is_empty());
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("check-again").is_some(), "shows In sync");
    })
    .unwrap();
}

#[gpui_kit::test]
fn overview_decisions_ask_before_losing_work(cx: &mut TestAppContext) {
    let sb = Sandbox::new(&[("dot_hello", "hello\n"), ("dot_gone", "gone\n")]);
    sb.chezmoi().apply(&[], &ApplyOptions::default()).unwrap();
    fs::write(sb.home.join(".hello"), "mine\n").unwrap();
    fs::write(sb.source.join("dot_hello"), "theirs\n").unwrap();
    fs::remove_file(sb.home.join(".gone")).unwrap();

    let (handle, view) = open(cx, &sb);
    assert_eq!(
        overview_files(&view, cx),
        vec![
            (".gone".into(), FileState::DeletedHere),
            (".hello".into(), FileState::BothChanged),
        ]
    );
    click(cx, handle, &["restore"]);
    assert_eq!(sb.home(".gone"), "gone\n");

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("take-shared", cx);
        assert!(window.has_active_dialog(cx), "replacing must ask first");
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        sb.home(".hello"),
        "mine\n",
        "nothing changes before confirming"
    );
    click(cx, handle, &["ok"]);
    assert_eq!(sb.home(".hello"), "theirs\n");
    assert!(overview_files(&view, cx).is_empty());
}

/// Two computers share a bare remote. The laptop changes a file and syncs;
/// the desktop syncs, reviews the plan, unchecks one of its own changes,
/// and confirms.
#[gpui_kit::test]
fn sync_brings_computers_together(cx: &mut TestAppContext) {
    let mut desktop = Sandbox::new(&[]);
    let base = desktop.home.parent().unwrap().to_path_buf();
    let config = base.join("gitconfig");
    fs::write(
        &config,
        "[user]\n\tname = Test\n\temail = test@example.com\n[commit]\n\tgpgsign = false\n[init]\n\tdefaultBranch = main\n",
    )
    .unwrap();
    let env = &mut desktop.options.env;
    env.push(("GIT_CONFIG_GLOBAL".into(), config.clone().into()));
    env.push(("GIT_CONFIG_NOSYSTEM".into(), "1".into()));
    let remote = base.join("remote.git");
    git(
        &base,
        &config,
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );
    let laptop = base.join("laptop");
    git(
        &base,
        &config,
        &[
            "clone",
            "-q",
            remote.to_str().unwrap(),
            laptop.to_str().unwrap(),
        ],
    );
    for (name, contents) in [("dot_hello", "hello\n"), ("dot_a", "a\n"), ("dot_b", "b\n")] {
        fs::write(laptop.join(name), contents).unwrap();
    }
    git(&laptop, &config, &["add", "-A"]);
    git(&laptop, &config, &["commit", "-q", "-m", "Initial"]);
    git(&laptop, &config, &["push", "-q", "origin", "main"]);
    git(
        &desktop.source,
        &config,
        &["clone", "-q", remote.to_str().unwrap(), "."],
    );
    desktop
        .chezmoi()
        .apply(&[], &ApplyOptions::default())
        .unwrap();

    // The laptop changes ~/.hello and uploads it; the desktop changes two
    // files of its own.
    fs::write(laptop.join("dot_hello"), "from laptop\n").unwrap();
    git(&laptop, &config, &["commit", "-q", "-am", "Laptop"]);
    git(&laptop, &config, &["push", "-q"]);
    fs::write(desktop.home.join(".a"), "a from desktop\n").unwrap();
    fs::write(desktop.home.join(".b"), "b, keep it here\n").unwrap();

    let (handle, view) = open(cx, &desktop);
    assert_eq!(
        overview_files(&view, cx),
        vec![
            (".a".into(), FileState::ChangedHere),
            (".b".into(), FileState::ChangedHere),
        ],
        "the laptop's change is not known before syncing"
    );

    // Step one pulls and opens the review; nothing on this computer changes.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("sync", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.has_active_dialog(cx), "the plan is shown first");
        window.click(SharedString::from("save:.b"), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(desktop.home(".hello"), "hello\n");
    assert_eq!(
        fs::read_to_string(desktop.source.join("dot_hello")).unwrap(),
        "from laptop\n",
        "the shared version was pulled"
    );

    click(cx, handle, &["ok"]);
    assert_eq!(desktop.home(".hello"), "from laptop\n");
    assert_eq!(
        fs::read_to_string(desktop.source.join("dot_a")).unwrap(),
        "a from desktop\n"
    );
    assert_eq!(
        fs::read_to_string(desktop.source.join("dot_b")).unwrap(),
        "b\n",
        "unchecked changes stay on this computer"
    );
    assert_eq!(
        git(&remote, &config, &["show", "main:dot_a"]),
        "a from desktop\n",
        "uploaded"
    );
    assert_eq!(
        overview_files(&view, cx),
        vec![(".b".into(), FileState::ChangedHere)]
    );
}

/// Without the remote, a sync still saves and commits on this computer;
/// only uploading fails.
#[gpui_kit::test]
fn sync_works_offline(cx: &mut TestAppContext) {
    let mut sb = Sandbox::new(&[]);
    let base = sb.home.parent().unwrap().to_path_buf();
    let config = base.join("gitconfig");
    fs::write(
        &config,
        "[user]\n\tname = Test\n\temail = test@example.com\n[commit]\n\tgpgsign = false\n[init]\n\tdefaultBranch = main\n",
    )
    .unwrap();
    let env = &mut sb.options.env;
    env.push(("GIT_CONFIG_GLOBAL".into(), config.clone().into()));
    env.push(("GIT_CONFIG_NOSYSTEM".into(), "1".into()));
    let remote = base.join("remote.git");
    git(
        &base,
        &config,
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );
    fs::write(sb.source.join("dot_hello"), "hello\n").unwrap();
    git(&sb.source, &config, &["init", "-q"]);
    git(&sb.source, &config, &["add", "-A"]);
    git(&sb.source, &config, &["commit", "-q", "-m", "Initial"]);
    git(
        &sb.source,
        &config,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&sb.source, &config, &["push", "-q", "-u", "origin", "main"]);
    sb.chezmoi().apply(&[], &ApplyOptions::default()).unwrap();
    fs::remove_dir_all(&remote).unwrap();
    fs::write(sb.home.join(".hello"), "edited offline\n").unwrap();

    let (handle, view) = open(cx, &sb);
    let offline = std::rc::Rc::new(std::cell::Cell::new(false));
    let _subscription = cx.update(|cx| {
        let model = view.read(cx).model().clone();
        let offline = offline.clone();
        cx.subscribe(&model, move |_, event: &DotfilesEvent, _| {
            if let DotfilesEvent::SyncReady(prepared) = event {
                offline.set(prepared.offline.is_some());
            }
        })
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("sync", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        offline.get(),
        "the review warns that the remote is unreachable"
    );
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.has_active_dialog(cx), "the plan is still shown");
        assert!(window.try_find("save:.hello").is_some());
    })
    .unwrap();
    click(cx, handle, &["ok"]);
    assert_eq!(
        fs::read_to_string(sb.source.join("dot_hello")).unwrap(),
        "edited offline\n"
    );
    assert_eq!(
        git(&sb.source, &config, &["log", "-1", "--format=%s"]).trim(),
        "Update ~/.hello"
    );
    assert!(overview_files(&view, cx).is_empty());
    cx.read(|cx| {
        let snapshot = view.read(cx).model().read(cx).snapshot().cloned().unwrap();
        let shared = snapshot.overview.shared.clone().unwrap();
        assert_eq!(shared.ahead, 1, "waiting to be uploaded");
    });
}

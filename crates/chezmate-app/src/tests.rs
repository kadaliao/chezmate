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

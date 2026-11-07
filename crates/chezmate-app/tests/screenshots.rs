//! Renders the app to PNG files for a visual check, with real Metal
//! rendering in an off-screen window (works while the screen is locked).
//! The data is a sandboxed two-computer setup, never the real dotfiles.
//!
//! ```sh
//! CHEZMATE_SCREENSHOTS=/tmp/shots CHEZMATE_LANG=zh \
//!   cargo test -p chezmate-app --test screenshots
//! ```
//!
//! Without `CHEZMATE_SCREENSHOTS` it does nothing. It has its own `main`
//! because the macOS platform must start on the main thread, which the
//! standard test harness does not use; the app's modules are included by
//! path since the app is a binary crate.

#![allow(dead_code, unused_imports)]

#[path = "../src/app.rs"]
mod app;
#[path = "../src/changes.rs"]
mod changes;
#[path = "../src/environment.rs"]
mod environment;
#[path = "../src/files.rs"]
mod files;
#[path = "../src/i18n.rs"]
mod i18n;
#[path = "../src/model.rs"]
mod model;
#[path = "../src/overview.rs"]
mod overview;
#[path = "../src/repository.rs"]
mod repository;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use chezmate_core::ChezmoiOptions;
use gpui_kit::component::{Theme, ThemeMode, WindowExt as _};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, VisualTestAppContext, px, size};

use crate::app::{Chezmate, Page};
use crate::overview::Selection;

fn git(dir: &Path, config: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A desktop whose files are in every state, plus a laptop that pushed
/// changes. Returns chezmoi options for the desktop.
fn demo(base: &Path) -> ChezmoiOptions {
    let config = base.join("gitconfig");
    write(
        &config,
        "[user]\n\tname = Demo\n\temail = demo@example.com\n[commit]\n\tgpgsign = false\n[init]\n\tdefaultBranch = main\n",
    );
    let remote = base.join("remote.git");
    let laptop = base.join("laptop");
    let (home, source, state) = (base.join("home"), base.join("source"), base.join("state"));
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&state).unwrap();
    write(&state.join("chezmoi.toml"), "");
    git(
        base,
        &config,
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );
    git(
        base,
        &config,
        &[
            "clone",
            "-q",
            remote.to_str().unwrap(),
            laptop.to_str().unwrap(),
        ],
    );
    for (path, contents) in [
        ("dot_zshrc", "export EDITOR=nvim\nalias ll=\"ls -l\"\n"),
        (
            "dot_gitconfig.tmpl",
            "[user]\n  name = Demo\n[core]\n  editor = {{ if eq .chezmoi.os \"darwin\" }}nvim{{ else }}vim{{ end }}\n",
        ),
        (
            "dot_config/fish/config.fish",
            "set -gx PATH $HOME/.local/bin $PATH\n",
        ),
        ("dot_vimrc", "set number\nsyntax on\n"),
        ("dot_tmux.conf", "set -g mouse on\n"),
        (
            "run_once_install-tools.sh",
            "#!/bin/sh\necho installing tools\n",
        ),
        (
            "run_after_ensure-brewfile.sh",
            "#!/bin/sh\necho ensure brewfile\n",
        ),
    ] {
        write(&laptop.join(path), contents);
    }
    git(&laptop, &config, &["add", "-A"]);
    git(&laptop, &config, &["commit", "-qm", "Initial"]);
    git(&laptop, &config, &["push", "-q", "origin", "main"]);
    git(
        base,
        &config,
        &[
            "clone",
            "-q",
            remote.to_str().unwrap(),
            source.to_str().unwrap(),
        ],
    );

    let options = ChezmoiOptions {
        source: Some(source.clone()),
        destination: Some(home.clone()),
        config: Some(state.join("chezmoi.toml")),
        persistent_state: Some(state.join("state.boltdb")),
        cache: Some(base.join("cache")),
        env: vec![
            ("HOME".into(), home.clone().into()),
            ("GIT_CONFIG_GLOBAL".into(), config.clone().into()),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        ],
        ..Default::default()
    };
    chezmate_core::Chezmoi::new(options.clone())
        .unwrap()
        .apply(&[], &Default::default())
        .unwrap();

    // The laptop changes two files and adds one.
    write(
        &laptop.join("dot_config/fish/config.fish"),
        "set -gx PATH $HOME/.local/bin $PATH\nset -g fish_greeting \"\"\n",
    );
    write(
        &laptop.join("dot_vimrc"),
        "set number\nsyntax on\nset relativenumber\n",
    );
    write(&laptop.join("dot_config/git/ignore"), "*.log\n.DS_Store\n");
    git(&laptop, &config, &["add", "-A"]);
    git(&laptop, &config, &["commit", "-qm", "Laptop edits"]);
    git(&laptop, &config, &["push", "-q"]);

    // The desktop: edits, a deleted file, and the laptop's commits pulled.
    write(
        &home.join(".zshrc"),
        "export EDITOR=nvim\nalias ll=\"ls -l\"\nalias gs=\"git status\"\n",
    );
    write(
        &home.join(".gitconfig"),
        "[user]\n  name = Demo\n[core]\n  editor = code\n",
    );
    write(&home.join(".vimrc"), "set number\nsyntax off\n");
    fs::remove_file(home.join(".tmux.conf")).unwrap();
    git(&source, &config, &["pull", "-q"]);
    write(
        &source.join("run_onchange_macos-defaults.sh"),
        "#!/bin/sh\ndefaults write -g KeyRepeat -int 2\n",
    );
    options
}

fn main() {
    let Some(out) = std::env::var_os("CHEZMATE_SCREENSHOTS").map(PathBuf::from) else {
        eprintln!("set CHEZMATE_SCREENSHOTS to a directory");
        return;
    };
    fs::create_dir_all(&out).unwrap();
    let root = tempfile::tempdir().unwrap();
    let base = root.path().canonicalize().unwrap();
    let options = demo(&base);

    let platform = gpui_kit::platform::current_platform(false);
    let mut cx =
        VisualTestAppContext::with_asset_source(platform, Arc::new(gpui_kit::assets::Assets));
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::i18n::init();
        crate::app::init(cx);
    });
    // Wrapped in GPUI Kit's Root, as `gpui_kit::open_window` does, so
    // dialogs and notifications work.
    let mut built = None;
    let window = cx
        .open_offscreen_window(size(px(1180.), px(760.)), |window, cx| {
            let view = cx.new(|cx| Chezmate::new(options, window, cx));
            built = Some(view.clone());
            cx.new(|cx| gpui_kit::base::Root::new(view, window, cx))
        })
        .unwrap();
    let view = built.unwrap();
    let handle: gpui_kit::AnyWindowHandle = window.into();
    cx.run_until_parked();

    let shoot = |cx: &mut VisualTestAppContext, name: &str| {
        cx.run_until_parked();
        // Let dialog and notification animations finish.
        for _ in 0..10 {
            cx.advance_clock(std::time::Duration::from_millis(50));
            cx.update_window(handle, |_, window, cx| window.render_frame(cx))
                .unwrap();
        }
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
        let image = cx.capture_screenshot(handle).unwrap();
        let path = out.join(format!("{name}.png"));
        image.save(&path).unwrap();
        println!("{}", path.display());
    };
    let select = |cx: &mut VisualTestAppContext, selection: Selection| {
        cx.update(|cx| {
            let overview = view.read(cx).overview().clone();
            overview.update(cx, |overview, cx| overview.select(selection, cx));
        });
    };

    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let suffix = if mode == ThemeMode::Dark { "-dark" } else { "" };
        cx.update(|cx| Theme::change(mode, None, cx));
        select(&mut cx, Selection::File(".vimrc".into()));
        shoot(&mut cx, &format!("overview-both-changed{suffix}"));
        if mode == ThemeMode::Dark {
            continue;
        }
        select(&mut cx, Selection::File(".zshrc".into()));
        shoot(&mut cx, "overview-changed-here");
        select(&mut cx, Selection::File(".gitconfig".into()));
        shoot(&mut cx, "overview-template");
        select(&mut cx, Selection::File(".config/git/ignore".into()));
        shoot(&mut cx, "overview-new-file");
        select(&mut cx, Selection::Step("macos-defaults.sh".into()));
        shoot(&mut cx, "overview-setup-step");
    }
    cx.update(|cx| Theme::change(ThemeMode::Light, None, cx));

    // The sync review.
    cx.update(|cx| {
        let model = view.read(cx).model().clone();
        model.update(cx, |model, cx| model.prepare_sync(cx));
    });
    shoot(&mut cx, "sync-review");
    cx.update_window(handle, |_, window, cx| window.close_all_dialogs(cx))
        .unwrap();

    cx.update(|cx| view.update(cx, |view, cx| view.set_page(Page::Changes, cx)));
    shoot(&mut cx, "advanced-changes");
}

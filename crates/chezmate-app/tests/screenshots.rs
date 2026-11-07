//! Renders the app to PNG files for a visual check, with real Metal
//! rendering in an off-screen window (works while the screen is locked).
//! The data is demo mode's two-computer setup (`src/demo.rs`) in a
//! temporary folder, never the real dotfiles.
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
#[path = "../src/assets.rs"]
mod assets;
#[path = "../src/changes.rs"]
mod changes;
#[path = "../src/config.rs"]
mod config;
#[path = "../src/demo.rs"]
mod demo;
#[path = "../src/environment.rs"]
mod environment;
#[path = "../src/files.rs"]
mod files;
#[path = "../src/i18n.rs"]
mod i18n;
#[path = "../src/language.rs"]
mod language;
#[path = "../src/model.rs"]
mod model;
#[path = "../src/overview.rs"]
mod overview;
#[path = "../src/repository.rs"]
mod repository;
#[path = "../src/viewer.rs"]
mod viewer;

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

fn main() {
    let Some(out) = std::env::var_os("CHEZMATE_SCREENSHOTS").map(PathBuf::from) else {
        eprintln!("set CHEZMATE_SCREENSHOTS to a directory");
        return;
    };
    fs::create_dir_all(&out).unwrap();
    let root = tempfile::tempdir().unwrap();
    let base = root.path().canonicalize().unwrap();
    let options = demo::create_in(&base).unwrap();

    let platform = gpui_kit::platform::current_platform(false);
    let mut cx = VisualTestAppContext::with_asset_source(platform, Arc::new(assets::AppAssets));
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

    // The Files page: the tree, a file's details and its contents.
    let files = cx.update(|cx| view.read(cx).files().clone());
    cx.update(|cx| view.update(cx, |view, cx| view.set_page(Page::Files, cx)));
    let reveal = |cx: &mut VisualTestAppContext, target: &str| {
        cx.update_window(handle, |_, window, cx| {
            files.update(cx, |files, cx| files.reveal(target, window, cx));
        })
        .unwrap();
    };
    let tab = |cx: &mut VisualTestAppContext, tab: viewer::ViewTab| {
        cx.update_window(handle, |_, window, cx| {
            let viewer = files
                .read(cx)
                .viewer(cx)
                .cloned()
                .expect("a file is selected");
            viewer.update(cx, |viewer, cx| viewer.set_tab(tab, window, cx));
        })
        .unwrap();
    };
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let suffix = if mode == ThemeMode::Dark { "-dark" } else { "" };
        cx.update(|cx| Theme::change(mode, None, cx));
        reveal(&mut cx, ".local/bin/backup-notes");
        reveal(&mut cx, ".config/nvim/init.lua");
        shoot(&mut cx, &format!("files-details{suffix}"));
        if mode == ThemeMode::Dark {
            continue;
        }
        reveal(&mut cx, ".vimrc");
        shoot(&mut cx, "files-details-changed");
        reveal(&mut cx, ".gitconfig");
        tab(&mut cx, viewer::ViewTab::Source);
        shoot(&mut cx, "files-template-source");
        reveal(&mut cx, ".local/bin/backup-notes");
        shoot(&mut cx, "files-script");
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("edit-file", cx);
            window.input("# edited\n", cx);
        })
        .unwrap();
        shoot(&mut cx, "files-editing");
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("cancel-edit", cx);
        })
        .unwrap();
        reveal(&mut cx, "Library/Application Support/Demo/logo.png");
        shoot(&mut cx, "files-image");
        reveal(&mut cx, "Library/Preferences/com.example.demo.plist");
        shoot(&mut cx, "files-plist");
        reveal(&mut cx, ".config");
        shoot(&mut cx, "files-folder");
        reveal(&mut cx, "scripts:install-tools.sh");
        shoot(&mut cx, "files-run-script");
        reveal(&mut cx, ".netrc");
        shoot(&mut cx, "files-encrypted");
    }
    cx.update(|cx| Theme::change(ThemeMode::Light, None, cx));

    // The Environment page with chezmoi's config and its template.
    cx.update(|cx| view.update(cx, |view, cx| view.set_page(Page::Environment, cx)));
    let config = cx.update(|cx| view.read(cx).environment().read(cx).config().clone());
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let suffix = if mode == ThemeMode::Dark { "-dark" } else { "" };
        cx.update(|cx| Theme::change(mode, None, cx));
        shoot(&mut cx, &format!("environment{suffix}"));
    }
    cx.update(|cx| Theme::change(ThemeMode::Light, None, cx));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("edit-config", cx);
        window.input("# ", cx);
    })
    .unwrap();
    shoot(&mut cx, "environment-editing");
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("cancel-config", cx);
        config.update(cx, |config, cx| {
            config.set_tab(config::ConfigTab::Template, window, cx)
        });
    })
    .unwrap();
    shoot(&mut cx, "environment-template");

    // Diagnostics, with the button that copies them for an agent.
    cx.update(|cx| {
        let model = view.read(cx).model().clone();
        model.update(cx, |model, cx| model.run_doctor(cx));
    });
    shoot(&mut cx, "environment-diagnostics");

    // Offline: the remote is gone.
    fs::remove_dir_all(base.join("remote.git")).unwrap();
    cx.update(|cx| {
        view.update(cx, |view, cx| view.set_page(Page::Overview, cx));
        let model = view.read(cx).model().clone();
        model.update(cx, |model, cx| model.prepare_sync(cx));
    });
    shoot(&mut cx, "sync-review-offline");
}

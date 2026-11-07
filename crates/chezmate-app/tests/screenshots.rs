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
#[path = "../src/assets.rs"]
mod assets;
#[path = "../src/changes.rs"]
mod changes;
#[path = "../src/config.rs"]
mod config;
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
    // age encryption with a key made for the demo, as `chezmoi init`
    // would write it from the template below.
    let chezmoi = chezmate_core::Chezmoi::new(ChezmoiOptions::default())
        .unwrap()
        .binary()
        .to_path_buf();
    let key = state.join("key.txt");
    let keygen = Command::new(&chezmoi)
        .args(["age-keygen", "--output"])
        .arg(&key)
        .output()
        .unwrap();
    assert!(keygen.status.success(), "age-keygen");
    let recipient = fs::read_to_string(&key)
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("# public key: ").map(str::to_string))
        .unwrap();
    write(
        &state.join("chezmoi.toml"),
        &format!(
            "encryption = \"age\"\n\n[age]\n  identity = {:?}\n  recipient = {recipient:?}\n\n[edit]\n  command = \"nvim\"\n\n[data]\n  email = \"demo@example.com\"\n",
            key.display().to_string()
        ),
    );
    let template = format!(
        "{{{{- $email := promptStringOnce . \"email\" \"Email address\" -}}}}\nencryption = \"age\"\n\n[age]\n  identity = \"~/.config/chezmoi/key.txt\"\n  recipient = {recipient:?}\n\n[edit]\n  command = \"nvim\"\n\n[data]\n  email = {{{{ $email | quote }}}}\n"
    );
    let netrc = base.join("netrc");
    write(
        &netrc,
        "machine api.example.com\n  login demo\n  password not-a-real-secret\n",
    );
    let encrypted = Command::new(&chezmoi)
        .arg("--config")
        .arg(state.join("chezmoi.toml"))
        .arg("encrypt")
        .arg(&netrc)
        .output()
        .unwrap();
    assert!(encrypted.status.success(), "chezmoi encrypt");
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
    fs::create_dir_all(&laptop).unwrap();
    fs::write(
        laptop.join("encrypted_private_dot_netrc.age"),
        &encrypted.stdout,
    )
    .unwrap();
    for (path, contents) in [
        (".chezmoi.toml.tmpl", template.as_str()),
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
        (
            "dot_config/nvim/init.lua",
            "-- Neovim settings\nlocal opt = vim.opt\n\nopt.number = true\nopt.relativenumber = true\nopt.expandtab = true\nopt.shiftwidth = 2\n\nvim.keymap.set(\"n\", \"<leader>w\", \"<cmd>write<cr>\", { desc = \"Save\" })\n\nfor _, plugin in ipairs({ \"telescope\", \"treesitter\" }) do\n  pcall(require, plugin)\nend\n",
        ),
        (
            "dot_local/bin/executable_backup-notes",
            "#!/usr/bin/env bash\nset -euo pipefail\n\n# Copy notes to the backup disk.\nrsync -a --delete \"$HOME/Notes/\" \"/Volumes/Backup/Notes/\"\necho \"done: $(date)\"\n",
        ),
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
    // A picture and a binary property list, as real setups have.
    let picture = laptop.join("Library/Application Support/Demo/logo.png");
    fs::create_dir_all(picture.parent().unwrap()).unwrap();
    let status = Command::new("sips")
        .args(["-s", "format", "png", "-Z", "256"])
        .arg("/System/Applications/Calculator.app/Contents/Resources/AppIcon.icns")
        .arg("--out")
        .arg(&picture)
        .output()
        .unwrap();
    assert!(status.status.success(), "sips");
    let plist = laptop.join("Library/Preferences/com.example.demo.plist");
    fs::create_dir_all(plist.parent().unwrap()).unwrap();
    fs::write(
        &plist,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict><key>AppleShowAllFiles</key><true/><key>KeyRepeat</key><integer>2</integer><key>NSUserKeyEquivalents</key><dict><key>Hide Others</key><string>@~h</string></dict></dict></plist>\n",
    )
    .unwrap();
    let status = Command::new("plutil")
        .args(["-convert", "binary1"])
        .arg(&plist)
        .status()
        .unwrap();
    assert!(status.success(), "plutil");
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

    // Offline: the remote is gone.
    fs::remove_dir_all(base.join("remote.git")).unwrap();
    cx.update(|cx| {
        view.update(cx, |view, cx| view.set_page(Page::Overview, cx));
        let model = view.read(cx).model().clone();
        model.update(cx, |model, cx| model.prepare_sync(cx));
    });
    shoot(&mut cx, "sync-review-offline");
}

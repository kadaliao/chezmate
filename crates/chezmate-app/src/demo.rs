//! Demo mode (`chezmate-app --demo`): a made-up setup in a temporary
//! folder, with files in every state, to try Chezmate without touching
//! real dotfiles. Two "computers" share a local repository: this one, with
//! its own edits, and a laptop that has uploaded changes. Everything the
//! app does in demo mode stays in that folder; the scripts only write to a
//! log there.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, ensure};
use chezmate_core::{Chezmoi, ChezmoiOptions};

/// Where demo mode keeps its files. Each start begins afresh.
pub fn folder() -> PathBuf {
    std::env::temp_dir().join("chezmate-demo")
}

/// Makes a new demo setup in [`folder`] and returns chezmoi options for it.
pub fn create() -> Result<ChezmoiOptions> {
    let base = folder();
    if base.exists() {
        fs::remove_dir_all(&base).with_context(|| format!("could not clear {}", base.display()))?;
    }
    fs::create_dir_all(&base)?;
    create_in(&base.canonicalize()?)
}

fn run(command: &mut Command, what: &str) -> Result<Vec<u8>> {
    let output = command
        .output()
        .with_context(|| format!("could not run {what}"))?;
    ensure!(
        output.status.success(),
        "{what} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(output.stdout)
}

fn git(dir: &Path, config: &Path, args: &[&str]) -> Result<()> {
    run(
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", config)
            .env("GIT_CONFIG_NOSYSTEM", "1"),
        &format!("git {}", args.join(" ")),
    )
    .map(drop)
}

fn write(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents).with_context(|| format!("could not write {}", path.display()))
}

/// Builds the demo setup in `base`, an empty folder given by its canonical
/// path (chezmoi reports paths with symlinks resolved).
pub fn create_in(base: &Path) -> Result<ChezmoiOptions> {
    let gitconfig = base.join("gitconfig");
    write(
        &gitconfig,
        "[user]\n\tname = Demo\n\temail = demo@example.com\n[commit]\n\tgpgsign = false\n[init]\n\tdefaultBranch = main\n",
    )?;
    let remote = base.join("remote.git");
    let laptop = base.join("laptop");
    let (home, source, state) = (base.join("home"), base.join("source"), base.join("state"));
    fs::create_dir_all(&home)?;
    fs::create_dir_all(&state)?;

    // age encryption with a key made for the demo, as `chezmoi init`
    // would write it from the template below.
    let chezmoi = Chezmoi::new(ChezmoiOptions::default())
        .map_err(|err| anyhow::anyhow!("{err}"))?
        .binary()
        .to_path_buf();
    let key = state.join("key.txt");
    run(
        Command::new(&chezmoi)
            .args(["age-keygen", "--output"])
            .arg(&key),
        "chezmoi age-keygen",
    )?;
    let recipient = fs::read_to_string(&key)?
        .lines()
        .find_map(|line| line.strip_prefix("# public key: ").map(str::to_string))
        .context("no public key in the age key file")?;
    let config = state.join("chezmoi.toml");
    write(
        &config,
        format!(
            "encryption = \"age\"\n\n[age]\n  identity = {:?}\n  recipient = {recipient:?}\n\n[edit]\n  command = \"nvim\"\n\n[data]\n  email = \"demo@example.com\"\n",
            key.display().to_string()
        ),
    )?;
    let template = format!(
        "{{{{- $email := promptStringOnce . \"email\" \"Email address\" -}}}}\nencryption = \"age\"\n\n[age]\n  identity = \"~/.config/chezmoi/key.txt\"\n  recipient = {recipient:?}\n\n[edit]\n  command = \"nvim\"\n\n[data]\n  email = {{{{ $email | quote }}}}\n"
    );
    let netrc = base.join("netrc");
    write(
        &netrc,
        "machine api.example.com\n  login demo\n  password not-a-real-secret\n",
    )?;
    let encrypted = run(
        Command::new(&chezmoi)
            .arg("--config")
            .arg(&config)
            .arg("encrypt")
            .arg(&netrc),
        "chezmoi encrypt",
    )?;

    // The laptop sets up the shared version.
    git(
        base,
        &gitconfig,
        &["init", "-q", "--bare", path_str(&remote)?],
    )?;
    git(
        base,
        &gitconfig,
        &["clone", "-q", path_str(&remote)?, path_str(&laptop)?],
    )?;
    write(&laptop.join("encrypted_private_dot_netrc.age"), encrypted)?;
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
        // Scripts run for real when the demo syncs, so they only write to
        // a log in the demo's home folder.
        (
            "run_once_install-tools.sh",
            "#!/bin/sh\n# Demo: a real setup would install tools here.\necho \"install tools\" >> \"$HOME/demo.log\"\n",
        ),
        (
            "run_after_ensure-brewfile.sh",
            "#!/bin/sh\n# Demo: a real setup would run `brew bundle` here.\necho \"ensure brewfile\" >> \"$HOME/demo.log\"\n",
        ),
    ] {
        write(&laptop.join(path), contents)?;
    }
    // A picture and a binary property list, as real setups have.
    let picture = laptop.join("Library/Application Support/Demo/logo.png");
    fs::create_dir_all(picture.parent().unwrap())?;
    run(
        Command::new("/usr/bin/sips")
            .args(["-s", "format", "png", "-Z", "256"])
            .arg("/System/Applications/Calculator.app/Contents/Resources/AppIcon.icns")
            .arg("--out")
            .arg(&picture),
        "sips",
    )?;
    let plist = laptop.join("Library/Preferences/com.example.demo.plist");
    write(
        &plist,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict><key>AppleShowAllFiles</key><true/><key>KeyRepeat</key><integer>2</integer><key>NSUserKeyEquivalents</key><dict><key>Hide Others</key><string>@~h</string></dict></dict></plist>\n",
    )?;
    run(
        Command::new("/usr/bin/plutil")
            .args(["-convert", "binary1"])
            .arg(&plist),
        "plutil",
    )?;
    git(&laptop, &gitconfig, &["add", "-A"])?;
    git(&laptop, &gitconfig, &["commit", "-qm", "Initial"])?;
    git(&laptop, &gitconfig, &["push", "-q", "origin", "main"])?;

    // This computer joins: clones the shared version and applies it.
    git(
        base,
        &gitconfig,
        &["clone", "-q", path_str(&remote)?, path_str(&source)?],
    )?;
    let options = ChezmoiOptions {
        source: Some(source.clone()),
        destination: Some(home.clone()),
        config: Some(config),
        persistent_state: Some(state.join("state.boltdb")),
        cache: Some(base.join("cache")),
        env: vec![
            ("HOME".into(), home.clone().into()),
            ("GIT_CONFIG_GLOBAL".into(), gitconfig.clone().into()),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        ],
        ..Default::default()
    };
    Chezmoi::new(options.clone())
        .and_then(|chezmoi| chezmoi.apply(&[], &Default::default()))
        .map_err(|err| anyhow::anyhow!("{err}"))?;

    // The laptop changes two files and adds one.
    write(
        &laptop.join("dot_config/fish/config.fish"),
        "set -gx PATH $HOME/.local/bin $PATH\nset -g fish_greeting \"\"\n",
    )?;
    write(
        &laptop.join("dot_vimrc"),
        "set number\nsyntax on\nset relativenumber\n",
    )?;
    write(&laptop.join("dot_config/git/ignore"), "*.log\n.DS_Store\n")?;
    git(&laptop, &gitconfig, &["add", "-A"])?;
    git(&laptop, &gitconfig, &["commit", "-qm", "Laptop edits"])?;
    git(&laptop, &gitconfig, &["push", "-q"])?;

    // This computer: edits, a deleted file, and the laptop's commits
    // pulled, plus a new setup step.
    write(
        &home.join(".zshrc"),
        "export EDITOR=nvim\nalias ll=\"ls -l\"\nalias gs=\"git status\"\n",
    )?;
    write(
        &home.join(".gitconfig"),
        "[user]\n  name = Demo\n[core]\n  editor = code\n",
    )?;
    write(&home.join(".vimrc"), "set number\nsyntax off\n")?;
    fs::remove_file(home.join(".tmux.conf"))?;
    git(&source, &gitconfig, &["pull", "-q"])?;
    write(
        &source.join("run_onchange_macos-defaults.sh"),
        "#!/bin/sh\n# Demo: a real setup would run `defaults write -g KeyRepeat -int 2` here.\necho \"KeyRepeat 2\" >> \"$HOME/demo.log\"\n",
    )?;
    Ok(options)
}

fn path_str(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("{} is not valid UTF-8", path.display()))
}

//! End-to-end tests against the real chezmoi executable in a sandbox.

mod common;

use chezmate_core::{
    AddOptions, ApplyOptions, Change, CheckResult, DiffOptions, EntryKind, StatusEntry,
};
use common::Sandbox;

/// chezmoi's `.chezmoi.os` uses Go's names (`darwin`), not Rust's (`macos`).
fn go_os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    }
}

fn entry<'a>(status: &'a [StatusEntry], path: &str) -> &'a StatusEntry {
    status
        .iter()
        .find(|e| e.path == path)
        .unwrap_or_else(|| panic!("{path} missing from status {status:?}"))
}

fn sample() -> Sandbox {
    let sb = Sandbox::new();
    sb.write_source("dot_hello", "hello\n");
    sb.write_source("dot_profile.tmpl", "os={{ .chezmoi.os }}\n");
    sb.write_source("private_dot_ssh/config", "Host *\n");
    sb.write_source(
        "run_once_setup.sh",
        "#!/bin/sh\necho ran >> \"$HOME/setup.log\"\n",
    );
    sb.write_source("symlink_dot_link", ".hello\n");
    sb
}

#[test]
fn info_reports_sandbox_paths() {
    let sb = sample();
    let info = sb.chezmoi.info().unwrap();
    assert!(!info.version.is_empty());
    assert_eq!(info.paths.source_dir, sb.source);
    assert_eq!(info.paths.dest_dir, sb.home);
    assert!(info.source_exists);
    assert!(info.config_exists);
    assert!(!info.source_is_git_repo);
    assert_eq!(info.config_template, None);

    // `chezmoi init` makes the config file from this template.
    sb.write_source(".chezmoi.toml.tmpl", "[data]\n");
    let info = sb.chezmoi.info().unwrap();
    assert_eq!(
        info.config_template,
        Some(sb.source.join(".chezmoi.toml.tmpl"))
    );
}

#[test]
fn status_and_managed_describe_a_fresh_source() {
    let sb = sample();
    let status = sb.chezmoi.status(&[]).unwrap();
    assert_eq!(entry(&status, ".hello").apply, Change::Added);
    assert_eq!(entry(&status, ".profile").apply, Change::Added);
    assert!(entry(&status, "setup.sh").is_script());
    assert!(status.iter().all(|e| !e.has_local_changes()));

    let managed = sb.chezmoi.managed().unwrap();
    let find = |t: &str| managed.iter().find(|e| e.target == t).unwrap();
    assert!(find(".profile").attributes.template);
    assert_eq!(find(".ssh").kind, EntryKind::Directory);
    assert!(find(".ssh").attributes.private);
    assert_eq!(find("setup.sh").kind, EntryKind::Script);
    assert!(find("setup.sh").attributes.once);
    assert_eq!(find(".link").kind, EntryKind::Symlink);
    assert_eq!(
        find(".ssh/config").source_relative,
        "private_dot_ssh/config"
    );
}

#[test]
fn diff_renders_templates() {
    let sb = sample();
    let diff = sb
        .chezmoi
        .diff(&[".profile".into()], &DiffOptions::default())
        .unwrap();
    assert!(
        diff.starts_with("diff --git a/.profile b/.profile"),
        "{diff}"
    );
    assert!(diff.contains(&format!("+os={}", go_os())), "{diff}");
    assert!(!diff.contains(".hello"));
}

#[test]
fn apply_dry_run_then_apply() {
    let sb = sample();
    let outcome = sb
        .chezmoi
        .apply(
            &[".hello".into()],
            &ApplyOptions {
                dry_run: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(outcome.output.contains("+hello"), "{}", outcome.output);
    assert!(!sb.home.join(".hello").exists(), "dry run must not write");

    sb.chezmoi.apply(&[], &ApplyOptions::default()).unwrap();
    assert_eq!(sb.read_home(".hello"), "hello\n");
    assert_eq!(sb.read_home("setup.log"), "ran\n");
    assert!(sb.chezmoi.status(&[]).unwrap().is_empty());

    // run_once scripts do not run again.
    sb.chezmoi.apply(&[], &ApplyOptions::default()).unwrap();
    assert_eq!(sb.read_home("setup.log"), "ran\n");
}

#[test]
fn local_edits_are_reported_and_protected() {
    let sb = sample();
    sb.chezmoi.apply(&[], &ApplyOptions::default()).unwrap();
    sb.write_home(".hello", "hello\nlocal edit\n");
    sb.write_source("dot_hello", "hello\nfrom source\n");

    let status = sb.chezmoi.status(&[]).unwrap();
    let hello = entry(&status, ".hello");
    assert_eq!(hello.local, Change::Modified);
    assert_eq!(hello.apply, Change::Modified);

    let err = sb
        .chezmoi
        .apply(&[".hello".into()], &ApplyOptions::default())
        .unwrap_err();
    assert_eq!(err.kind(), "conflict", "{err}");
    assert_eq!(sb.read_home(".hello"), "hello\nlocal edit\n");

    sb.chezmoi
        .apply(
            &[".hello".into()],
            &ApplyOptions {
                force: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(sb.read_home(".hello"), "hello\nfrom source\n");
}

#[test]
fn re_add_copies_edits_but_skips_templates() {
    let sb = sample();
    sb.chezmoi.apply(&[], &ApplyOptions::default()).unwrap();
    sb.write_home(".hello", "edited\n");
    sb.write_home(".profile", "edited\n");

    let outcome = sb
        .chezmoi
        .re_add(&[".hello".into(), ".profile".into()])
        .unwrap();
    assert_eq!(outcome.updated, vec![".hello".to_string()]);
    assert_eq!(outcome.skipped.len(), 1);
    assert_eq!(outcome.skipped[0].target, ".profile");
    assert_eq!(sb.read_source("dot_hello"), "edited\n");
    assert_eq!(sb.read_source("dot_profile.tmpl"), "os={{ .chezmoi.os }}\n");

    let err = sb.chezmoi.re_add(&[".unmanaged".into()]).unwrap_err();
    assert_eq!(err.kind(), "invalid_request");
}

#[test]
fn add_and_forget() {
    let sb = sample();
    sb.write_home(".newrc", "set x\n");
    sb.chezmoi
        .add(&[".newrc".into()], &AddOptions::default())
        .unwrap();
    assert_eq!(sb.read_source("dot_newrc"), "set x\n");
    assert!(
        sb.chezmoi
            .managed()
            .unwrap()
            .iter()
            .any(|e| e.target == ".newrc")
    );

    sb.chezmoi.forget(&[".newrc".into()]).unwrap();
    assert!(!sb.source.join("dot_newrc").exists());
    // forget leaves the destination file alone.
    assert_eq!(sb.read_home(".newrc"), "set x\n");
}

#[test]
fn add_reports_chezmoi_errors() {
    let sb = sample();
    sb.write_home(".secret", "token\n");
    let err = sb
        .chezmoi
        .add(
            &[".secret".into()],
            &AddOptions {
                encrypt: true,
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(err.kind(), "command_failed");
    assert!(
        err.to_string().contains("encryption not configured"),
        "{err}"
    );
}

#[test]
fn targets_outside_destination_are_rejected() {
    let sb = sample();
    let err = sb.chezmoi.status(&["../etc/passwd".into()]).unwrap_err();
    assert_eq!(err.kind(), "invalid_request");
}

#[test]
fn cat_and_source_path() {
    let sb = sample();
    assert_eq!(
        sb.chezmoi.cat(".profile").unwrap(),
        format!("os={}\n", go_os())
    );
    assert_eq!(
        sb.chezmoi.source_path(".profile").unwrap(),
        sb.source.join("dot_profile.tmpl")
    );
}

#[test]
fn doctor_lists_checks() {
    // doctor runs `--version` on every tool it finds; some (1Password's `op`)
    // can block. Limit PATH to system directories so the test is repeatable.
    let sb = Sandbox::with_env(vec![("PATH".into(), "/usr/bin:/bin".into())]);
    let checks = sb.chezmoi.doctor().unwrap();
    let version = checks.iter().find(|c| c.check == "version").unwrap();
    assert_eq!(version.result, CheckResult::Ok);
    assert!(checks.iter().any(|c| c.check == "source-dir"));
}

#[test]
fn scripts_that_run_on_every_apply_are_marked() {
    let sb = sample();
    sb.write_source(
        "run_after_ensure.sh",
        "#!/bin/sh\necho x >> \"$HOME/ensure.log\"\n",
    );
    let status = sb.chezmoi.status(&[]).unwrap();
    assert!(entry(&status, "ensure.sh").runs_every_apply);
    assert!(
        !entry(&status, "setup.sh").runs_every_apply,
        "run_once is pending"
    );

    sb.chezmoi.apply(&[], &ApplyOptions::default()).unwrap();
    let status = sb.chezmoi.status(&[]).unwrap();
    // chezmoi keeps listing it, but nothing is pending any more.
    assert_eq!(status.len(), 1);
    assert!(status[0].runs_every_apply && !status[0].is_pending());
    sb.chezmoi.apply(&[], &ApplyOptions::default()).unwrap();
    assert_eq!(sb.read_home("ensure.log"), "x\nx\n");
}

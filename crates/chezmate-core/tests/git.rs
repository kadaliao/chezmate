//! Git operations on a sandboxed source repository with a local bare
//! remote and a second clone standing in for another machine.

mod common;

use std::fs;
use std::path::Path;
use std::process::Command;

use chezmate_core::{Change, GitFileStatus};
use common::Sandbox;

type Env = Vec<(std::ffi::OsString, std::ffi::OsString)>;

/// Isolates git from the developer's global config (identity, signing,
/// hooks, default branch). Keep the returned directory alive.
fn git_env() -> (tempfile::TempDir, Env) {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("gitconfig");
    fs::write(
        &config,
        "[user]\n\tname = Test\n\temail = test@example.com\n[commit]\n\tgpgsign = false\n[init]\n\tdefaultBranch = main\n[pull]\n\trebase = false\n",
    )
    .unwrap();
    let env = vec![
        ("GIT_CONFIG_GLOBAL".into(), config.into()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ];
    (dir, env)
}

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

struct Setup {
    sb: Sandbox,
    env: Env,
    other: std::path::PathBuf,
    _git_config: tempfile::TempDir,
}

/// Source repo with one pushed commit, a bare remote, and a second clone.
fn setup() -> Setup {
    let (git_config, env) = git_env();
    let sb = Sandbox::with_env(env.clone());
    let base = sb.root.path().canonicalize().unwrap();
    let remote = base.join("remote.git");
    git(
        &base,
        &env,
        &["init", "--bare", "-q", remote.to_str().unwrap()],
    );
    sb.write_source("dot_hello", "hello\n");
    git(&sb.source, &env, &["init", "-q"]);
    git(&sb.source, &env, &["add", "-A"]);
    git(
        &sb.source,
        &env,
        &["commit", "-q", "-m", "Initial dotfiles"],
    );
    git(
        &sb.source,
        &env,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&sb.source, &env, &["push", "-q", "-u", "origin", "main"]);
    let other = base.join("other");
    git(
        &base,
        &env,
        &[
            "clone",
            "-q",
            remote.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    Setup {
        sb,
        env,
        other,
        _git_config: git_config,
    }
}

#[test]
fn status_diff_and_commit() {
    let Setup {
        sb, _git_config, ..
    } = setup();
    let git = sb.chezmoi.git().unwrap();
    assert!(git.is_repository());
    let status = git.status().unwrap();
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert_eq!(status.upstream.as_deref(), Some("origin/main"));
    assert!(status.is_clean());
    assert_eq!(git.log(10).unwrap()[0].subject, "Initial dotfiles");

    sb.write_source("dot_hello", "hello\nworld\n");
    sb.write_source("dot_new", "new\n");
    let status = git.status().unwrap();
    let find = |p: &str| status.changes.iter().find(|c| c.path == p).unwrap().status;
    assert_eq!(find("dot_hello"), GitFileStatus::Modified);
    assert_eq!(find("dot_new"), GitFileStatus::Untracked);

    let diff = git.diff().unwrap();
    assert!(diff.contains("+world"), "{diff}");
    assert!(
        diff.contains("+++ b/dot_new"),
        "untracked files are diffed: {diff}"
    );

    assert_eq!(git.commit_all("  ").unwrap_err().kind(), "invalid_request");
    let hash = git.commit_all("Update hello").unwrap();
    assert_eq!(hash.len(), 40);
    let status = git.status().unwrap();
    assert!(status.is_clean());
    assert_eq!(status.ahead, 1);
    assert_eq!(
        git.commit_all("again").unwrap_err().kind(),
        "invalid_request"
    );

    git.push().unwrap();
    assert_eq!(git.status().unwrap().ahead, 0);
}

#[test]
fn pull_brings_changes_from_another_machine_without_applying() {
    let Setup {
        sb,
        env,
        other,
        _git_config,
    } = setup();
    sb.chezmoi.apply(&[], &Default::default()).unwrap();
    fs::write(other.join("dot_hello"), "hello from laptop\n").unwrap();
    git(&other, &env, &["commit", "-q", "-am", "Edit on laptop"]);
    git(&other, &env, &["push", "-q"]);

    let git_ops = sb.chezmoi.git().unwrap();
    git_ops.fetch().unwrap();
    assert_eq!(git_ops.status().unwrap().behind, 1);
    git_ops.pull().unwrap();
    assert_eq!(git_ops.status().unwrap().behind, 0);
    assert_eq!(sb.read_source("dot_hello"), "hello from laptop\n");

    // Pulling only updates the source; the change now waits in status.
    assert_eq!(sb.read_home(".hello"), "hello\n");
    let status = sb.chezmoi.status(&[]).unwrap();
    assert_eq!(status[0].path, ".hello");
    assert_eq!(status[0].apply, Change::Modified);
}

#[test]
fn conflicting_pull_leaves_the_repository_unchanged() {
    let Setup {
        sb,
        env,
        other,
        _git_config,
    } = setup();
    fs::write(other.join("dot_hello"), "laptop\n").unwrap();
    git(&other, &env, &["commit", "-q", "-am", "Laptop edit"]);
    git(&other, &env, &["push", "-q"]);

    let git_ops = sb.chezmoi.git().unwrap();
    sb.write_source("dot_hello", "desktop\n");
    git_ops.commit_all("Desktop edit").unwrap();
    let head = git_ops.status().unwrap().head;

    let err = git_ops.pull().unwrap_err();
    assert_eq!(err.kind(), "command_failed");
    assert!(err.to_string().contains("aborted the rebase"), "{err}");
    let status = git_ops.status().unwrap();
    assert_eq!(status.head, head);
    assert!(status.is_clean());
    assert_eq!(sb.read_source("dot_hello"), "desktop\n");
}

#[test]
fn repository_without_commits() {
    let (_git_config, env) = git_env();
    let sb = Sandbox::with_env(env.clone());
    git(&sb.source, &env, &["init", "-q"]);
    sb.write_source("dot_hello", "hello\n");
    let git_ops = sb.chezmoi.git().unwrap();
    assert!(git_ops.log(5).unwrap().is_empty());
    let status = git_ops.status().unwrap();
    assert_eq!(status.head, None);
    assert!(git_ops.diff().unwrap().contains("+hello"));
    git_ops.commit_all("First").unwrap();
    assert_eq!(git_ops.log(5).unwrap().len(), 1);
}

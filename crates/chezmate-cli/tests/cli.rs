//! Runs the `chezmate` binary against a sandboxed real chezmoi.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

struct Sandbox {
    _root: TempDir,
    home: PathBuf,
    source: PathBuf,
    config_dir: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        let (home, source, config_dir) =
            (base.join("home"), base.join("source"), base.join("config"));
        for dir in [&home, &source, &config_dir] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(config_dir.join("chezmoi.toml"), "").unwrap();
        fs::write(source.join("dot_hello"), "hello\n").unwrap();
        Self {
            _root: root,
            home,
            source,
            config_dir,
        }
    }

    /// Runs `chezmate --json <args>` from an unrelated working directory and
    /// returns (exit success, parsed JSON).
    fn json(&self, args: &[&str]) -> (bool, Value) {
        let output = Command::new(env!("CARGO_BIN_EXE_chezmate"))
            .arg("--json")
            .args(["--source", path(&self.source)])
            .args(["--destination", path(&self.home)])
            .args(["--config", path(&self.config_dir.join("chezmoi.toml"))])
            .args([
                "--persistent-state",
                path(&self.config_dir.join("state.boltdb")),
            ])
            .args(args)
            .current_dir(std::env::temp_dir())
            .env("HOME", &self.home)
            .output()
            .unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let value: Value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|err| panic!("not JSON ({err}): {stdout:?}"));
        assert_eq!(
            value["ok"].as_bool(),
            Some(output.status.success()),
            "{value}"
        );
        (output.status.success(), value)
    }
}

fn path(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn status_apply_and_conflict_roundtrip() {
    let sb = Sandbox::new();
    let (ok, status) = sb.json(&["status"]);
    assert!(ok);
    assert_eq!(status["data"][0]["path"], ".hello");
    assert_eq!(status["data"][0]["apply"], "added");

    // Relative targets mean the destination, not the working directory.
    let (ok, diff) = sb.json(&["diff", ".hello"]);
    assert!(ok);
    assert!(diff["data"]["diff"].as_str().unwrap().contains("+hello"));

    let (ok, _) = sb.json(&["apply", ".hello"]);
    assert!(ok);
    assert_eq!(
        fs::read_to_string(sb.home.join(".hello")).unwrap(),
        "hello\n"
    );

    fs::write(sb.home.join(".hello"), "mine\n").unwrap();
    fs::write(sb.source.join("dot_hello"), "theirs\n").unwrap();
    let (ok, err) = sb.json(&["apply", ".hello"]);
    assert!(!ok);
    assert_eq!(err["error"]["kind"], "conflict");
    assert_eq!(err["error"]["paths"][0], ".hello");

    let (ok, readd) = sb.json(&["re-add"]);
    assert!(ok);
    assert_eq!(readd["data"]["updated"][0], ".hello");
    assert_eq!(
        fs::read_to_string(sb.source.join("dot_hello")).unwrap(),
        "mine\n"
    );
}

#[test]
fn overview_and_sync() {
    let sb = Sandbox::new();
    fs::write(sb.source.join("dot_profile"), "profile\n").unwrap();
    fs::write(sb.home.join(".profile"), "my own profile\n").unwrap();
    let (ok, overview) = sb.json(&["overview"]);
    assert!(ok, "{overview}");
    let files = overview["data"]["files"].as_array().unwrap();
    assert_eq!(files[0]["target"], ".hello");
    assert_eq!(files[0]["state"], "update_available");
    assert_eq!(files[0]["incoming"], "added");
    assert_eq!(files[1]["target"], ".profile");
    assert_eq!(files[1]["state"], "both_changed");
    assert_eq!(files[1]["never_synced"], true);
    assert_eq!(overview["data"]["shared"], Value::Null);

    let (ok, dry) = sb.json(&["sync", "--dry-run"]);
    assert!(ok);
    assert_eq!(dry["data"]["plan"]["update"][0], ".hello");
    assert_eq!(dry["data"]["plan"]["decide"][0], ".profile");
    assert!(!sb.home.join(".hello").exists());

    let (ok, done) = sb.json(&["sync"]);
    assert!(ok, "{done}");
    assert_eq!(done["data"]["outcome"]["updated"][0], ".hello");
    assert_eq!(
        fs::read_to_string(sb.home.join(".hello")).unwrap(),
        "hello\n"
    );
    assert_eq!(
        fs::read_to_string(sb.home.join(".profile")).unwrap(),
        "my own profile\n",
        "a file this computer had of its own is never overwritten"
    );
}

#[test]
fn forget_requires_confirmation() {
    let sb = Sandbox::new();
    let (ok, err) = sb.json(&["forget", ".hello"]);
    assert!(!ok);
    assert_eq!(err["error"]["kind"], "invalid_request");
    assert!(sb.source.join("dot_hello").exists());

    let (ok, _) = sb.json(&["forget", "--yes", ".hello"]);
    assert!(ok);
    assert!(!sb.source.join("dot_hello").exists());
}

#[test]
fn reports_missing_chezmoi() {
    let output = Command::new(env!("CARGO_BIN_EXE_chezmate"))
        .args(["--json", "--chezmoi", "/nonexistent/chezmoi", "status"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["kind"], "chezmoi_not_found");
}

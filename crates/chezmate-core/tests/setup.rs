//! Starting out: a computer that has never used chezmoi starts a shared
//! version or fetches one, answers its config template's questions, picks
//! files and connects a remote. Real chezmoi and git; every path is inside
//! a temporary directory, and git's global config is an empty file, as on a
//! computer where git was never set up.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use chezmate_core::{
    AddOptions, Chezmoi, ChezmoiOptions, ConfigAnswer, FileState, QuestionKind, RemoteProblem,
    remote_problem,
};
use tempfile::TempDir;

/// A computer with no chezmoi source directory and no config file.
struct Computer {
    home: PathBuf,
    source: PathBuf,
    config: PathBuf,
    chezmoi: Chezmoi,
}

impl Computer {
    fn new(base: &Path, name: &str) -> Self {
        let root = base.join(name);
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();
        let source = root.join("source");
        let config = root.join("config/chezmoi.toml");
        let chezmoi = Chezmoi::new(ChezmoiOptions {
            source: Some(source.clone()),
            destination: Some(home.clone()),
            config: Some(config.clone()),
            persistent_state: Some(root.join("state.boltdb")),
            cache: Some(root.join("cache")),
            env: vec![
                ("HOME".into(), home.clone().into()),
                ("GIT_CONFIG_GLOBAL".into(), base.join("gitconfig").into()),
                ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ],
            ..Default::default()
        })
        .expect("chezmoi must be installed to run these tests");
        Self {
            home,
            source,
            config,
            chezmoi,
        }
    }

    fn write_home(&self, relative: &str, contents: &str) {
        let path = self.home.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
}

fn base() -> (TempDir, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().canonicalize().unwrap();
    // No identity, like a computer where git was never set up.
    fs::write(base.join("gitconfig"), "").unwrap();
    (root, base)
}

fn git(dir: &Path, base: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", base.join("gitconfig"))
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

fn bare_remote(base: &Path, name: &str) -> PathBuf {
    let remote = base.join(name);
    git(
        base,
        base,
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );
    remote
}

fn url(path: &Path) -> String {
    format!("file://{}", path.display())
}

/// A remote that already holds a shared version with these files.
fn remote_with(base: &Path, name: &str, files: &[(&str, &str)]) -> PathBuf {
    let remote = bare_remote(base, name);
    let work = base.join(format!("{name}-work"));
    git(
        base,
        base,
        &[
            "clone",
            "-q",
            remote.to_str().unwrap(),
            work.to_str().unwrap(),
        ],
    );
    for (path, contents) in files {
        fs::write(work.join(path), contents).unwrap();
    }
    git(&work, base, &["add", "-A"]);
    git(&work, base, &["commit", "-q", "-m", "Initial"]);
    git(&work, base, &["push", "-q", "origin", "HEAD"]);
    remote
}

#[test]
fn first_computer_starts_and_second_fetches() {
    let (_root, base) = base();
    let first = Computer::new(&base, "first");
    first.write_home(".zshrc", "export EDITOR=vim\n");

    first.chezmoi.start_new().unwrap();
    assert!(first.source.join(".git").is_dir());
    let again = first.chezmoi.start_new().unwrap_err();
    assert_eq!(again.kind(), "invalid_request");

    // Nothing to upload yet.
    let remote = bare_remote(&base, "remote.git");
    let empty = first.chezmoi.connect_shared(&url(&remote)).unwrap_err();
    assert_eq!(empty.kind(), "invalid_request");

    first
        .chezmoi
        .add(&[".zshrc".into()], &AddOptions::default())
        .unwrap();
    first.chezmoi.connect_shared(&url(&remote)).unwrap();
    // Git has no identity here: the commit gets one for itself, and none
    // is written to the repository's config.
    let author = git(&first.source, &base, &["log", "-1", "--format=%an <%ae>"]);
    assert!(author.contains('@'), "{author}");
    let local = git(&first.source, &base, &["config", "--local", "--list"]);
    assert!(!local.contains("user."), "{local}");
    let status = first.chezmoi.git().unwrap().status().unwrap();
    assert!(status.upstream.is_some(), "{status:?}");
    assert!(status.is_clean());
    assert_eq!(status.ahead, 0);
    let pushed = git(&remote, &base, &["ls-tree", "--name-only", "HEAD"]);
    assert_eq!(pushed.trim(), "dot_zshrc");

    let second = Computer::new(&base, "second");
    second.chezmoi.start_from(&url(&remote), false).unwrap();
    assert!(!second.chezmoi.needs_config().unwrap());
    let overview = second.chezmoi.overview().unwrap();
    assert_eq!(overview.files.len(), 1);
    assert_eq!(overview.files[0].target, ".zshrc");
    assert_eq!(overview.files[0].state, FileState::UpdateAvailable);
    assert!(
        !second.home.join(".zshrc").exists(),
        "fetching changes nothing in the home directory"
    );
    let shared = overview.shared.unwrap();
    assert!(shared.upstream.is_some());
}

#[test]
fn fetching_asks_the_config_templates_questions() {
    let (_root, base) = base();
    let template = r#"{{- $email := promptStringOnce . "email" "Email address" -}}
{{- $work := promptBoolOnce . "work" "Is this a work computer" false -}}
{{- $nick := promptString "Nickname, \"short\"" -}}
{{- $host := promptString (printf "Name for %s" "this computer") -}}
[data]
  email = {{ $email | quote }}
  work = {{ $work }}
  nick = {{ $nick | quote }}
  host = {{ $host | quote }}
"#;
    let remote = remote_with(
        &base,
        "remote.git",
        &[
            (".chezmoi.toml.tmpl", template),
            ("dot_gitconfig.tmpl", "[user]\n\temail = {{ .email }}\n"),
        ],
    );
    let computer = Computer::new(&base, "new");
    computer.chezmoi.start_from(&url(&remote), false).unwrap();
    assert!(computer.chezmoi.needs_config().unwrap());
    let questions = computer.chezmoi.config_questions().unwrap();
    let prompts: Vec<(QuestionKind, &str)> = questions
        .iter()
        .map(|q| (q.kind, q.prompt.as_str()))
        .collect();
    assert_eq!(
        prompts,
        vec![
            (QuestionKind::Text, "Email address"),
            (QuestionKind::YesNo, "Is this a work computer"),
            (QuestionKind::Text, "Nickname, \"short\""),
        ],
        "the computed question cannot be seen in the template"
    );

    let email = ConfigAnswer {
        kind: QuestionKind::Text,
        prompt: "Email address".into(),
        value: "me@example.com, really".into(),
    };
    let missing = computer
        .chezmoi
        .init_config(std::slice::from_ref(&email))
        .unwrap()
        .expect("chezmoi asks the unanswered question");
    assert_eq!(missing.prompt, "Is this a work computer");
    assert!(!computer.config.exists());

    let work = ConfigAnswer {
        kind: QuestionKind::YesNo,
        prompt: "Is this a work computer".into(),
        value: "true".into(),
    };
    let nick = ConfigAnswer {
        kind: QuestionKind::Text,
        prompt: "Nickname, \"short\"".into(),
        value: "Kay \"K\"".into(),
    };
    // chezmoi asks the computed one; its text comes from what it printed.
    let computed = computer
        .chezmoi
        .init_config(&[email.clone(), work.clone(), nick.clone()])
        .unwrap()
        .expect("the computed question");
    assert_eq!(computed.prompt, "Name for this computer");
    let host = ConfigAnswer {
        kind: computed.kind,
        prompt: computed.prompt,
        value: "laptop".into(),
    };
    assert_eq!(
        computer
            .chezmoi
            .init_config(&[email, work, nick, host])
            .unwrap(),
        None
    );
    let config = fs::read_to_string(&computer.config).unwrap();
    assert!(
        config.contains("email = \"me@example.com, really\""),
        "{config}"
    );
    assert!(config.contains("work = true"), "{config}");
    assert!(config.contains(r#"nick = "Kay \"K\"""#), "{config}");
    assert!(config.contains("host = \"laptop\""), "{config}");
    assert!(!computer.chezmoi.needs_config().unwrap());
    let rendered = computer.chezmoi.cat(".gitconfig").unwrap();
    assert!(rendered.contains("me@example.com, really"), "{rendered}");
}

#[test]
fn a_template_that_checks_for_a_terminal_uses_its_defaults() {
    let (_root, base) = base();
    let template = r#"{{- $email := "nobody@example.com" -}}
{{- if stdinIsATTY -}}{{- $email = promptString "Email address" -}}{{- end -}}
{{- $host := promptString (print "Computer name") -}}
[data]
  email = {{ $email | quote }}
  host = {{ $host | quote }}
"#;
    let remote = remote_with(&base, "remote.git", &[(".chezmoi.toml.tmpl", template)]);
    let computer = Computer::new(&base, "new");
    computer.chezmoi.start_from(&url(&remote), false).unwrap();
    assert!(computer.chezmoi.config_questions().unwrap().is_empty());
    let asked = computer.chezmoi.init_config(&[]).unwrap().unwrap();
    assert_eq!(asked.prompt, "Computer name", "only the unguarded question");
    let answer = ConfigAnswer {
        kind: asked.kind,
        prompt: asked.prompt,
        value: "desk".into(),
    };
    assert_eq!(computer.chezmoi.init_config(&[answer]).unwrap(), None);
    let config = fs::read_to_string(&computer.config).unwrap();
    assert!(config.contains("nobody@example.com"), "{config}");
    assert!(config.contains("host = \"desk\""), "{config}");
}

#[test]
fn fetching_never_replaces_an_existing_config() {
    let (_root, base) = base();
    let remote = remote_with(&base, "remote.git", &[("dot_hello", "hi\n")]);
    let computer = Computer::new(&base, "moved");
    // Set up before, but the source directory is gone (an unmounted drive).
    fs::create_dir_all(computer.config.parent().unwrap()).unwrap();
    fs::write(&computer.config, "[data]\n  keep = true\n").unwrap();
    let err = computer
        .chezmoi
        .start_from(&url(&remote), false)
        .unwrap_err();
    assert_eq!(err.kind(), "invalid_request");
    assert!(!computer.source.exists());
    assert_eq!(
        fs::read_to_string(&computer.config).unwrap(),
        "[data]\n  keep = true\n"
    );
}

#[test]
fn connecting_never_replaces_a_remote() {
    let (_root, base) = base();
    let computer = Computer::new(&base, "used");
    computer.write_home(".zshrc", "x\n");
    computer.chezmoi.start_new().unwrap();
    computer
        .chezmoi
        .add(&[".zshrc".into()], &AddOptions::default())
        .unwrap();
    let real = bare_remote(&base, "real.git");
    git(
        &computer.source,
        &base,
        &["remote", "add", "origin", &url(&real)],
    );
    let other = bare_remote(&base, "other.git");
    let err = computer.chezmoi.connect_shared(&url(&other)).unwrap_err();
    assert_eq!(err.kind(), "invalid_request");
    let handle = computer.chezmoi.git().unwrap();
    assert_eq!(handle.remote_url().unwrap(), Some(url(&real)));
    assert!(!handle.status().unwrap().is_clean(), "nothing committed");
    // The same address: upload to it.
    computer.chezmoi.connect_shared(&url(&real)).unwrap();
    assert!(handle.status().unwrap().upstream.is_some());
}

#[test]
fn skipping_the_questions_writes_an_empty_config() {
    let (_root, base) = base();
    let remote = remote_with(
        &base,
        "remote.git",
        &[(".chezmoi.toml.tmpl", "{{ promptString \"Name\" }}\n")],
    );
    let computer = Computer::new(&base, "new");
    computer.chezmoi.start_from(&url(&remote), false).unwrap();
    assert!(computer.chezmoi.needs_config().unwrap());
    computer.chezmoi.skip_config().unwrap();
    assert!(!computer.chezmoi.needs_config().unwrap());
    assert_eq!(fs::read_to_string(&computer.config).unwrap(), "");
}

#[test]
fn a_failed_fetch_leaves_nothing_behind() {
    let (_root, base) = base();
    let computer = Computer::new(&base, "new");
    let err = computer
        .chezmoi
        .start_from(&url(&base.join("missing.git")), false)
        .unwrap_err();
    assert_eq!(remote_problem(&err), Some(RemoteProblem::NotFound), "{err}");
    assert!(!computer.source.exists());
    computer.chezmoi.start_new().unwrap();
}

#[test]
fn connecting_to_a_repository_with_history_is_refused() {
    let (_root, base) = base();
    let remote = remote_with(&base, "used.git", &[("dot_other", "x\n")]);
    let computer = Computer::new(&base, "new");
    computer.write_home(".zshrc", "x\n");
    computer.chezmoi.start_new().unwrap();
    computer
        .chezmoi
        .add(&[".zshrc".into()], &AddOptions::default())
        .unwrap();
    let err = computer.chezmoi.connect_shared(&url(&remote)).unwrap_err();
    assert_eq!(remote_problem(&err), Some(RemoteProblem::NotEmpty), "{err}");
    let handle = computer.chezmoi.git().unwrap();
    assert_eq!(handle.remote_url().unwrap(), None, "no origin left behind");
    // The commit stays; connecting to an empty remote afterwards works.
    let empty = bare_remote(&base, "empty.git");
    computer.chezmoi.connect_shared(&url(&empty)).unwrap();
    let status = computer.chezmoi.git().unwrap().status().unwrap();
    assert!(status.upstream.is_some());
}

#[test]
fn connecting_a_folder_that_is_not_a_repository() {
    let (_root, base) = base();
    let computer = Computer::new(&base, "new");
    fs::create_dir_all(&computer.source).unwrap();
    fs::write(computer.source.join("dot_hello"), "hello\n").unwrap();
    let remote = bare_remote(&base, "remote.git");
    computer.chezmoi.connect_shared(&url(&remote)).unwrap();
    let pushed = git(&remote, &base, &["ls-tree", "--name-only", "HEAD"]);
    assert_eq!(pushed.trim(), "dot_hello");
}

#[test]
fn suggests_common_dotfiles() {
    let (_root, base) = base();
    let computer = Computer::new(&base, "new");
    computer.write_home(".zshrc", "x\n");
    computer.write_home(".gitconfig", "[user]\n");
    computer.write_home(".config/nvim/init.lua", "vim.o.number = true\n");
    computer.write_home(".config/nvim/lua/keys.lua", "-- keys\n");
    computer.write_home(".ssh/config", "Host *\n");
    computer.write_home(".zprofile", "");
    computer.write_home(".bashrc", &"x".repeat(2 << 20));
    computer.write_home(".tmux.conf", "set -g mouse on\n");
    std::os::unix::fs::symlink(computer.home.join(".zshrc"), computer.home.join(".vimrc")).unwrap();

    computer.chezmoi.start_new().unwrap();
    computer
        .chezmoi
        .add(&[".tmux.conf".into()], &AddOptions::default())
        .unwrap();
    let managed = computer.chezmoi.managed().unwrap();
    let suggestions = computer.chezmoi.suggestions(&managed).unwrap();
    let found: Vec<(&str, bool, usize)> = suggestions
        .iter()
        .map(|s| (s.target.as_str(), s.directory, s.files))
        .collect();
    assert_eq!(
        found,
        vec![
            (".zshrc", false, 1),
            (".gitconfig", false, 1),
            (".config/nvim", true, 2),
            (".ssh/config", false, 1),
        ],
        "empty, large, symlinked and managed entries are left out"
    );

    // A folder that is a git checkout of its own is left out too.
    computer.write_home(".config/nvim/.git/HEAD", "ref: refs/heads/main\n");
    let suggestions = computer.chezmoi.suggestions(&managed).unwrap();
    assert!(suggestions.iter().all(|s| s.target != ".config/nvim"));
}

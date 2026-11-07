//! Getting a computer started: a new shared version, or one fetched from
//! another computer; the questions its config template asks; dotfiles worth
//! suggesting; and connecting the shared version to a remote.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::chezmoi::Chezmoi;
use crate::error::{Error, Result};
use crate::managed::ManagedEntry;

/// `chezmoi init` itself is quick; cloning depends on the network.
const INIT_TIMEOUT: Duration = Duration::from_secs(60);
const CLONE_TIMEOUT: Duration = Duration::from_secs(180);
/// Larger files are rarely configuration; leave them out of suggestions.
const MAX_SUGGESTED_BYTES: u64 = 1 << 20;
/// Folders with more files than this are probably not just settings.
const MAX_SUGGESTED_FILES: usize = 200;

/// What a question in a config template asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    /// `promptString`
    Text,
    /// `promptBool`
    YesNo,
    /// `promptInt`
    Number,
    /// `promptChoice`
    Choice,
    /// `promptMultichoice`
    MultipleChoice,
}

impl QuestionKind {
    fn function(self) -> &'static str {
        match self {
            QuestionKind::Text => "promptString",
            QuestionKind::YesNo => "promptBool",
            QuestionKind::Number => "promptInt",
            QuestionKind::Choice => "promptChoice",
            QuestionKind::MultipleChoice => "promptMultichoice",
        }
    }

    /// The `chezmoi init` flag that answers it.
    fn flag(self) -> String {
        format!("--{}", self.function())
    }

    /// The kind a template function such as `promptStringOnce` asks.
    fn of_function(name: &str) -> Option<Self> {
        let base = name.strip_suffix("Once").unwrap_or(name);
        [
            QuestionKind::Text,
            QuestionKind::YesNo,
            QuestionKind::Number,
            QuestionKind::Choice,
            QuestionKind::MultipleChoice,
        ]
        .into_iter()
        .find(|k| k.function() == base)
    }
}

/// A question the config template asks when `chezmoi init` makes the
/// config file, such as `{{ promptStringOnce . "email" "Email address" }}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigQuestion {
    pub kind: QuestionKind,
    /// The text chezmoi shows. Answers are matched to questions by it.
    pub prompt: String,
    pub default: Option<String>,
    /// The options of a choice, when the template lists them literally.
    pub choices: Vec<String>,
}

/// An answer to a [`ConfigQuestion`]. Yes/no answers are `true` or
/// `false`; several choices are joined with `/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigAnswer {
    pub kind: QuestionKind,
    pub prompt: String,
    pub value: String,
}

/// The questions in a config template, in order, each once. Questions
/// whose text is computed by the template cannot be found this way;
/// [`Chezmoi::init_config`] reports those when chezmoi asks them.
pub fn parse_questions(template: &str) -> Vec<ConfigQuestion> {
    let mut found: Vec<ConfigQuestion> = Vec::new();
    let mut rest = template;
    while let Some(at) = rest.find("prompt") {
        let after = &rest[at..];
        rest = &after["prompt".len()..];
        // A function name, not part of a longer word.
        if template[..template.len() - after.len()]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.')
        {
            continue;
        }
        let Some((question, used)) = parse_call(after) else {
            continue;
        };
        rest = &after[used..];
        if !found
            .iter()
            .any(|q| q.kind == question.kind && q.prompt == question.prompt)
        {
            found.push(question);
        }
    }
    found
}

/// Parses one `prompt…` call at the start of `text`. Returns the question
/// and how many bytes it used.
fn parse_call(text: &str) -> Option<(ConfigQuestion, usize)> {
    let name_end = text
        .find(|c: char| !c.is_ascii_alphanumeric())
        .unwrap_or(text.len());
    let name = &text[..name_end];
    let once = name.ends_with("Once");
    let kind = QuestionKind::of_function(name)?;
    let mut tokens = Tokens { text, at: name_end };
    if once {
        // The map to look in (usually `.`) and the key the answer is kept
        // under.
        tokens.next()?;
        tokens.next()?;
    }
    let Some(Token::Text(prompt)) = tokens.next() else {
        return None;
    };
    let mut choices = Vec::new();
    if matches!(kind, QuestionKind::Choice | QuestionKind::MultipleChoice)
        && let Some(Token::List(items)) = tokens.next()
    {
        choices = items;
    }
    let default = match tokens.next() {
        Some(Token::Text(value)) => Some(value),
        Some(Token::Word(word))
            if word == "true" || word == "false" || word.parse::<i64>().is_ok() =>
        {
            Some(word)
        }
        _ => None,
    };
    let question = ConfigQuestion {
        kind,
        prompt,
        default,
        choices,
    };
    Some((question, tokens.at))
}

enum Token {
    /// A string literal.
    Text(String),
    /// `(list "a" "b")`
    List(Vec<String>),
    /// `.`, `$var`, `true`, `3` and the like.
    Word(String),
    /// Any other parenthesized expression.
    Other,
}

/// Template arguments, one at a time, until the action ends.
struct Tokens<'a> {
    text: &'a str,
    at: usize,
}

impl Tokens<'_> {
    fn next(&mut self) -> Option<Token> {
        let rest = &self.text[self.at..];
        let trimmed = rest.trim_start();
        self.at += rest.len() - trimmed.len();
        let first = trimmed.chars().next()?;
        // The action ends: `}}`, `-}}`, a pipe or a closing parenthesis.
        if trimmed.starts_with("-}}") || matches!(first, ')' | '}' | '|') {
            return None;
        }
        match first {
            '"' => {
                let (value, used) = go_string(trimmed)?;
                self.at += used;
                Some(Token::Text(value))
            }
            '`' => {
                let end = trimmed[1..].find('`')? + 1;
                self.at += end + 1;
                Some(Token::Text(trimmed[1..end].to_string()))
            }
            '(' => {
                let end = matching_paren(trimmed)?;
                let inner = &trimmed[1..end];
                self.at += end + 1;
                let Some(items) = inner.trim_start().strip_prefix("list") else {
                    return Some(Token::Other);
                };
                let mut list = Tokens { text: items, at: 0 };
                let mut values = Vec::new();
                while let Some(token) = list.next() {
                    match token {
                        Token::Text(value) => values.push(value),
                        _ => return Some(Token::Other),
                    }
                }
                Some(Token::List(values))
            }
            _ => {
                let end = trimmed
                    .find(|c: char| c.is_whitespace() || matches!(c, ')' | '}' | '|'))
                    .unwrap_or(trimmed.len());
                self.at += end;
                Some(Token::Word(trimmed[..end].to_string()))
            }
        }
    }
}

/// A double-quoted Go string at the start of `text`: its value and length.
fn go_string(text: &str) -> Option<(String, usize)> {
    let mut value = String::new();
    let mut chars = text.char_indices().skip(1);
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((value, i + 1)),
            '\\' => {
                let (_, escaped) = chars.next()?;
                value.push(match escaped {
                    'n' => '\n',
                    't' => '\t',
                    other => other,
                });
            }
            '\n' => return None,
            c => value.push(c),
        }
    }
    None
}

/// The index of the `)` closing the `(` at the start of `text`.
fn matching_paren(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in text.char_indices() {
        if in_string {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// The question chezmoi asked and got no answer to. chezmoi prints the
/// question on standard output (`Email address? `, `Shell (zsh/fish,
/// default zsh)? `) and fails with `… at <promptStringOnce . "email"
/// "Email address">: error calling promptStringOnce: EOF`. The call as
/// written is used when it can be read; otherwise, when the template
/// computes the text, the question as printed.
pub fn unanswered_question(stderr: &str, stdout: &str) -> Option<ConfigQuestion> {
    const CALLING: &str = ": error calling ";
    let calling = stderr.find(&format!("{CALLING}prompt"))?;
    let line = stderr[calling + CALLING.len()..].lines().next()?.trim_end();
    let (function, reason) = line.split_once(": ")?;
    if reason != "EOF" {
        return None;
    }
    let kind = QuestionKind::of_function(function)?;
    let call = stderr[..calling]
        .rfind(" at <")
        .and_then(|start| stderr[start + " at <".len()..calling].strip_suffix('>'));
    if let Some((question, _)) = call.and_then(parse_call) {
        return Some(question);
    }
    printed_question(kind, stdout)
}

/// A question from chezmoi's prompt: `Text? `, `Text (default x)? `,
/// `Text (a/b, default a)? `; several choices print `Text?` and a
/// `Choices (a/b)` line.
fn printed_question(kind: QuestionKind, stdout: &str) -> Option<ConfigQuestion> {
    let mut lines = stdout.lines();
    let first = lines.next()?.trim_end();
    let mut prompt = first.strip_suffix('?').unwrap_or(first).to_string();
    let mut default = None;
    let mut choices = Vec::new();
    if let Some(open) = prompt.rfind(" (")
        && prompt.ends_with(')')
    {
        let inner = prompt[open + 2..prompt.len() - 1].to_string();
        let (listed, given) = match inner.strip_prefix("default ") {
            Some(given) => ("", Some(given)),
            None => match inner.split_once(", default ") {
                Some((listed, given)) => (listed, Some(given)),
                None => (inner.as_str(), None),
            },
        };
        let is_choice = kind == QuestionKind::Choice && !listed.is_empty();
        if given.is_some() || is_choice {
            if is_choice {
                choices = listed.split('/').map(str::to_string).collect();
            }
            default = given.map(|g| g.trim_matches('"').to_string());
            prompt.truncate(open);
        }
    }
    if kind == QuestionKind::MultipleChoice
        && let Some(listed) = lines.find_map(|l| l.trim().strip_prefix("Choices ("))
    {
        choices = listed
            .trim_end_matches(')')
            .split('/')
            .map(str::to_string)
            .collect();
    }
    (!prompt.trim().is_empty()).then_some(ConfigQuestion {
        kind,
        prompt,
        default,
        choices,
    })
}

/// Answers a key that no question uses, so a flag value always has more
/// than one `=`: with exactly one, chezmoi's flag parser only trims quotes
/// instead of reading the value as CSV.
const PADDING: &str = "__chezmate=1";

/// The `chezmoi init` arguments for these answers: one flag per kind,
/// whose value is a CSV record of `prompt=value` pairs. Numbers use a
/// parser without quoting, so their questions cannot contain `,` or `=`.
fn answer_args(answers: &[ConfigAnswer]) -> Result<Vec<OsString>> {
    let mut args = Vec::new();
    let mut kinds: Vec<QuestionKind> = answers.iter().map(|a| a.kind).collect();
    let mut seen = HashSet::new();
    kinds.retain(|k| seen.insert(*k));
    for kind in kinds {
        let pairs = answers.iter().filter(|a| a.kind == kind);
        let value = if kind == QuestionKind::Number {
            pairs
                .map(|a| {
                    if a.prompt.contains([',', '=']) || a.value.parse::<i64>().is_err() {
                        return Err(Error::InvalidRequest(format!(
                            "cannot answer the number question {:?} with {:?}",
                            a.prompt, a.value
                        )));
                    }
                    Ok(format!("{}={}", a.prompt, a.value))
                })
                .collect::<Result<Vec<_>>>()?
                .join(",")
        } else {
            let mut record: Vec<String> = pairs
                .map(|a| csv_field(&format!("{}={}", a.prompt, a.value)))
                .collect();
            if record.iter().map(|f| f.matches('=').count()).sum::<usize>() < 2 {
                record.push(PADDING.to_string());
            }
            record.join(",")
        };
        args.push(kind.flag().into());
        args.push(value.into());
    }
    Ok(args)
}

fn csv_field(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) || field.starts_with(' ') || field.ends_with(' ') {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

/// Why fetching from or uploading to a remote failed, in terms a person
/// can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteProblem {
    /// No such repository, or it is private and this computer has no access.
    NotFound,
    /// The server refused the credentials (an SSH key, for example).
    AccessDenied,
    /// The server could not be reached.
    Offline,
    /// The remote already has commits this computer does not have.
    NotEmpty,
}

/// Recognizes the common ways cloning and pushing fail.
pub fn remote_problem(err: &Error) -> Option<RemoteProblem> {
    let text = err.stderr().unwrap_or_default();
    let has = |needles: &[&str]| needles.iter().any(|n| text.contains(n));
    // GitHub over HTTPS: "fatal: repository 'https://…' not found".
    let missing_repository = text
        .lines()
        .any(|l| l.contains("fatal: repository '") && l.trim_end().ends_with("' not found"));
    if has(&["[rejected]", "fetch first", "non-fast-forward"]) {
        Some(RemoteProblem::NotEmpty)
    } else if has(&[
        "Could not resolve host",
        "Network is unreachable",
        "Connection timed out",
        "Operation timed out",
        "Connection refused",
    ]) {
        Some(RemoteProblem::Offline)
    } else if has(&[
        "Permission denied (publickey",
        "remote: Permission to ",
        "Authentication failed",
        "Host key verification failed",
    ]) {
        Some(RemoteProblem::AccessDenied)
    } else if missing_repository
        || has(&[
            "could not read Username",
            "Repository not found",
            "does not appear to be a git repository",
        ])
    {
        Some(RemoteProblem::NotFound)
    } else {
        None
    }
}

/// What a suggested file is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    Shell,
    Git,
    Editor,
    Terminal,
    Ssh,
}

/// A common dotfile found on this computer that is not synced yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Suggestion {
    /// Relative to the home directory.
    pub target: String,
    pub topic: Topic,
    pub directory: bool,
    /// Files inside a folder; 1 for a file.
    pub files: usize,
    pub bytes: u64,
}

/// Well-known settings files and folders, in the order they are shown.
/// Files that usually hold secrets (`.npmrc`, `.netrc`, keys) are left out.
const CANDIDATES: &[(&str, Topic)] = &[
    (".zshrc", Topic::Shell),
    (".zprofile", Topic::Shell),
    (".zshenv", Topic::Shell),
    (".bashrc", Topic::Shell),
    (".bash_profile", Topic::Shell),
    (".profile", Topic::Shell),
    (".inputrc", Topic::Shell),
    (".config/fish/config.fish", Topic::Shell),
    (".config/starship.toml", Topic::Shell),
    (".gitconfig", Topic::Git),
    (".gitignore_global", Topic::Git),
    (".config/git/config", Topic::Git),
    (".config/git/ignore", Topic::Git),
    (".vimrc", Topic::Editor),
    (".ideavimrc", Topic::Editor),
    (".editorconfig", Topic::Editor),
    (".config/nvim", Topic::Editor),
    (".config/helix/config.toml", Topic::Editor),
    (".config/zed/settings.json", Topic::Editor),
    (".config/zed/keymap.json", Topic::Editor),
    (".emacs", Topic::Editor),
    (".tmux.conf", Topic::Terminal),
    (".config/tmux/tmux.conf", Topic::Terminal),
    (".config/ghostty/config", Topic::Terminal),
    (".config/kitty/kitty.conf", Topic::Terminal),
    (".config/alacritty/alacritty.toml", Topic::Terminal),
    (".wezterm.lua", Topic::Terminal),
    (".config/wezterm/wezterm.lua", Topic::Terminal),
    (".aerospace.toml", Topic::Terminal),
    (".ssh/config", Topic::Ssh),
];

/// Common dotfiles in `home` that are not managed yet. Symbolic links,
/// large files, folders with many files and git checkouts are left out.
pub fn suggestions_in(home: &Path, managed: &[ManagedEntry]) -> Vec<Suggestion> {
    let managed: HashSet<&str> = managed.iter().map(|e| e.target.as_str()).collect();
    let overlaps = |target: &str| {
        managed.iter().any(|m| {
            *m == target
                || target.starts_with(&format!("{m}/"))
                || m.starts_with(&format!("{target}/"))
        })
    };
    CANDIDATES
        .iter()
        .filter(|(target, _)| !overlaps(target))
        .filter_map(|&(target, topic)| {
            let path = home.join(target);
            let meta = fs::symlink_metadata(&path).ok()?;
            let (directory, files, bytes) = if meta.is_file() {
                (false, 1, meta.len())
            } else if meta.is_dir() {
                let (files, bytes) = folder_size(&path, MAX_SUGGESTED_FILES)?;
                (true, files, bytes)
            } else {
                return None;
            };
            let limit = if directory {
                MAX_SUGGESTED_BYTES * 4
            } else {
                MAX_SUGGESTED_BYTES
            };
            (files > 0 && bytes > 0 && bytes <= limit).then(|| Suggestion {
                target: target.to_string(),
                topic,
                directory,
                files,
                bytes,
            })
        })
        .collect()
}

/// Files and bytes in a folder; `None` past `limit` files or for a git
/// checkout.
fn folder_size(path: &Path, limit: usize) -> Option<(usize, u64)> {
    let mut stack = vec![path.to_path_buf()];
    let (mut files, mut bytes) = (0usize, 0u64);
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).ok()? {
            let entry = entry.ok()?;
            if entry.file_name() == ".git" {
                return None;
            }
            let meta = entry.path().symlink_metadata().ok()?;
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                files += 1;
                bytes += meta.len();
                if files > limit {
                    return None;
                }
            }
        }
    }
    Some((files, bytes))
}

fn has_entries(dir: &Path) -> bool {
    fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some())
}

/// A repository given as `user` or `user/repo` rather than a URL; chezmoi
/// turns it into a GitHub URL.
fn is_shorthand(repo: &str) -> bool {
    !repo.contains("://") && !repo.contains('@') && !repo.starts_with('/')
}

impl Chezmoi {
    /// Starts a new, empty shared version on this computer: the source
    /// directory as a git repository. Fails if it already has files.
    pub fn start_new(&self) -> Result<()> {
        let paths = self.paths()?;
        if has_entries(&paths.source_dir) {
            return Err(Error::InvalidRequest(format!(
                "{} already exists",
                paths.source_dir.display()
            )));
        }
        let args: Vec<OsString> = vec!["init".into()];
        let display = self.display(&args);
        Self::check(&display, self.exec(args, Some(INIT_TIMEOUT))?).map(drop)
    }

    /// Fetches an existing shared version (`chezmoi init <repo>`) without
    /// changing anything in the home directory. `repo` is a URL, or `user`
    /// or `user/repo` on GitHub, tried over SSH first when `prefer_ssh`,
    /// then the other way.
    ///
    /// Succeeds when the clone worked even if the config template has
    /// questions left: [`Chezmoi::config_questions`] lists them. A failed
    /// clone leaves no source directory behind. Refuses when a config file
    /// exists: the fetched template would replace it.
    pub fn start_from(&self, repo: &str, prefer_ssh: bool) -> Result<()> {
        let repo = repo.trim();
        if repo.is_empty() || repo.starts_with('-') || repo.contains(char::is_whitespace) {
            return Err(Error::InvalidRequest(format!(
                "{repo:?} is not a repository address"
            )));
        }
        let paths = self.paths()?;
        if has_entries(&paths.source_dir) {
            return Err(Error::InvalidRequest(format!(
                "{} already exists",
                paths.source_dir.display()
            )));
        }
        if paths.config_file.exists() {
            return Err(Error::InvalidRequest(format!(
                "chezmoi's config file {} already exists",
                paths.config_file.display()
            )));
        }
        let attempts: &[bool] = if !is_shorthand(repo) {
            &[false]
        } else if prefer_ssh {
            &[true, false]
        } else {
            &[false, true]
        };
        let mut first_error = None;
        for &ssh in attempts {
            match self.clone_once(repo, ssh) {
                Ok(()) => return Ok(()),
                Err(err) => {
                    // Only a clone that failed for "not found" or "denied"
                    // can depend on the protocol.
                    let cloned = paths.working_tree.join(".git").exists();
                    let retry = !cloned
                        && matches!(
                            remote_problem(&err),
                            Some(RemoteProblem::NotFound | RemoteProblem::AccessDenied)
                        );
                    first_error.get_or_insert(err);
                    if !retry {
                        break;
                    }
                }
            }
        }
        Err(first_error.expect("at least one attempt"))
    }

    fn clone_once(&self, repo: &str, ssh: bool) -> Result<()> {
        let paths = self.paths()?;
        let mut args: Vec<OsString> = vec!["init".into()];
        if ssh {
            args.push("--ssh".into());
        }
        args.push(repo.into());
        let display = self.display(&args);
        let captured = self.exec(args, Some(CLONE_TIMEOUT))?;
        let cloned = paths.working_tree.join(".git").exists();
        if !captured.success()
            && cloned
            && unanswered_question(&captured.stderr, &captured.stdout).is_some()
        {
            return Ok(());
        }
        let result = Self::check(&display, captured).map(drop);
        if result.is_err() && !cloned && !has_entries(&paths.source_dir) {
            let _ = fs::remove_dir(&paths.source_dir);
        }
        result
    }

    /// The config file still has to be made from the source directory's
    /// template: there is a template but no config file.
    pub fn needs_config(&self) -> Result<bool> {
        let info = self.info()?;
        Ok(info.source_exists && info.config_template.is_some() && !info.config_exists)
    }

    /// The questions in the source directory's config template. A
    /// template that checks `stdinIsATTY` asks only on a terminal and uses
    /// its defaults here, so none are listed for it; chezmoi still reports
    /// any it does ask through [`Chezmoi::init_config`].
    pub fn config_questions(&self) -> Result<Vec<ConfigQuestion>> {
        let Some(template) = self.info()?.config_template else {
            return Ok(Vec::new());
        };
        let text = fs::read_to_string(&template).map_err(|source| Error::Io {
            context: format!("reading {}", template.display()),
            source,
        })?;
        if text.contains("stdinIsATTY") {
            return Ok(Vec::new());
        }
        Ok(parse_questions(&text))
    }

    /// Makes the config file from the template (`chezmoi init` without a
    /// repository) with these answers. Returns the question chezmoi asked
    /// that had no answer, if any; nothing is written then.
    pub fn init_config(&self, answers: &[ConfigAnswer]) -> Result<Option<ConfigQuestion>> {
        let mut args: Vec<OsString> = vec!["init".into()];
        args.extend(answer_args(answers)?);
        let display = self.display(&args);
        let captured = self.exec(args, Some(INIT_TIMEOUT))?;
        if !captured.success()
            && let Some(question) = unanswered_question(&captured.stderr, &captured.stdout)
        {
            return Ok(Some(question));
        }
        Self::check(&display, captured).map(|_| None)
    }

    /// Writes an empty config file instead of making one from the
    /// template, for when the template cannot be completed here. It can be
    /// edited later.
    pub fn skip_config(&self) -> Result<()> {
        let path = self.paths()?.config_file;
        let io = |source| Error::Io {
            context: format!("writing {}", path.display()),
            source,
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io)?;
        }
        if !path.exists() {
            fs::write(&path, "").map_err(io)?;
        }
        Ok(())
    }

    /// Common dotfiles in the home directory that are not managed yet.
    pub fn suggestions(&self, managed: &[ManagedEntry]) -> Result<Vec<Suggestion>> {
        Ok(suggestions_in(&self.paths()?.dest_dir, managed))
    }

    /// Connects the shared version to an empty remote repository and
    /// uploads it: commits what is there, sets `origin` and pushes. The
    /// source directory becomes a git repository first if it is not one.
    /// An `origin` pointing elsewhere is never replaced, and a failed
    /// upload leaves no new `origin` behind.
    pub fn connect_shared(&self, url: &str) -> Result<()> {
        let url = url.trim();
        if url.is_empty() || url.starts_with('-') || url.contains(char::is_whitespace) {
            return Err(Error::InvalidRequest(format!(
                "{url:?} is not a repository address"
            )));
        }
        let paths = self.paths()?;
        if !paths.source_dir.is_dir() {
            return Err(Error::InvalidRequest(
                "there is no shared version to upload yet".into(),
            ));
        }
        let git = self.git()?;
        if !paths.working_tree.join(".git").exists() {
            git.init()?;
        }
        let existing = git.remote_url()?;
        if let Some(existing) = existing.as_deref().filter(|e| *e != url) {
            return Err(Error::InvalidRequest(format!(
                "the shared version already has a remote: {existing}"
            )));
        }
        if !git.status()?.is_clean() {
            git.commit_all("Add dotfiles")?;
        }
        if git.status()?.head.is_none() {
            return Err(Error::InvalidRequest(
                "the shared version is empty: add some files first".into(),
            ));
        }
        if existing.is_none() {
            git.add_remote(url)?;
        }
        git.push_upstream().inspect_err(|_| {
            if existing.is_none() {
                let _ = git.remove_remote();
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question(kind: QuestionKind, prompt: &str, default: Option<&str>) -> ConfigQuestion {
        ConfigQuestion {
            kind,
            prompt: prompt.into(),
            default: default.map(str::to_string),
            choices: Vec::new(),
        }
    }

    #[test]
    fn finds_questions_in_a_config_template() {
        let template = r#"
{{- $email := promptStringOnce . "email" "Email address" -}}
{{- $work := promptBoolOnce . "work" "Is this a work computer" false -}}
{{- $name := promptString "Your name" "Anon" | trim -}}
{{- $shell := promptChoiceOnce . "shell" "Shell" (list "zsh" "fish") "zsh" -}}
{{- $tools := promptMultichoice "Tools" (list "vim" "emacs") -}}
{{- $n := promptInt "Monitors" 2 -}}
{{- if stdinIsATTY }}{{ $email = promptString "Email address" }}{{ end -}}
{{- $raw := promptString `Raw "quoted"` -}}
{{- $dynamic := promptString (printf "Name for %s" .chezmoi.hostname) -}}
[data]
  email = {{ $email | quote }}
"#;
        let mut shell = question(QuestionKind::Choice, "Shell", Some("zsh"));
        shell.choices = vec!["zsh".into(), "fish".into()];
        let mut tools = question(QuestionKind::MultipleChoice, "Tools", None);
        tools.choices = vec!["vim".into(), "emacs".into()];
        assert_eq!(
            parse_questions(template),
            vec![
                question(QuestionKind::Text, "Email address", None),
                question(
                    QuestionKind::YesNo,
                    "Is this a work computer",
                    Some("false")
                ),
                question(QuestionKind::Text, "Your name", Some("Anon")),
                shell,
                tools,
                question(QuestionKind::Number, "Monitors", Some("2")),
                question(QuestionKind::Text, "Raw \"quoted\"", None),
            ]
        );
        assert!(parse_questions("{{ .promptString }} myprompt").is_empty());
    }

    #[test]
    fn reads_the_question_chezmoi_could_not_ask() {
        let stderr = "chezmoi: template: chezmoi.toml:1:14: executing \"chezmoi.toml\" at <promptStringOnce . \"email\" \"Email address\">: error calling promptStringOnce: EOF\n";
        assert_eq!(
            unanswered_question(stderr, "Email address? "),
            Some(question(QuestionKind::Text, "Email address", None))
        );
        let other = "chezmoi: template: x:1:2: executing \"x\" at <promptString \"A\">: error calling promptString: boom\n";
        assert_eq!(unanswered_question(other, "A? "), None);

        // Text the template computes: as chezmoi printed it.
        let computed = |call: &str, function: &str| {
            format!(
                "chezmoi: template: c:1:9: executing \"c\" at <{call}>: error calling {function}: EOF\n"
            )
        };
        assert_eq!(
            unanswered_question(
                &computed(
                    "promptString (printf \"Name for %s\" \"host\")",
                    "promptString"
                ),
                "Name for host? "
            ),
            Some(question(QuestionKind::Text, "Name for host", None))
        );
        assert_eq!(
            unanswered_question(
                &computed("promptBool (print \"Work\")", "promptBool"),
                "Work (default true)? "
            ),
            Some(question(QuestionKind::YesNo, "Work", Some("true")))
        );
        let mut shell = question(QuestionKind::Choice, "Shell", Some("fish"));
        shell.choices = vec!["zsh".into(), "fish".into()];
        assert_eq!(
            unanswered_question(
                &computed("promptChoice (print \"Shell\") $c", "promptChoice"),
                "Shell (zsh/fish, default fish)? "
            ),
            Some(shell)
        );
        let mut tools = question(QuestionKind::MultipleChoice, "Tools", None);
        tools.choices = vec!["a".into(), "b".into()];
        assert_eq!(
            unanswered_question(
                &computed(
                    "promptMultichoice (print \"Tools\") $l",
                    "promptMultichoice"
                ),
                "Tools?\nChoices (a/b)\nChoice (ENTER to stop)> "
            ),
            Some(tools)
        );
        assert_eq!(
            unanswered_question(
                &computed("promptString (print \"Name\") \"Anon\"", "promptString"),
                "Name (default \"Anon\")? "
            ),
            Some(question(QuestionKind::Text, "Name", Some("Anon")))
        );
    }

    #[test]
    fn answers_are_always_read_as_csv() {
        let answer = |kind, prompt: &str, value: &str| ConfigAnswer {
            kind,
            prompt: prompt.into(),
            value: value.into(),
        };
        let args = answer_args(&[
            answer(QuestionKind::Text, "Email", "a@b.c"),
            answer(QuestionKind::YesNo, "Work", "true"),
            answer(QuestionKind::Text, "Name, full", "Doe, \"J\""),
            answer(QuestionKind::Number, "Monitors", "2"),
        ])
        .unwrap();
        let args: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "--promptString",
                "Email=a@b.c,\"Name, full=Doe, \"\"J\"\"\"",
                "--promptBool",
                "Work=true,__chezmate=1",
                "--promptInt",
                "Monitors=2",
            ]
        );
        assert!(answer_args(&[answer(QuestionKind::Number, "A, b", "1")]).is_err());
        assert!(answer_args(&[answer(QuestionKind::Number, "A", "x")]).is_err());
    }

    #[test]
    fn recognizes_remote_problems() {
        let failed = |stderr: &str| Error::Failed {
            command: "git".into(),
            code: Some(128),
            stderr: stderr.into(),
        };
        assert_eq!(
            remote_problem(&failed(
                "fatal: could not read Username for 'https://github.com': terminal prompts disabled"
            )),
            Some(RemoteProblem::NotFound)
        );
        assert_eq!(
            remote_problem(&failed("git@github.com: Permission denied (publickey).")),
            Some(RemoteProblem::AccessDenied)
        );
        assert_eq!(
            remote_problem(&failed(
                "ssh: Could not resolve hostname github.com: nodename nor servname provided"
            )),
            Some(RemoteProblem::Offline)
        );
        assert_eq!(
            remote_problem(&failed(" ! [rejected]        HEAD -> main (fetch first)")),
            Some(RemoteProblem::NotEmpty)
        );
        assert_eq!(
            remote_problem(&failed(
                "remote: Repository not found.\nfatal: repository 'https://github.com/a/b.git/' not found"
            )),
            Some(RemoteProblem::NotFound)
        );
        assert_eq!(remote_problem(&failed("something else")), None);
        // Not about the remote at all.
        assert_eq!(
            remote_problem(&failed(
                "error calling output: exec: \"op\": executable file not found in $PATH"
            )),
            None
        );
        assert_eq!(
            remote_problem(&failed("open /x/y: permission denied")),
            None
        );
    }

    #[test]
    fn shorthand_is_a_github_name() {
        assert!(is_shorthand("kadaliao"));
        assert!(is_shorthand("kadaliao/dotfiles"));
        assert!(!is_shorthand("https://github.com/a/b.git"));
        assert!(!is_shorthand("git@github.com:a/b.git"));
        assert!(!is_shorthand("/srv/dotfiles.git"));
    }
}

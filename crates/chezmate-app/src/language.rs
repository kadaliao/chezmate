//! Which syntax a dotfile is written in, for highlighting. The names are
//! the tokens GPUI Kit's syntect highlighter looks up (a file extension or
//! a syntax name from bat's collection).

/// The highlighting language for `target` (a path relative to the home
/// directory). `first_line` is the file's first line, for scripts that
/// name their interpreter with `#!`; pass "" when it is not known.
pub fn language_for(target: &str, first_line: &str) -> String {
    let (dir, name) = target.rsplit_once('/').unwrap_or(("", target));
    let name = name.to_lowercase();
    let parent = dir.rsplit('/').next().unwrap_or("");
    let known = match name.as_str() {
        "config" if parent == ".ssh" => Some("ssh_config"),
        "config" if parent == "git" => Some("gitconfig"),
        "ignore" if parent == "git" => Some("gitignore"),
        ".zshrc" | ".zshenv" | ".zprofile" | ".zlogin" | ".zlogout" | ".bashrc"
        | ".bash_profile" | ".bash_login" | ".bash_logout" | ".bash_aliases" | ".profile"
        | ".tmux.conf" | "tmux.conf" => Some("bash"),
        ".vimrc" | ".gvimrc" | ".exrc" => Some("vim"),
        ".gitignore" | ".gitignore_global" => Some("gitignore"),
        ".editorconfig" => Some("ini"),
        "brewfile" | "gemfile" | ".irbrc" => Some("ruby"),
        name if name.starts_with(".gitconfig") => Some("gitconfig"),
        _ => None,
    };
    if let Some(language) = known {
        return language.to_string();
    }
    let stem = name.strip_prefix('.').unwrap_or(&name);
    if let Some((_, extension)) = stem.rsplit_once('.') {
        return match extension {
            "conf" | "cfg" => "ini",
            "plist" | "svg" => "xml",
            "sh" | "zsh" | "bash" => "bash",
            "yml" => "yaml",
            other => other,
        }
        .to_string();
    }
    if let Some(language) = shebang(first_line) {
        return language.to_string();
    }
    name
}

/// The language a `#!` line names, if it is one we know.
fn shebang(line: &str) -> Option<&'static str> {
    let command = line.strip_prefix("#!")?;
    let mut words = command.split_whitespace();
    let mut program = words.next()?.rsplit('/').next()?;
    if program == "env" {
        program = words.find(|word| !word.starts_with('-'))?;
    }
    Some(match program {
        "sh" | "bash" | "zsh" | "dash" | "ksh" => "bash",
        "fish" => "fish",
        "node" => "javascript",
        "ruby" => "ruby",
        "perl" => "perl",
        "osascript" => "applescript",
        p if p.starts_with("python") => "python",
        _ => return None,
    })
}

/// A readable name for a language from [`language_for`], if it has one.
pub fn language_label(language: &str) -> Option<&'static str> {
    Some(match language {
        "bash" => "Shell",
        "fish" => "Fish",
        "toml" => "TOML",
        "json" => "JSON",
        "yaml" => "YAML",
        "xml" => "XML",
        "ini" => "INI",
        "lua" => "Lua",
        "py" | "python" => "Python",
        "js" | "javascript" => "JavaScript",
        "ts" => "TypeScript",
        "md" => "Markdown",
        "rb" | "ruby" => "Ruby",
        "perl" | "pl" => "Perl",
        "applescript" => "AppleScript",
        "gitconfig" => "Git Config",
        "gitignore" => "Git Ignore",
        "ssh_config" => "SSH Config",
        "vim" => "Vim Script",
        "rs" => "Rust",
        "go" => "Go",
        "swift" => "Swift",
        "nix" => "Nix",
        "html" => "HTML",
        "css" => "CSS",
        "sql" => "SQL",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotfiles_get_their_language() {
        for (target, first_line, language) in [
            (".zshrc", "", "bash"),
            (".bash_profile", "", "bash"),
            (".tmux.conf", "", "bash"),
            (".gitconfig", "", "gitconfig"),
            (".gitconfig-github", "", "gitconfig"),
            (".config/git/config", "", "gitconfig"),
            (".ssh/config", "", "ssh_config"),
            ("Brewfile", "", "ruby"),
            (".config/fish/config.fish", "", "fish"),
            (".config/starship.toml", "", "toml"),
            (".config/nvim/init.lua", "", "lua"),
            ("Library/LaunchAgents/x.plist", "", "xml"),
            (".config/frp/frpc.conf", "", "ini"),
            (".local/bin/tool.sh", "", "bash"),
            (
                ".local/bin/say-as-me",
                "#!/usr/bin/env -S python3 -u",
                "python",
            ),
            (".local/bin/qdrive", "#!/bin/zsh", "bash"),
            (".local/bin/run", "#!/usr/bin/env fish", "fish"),
            (".local/bin/odd", "#!/usr/bin/awk -f", "odd"),
            ("notes", "", "notes"),
        ] {
            assert_eq!(language_for(target, first_line), language, "{target}");
        }
        assert_eq!(language_label("bash"), Some("Shell"));
        assert_eq!(language_label("notes"), None);
    }
}

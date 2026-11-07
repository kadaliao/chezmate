//! English and Chinese text for the simplified interface, following the
//! system language. `CHEZMATE_LANG=en` or `zh` overrides it.

use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Zh,
}

static LANG: OnceLock<Lang> = OnceLock::new();

pub fn lang() -> Lang {
    *LANG.get_or_init(detect)
}

/// Picks the language once and tells GPUI Kit, so its own buttons (OK,
/// Cancel) match.
pub fn init() {
    gpui_kit::component::set_locale(match lang() {
        Lang::En => "en",
        Lang::Zh => "zh-CN",
    });
}

/// The text for the current language.
pub fn tr<T>(en: T, zh: T) -> T {
    match lang() {
        Lang::En => en,
        Lang::Zh => zh,
    }
}

fn detect() -> Lang {
    if let Some(lang) = std::env::var("CHEZMATE_LANG").ok().and_then(|v| parse(&v)) {
        return lang;
    }
    // A Mac app follows the system language, which the terminal's LANG
    // often does not match; apps launched from Finder have no LANG at all.
    #[cfg(target_os = "macos")]
    if let Some(lang) = apple_languages().as_deref().and_then(first_language) {
        return lang;
    }
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|var| std::env::var(var).ok())
        .find(|value| !value.is_empty())
        .and_then(|value| parse(&value))
        .unwrap_or(Lang::En)
}

fn parse(value: &str) -> Option<Lang> {
    let value = value.trim().trim_matches('"').to_ascii_lowercase();
    if value.starts_with("zh") {
        Some(Lang::Zh)
    } else if value.is_empty() || value == "c" || value == "posix" {
        None
    } else {
        Some(Lang::En)
    }
}

#[cfg(target_os = "macos")]
fn apple_languages() -> Option<String> {
    let output = std::process::Command::new("/usr/bin/defaults")
        .args(["read", "-g", "AppleLanguages"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The first entry of `defaults read -g AppleLanguages`, e.g.
/// `(\n    "zh-Hans-CN",\n    "en-CN"\n)`.
fn first_language(list: &str) -> Option<Lang> {
    list.lines()
        .map(|line| line.trim().trim_matches(['(', ')', ',']).trim())
        .find(|line| !line.is_empty())
        .and_then(parse)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_locale_names() {
        assert_eq!(parse("zh_CN.UTF-8"), Some(Lang::Zh));
        assert_eq!(parse("\"zh-Hans-SG\""), Some(Lang::Zh));
        assert_eq!(parse("en_US.UTF-8"), Some(Lang::En));
        assert_eq!(parse("C"), None);
        assert_eq!(
            first_language("(\n    \"zh-Hans-SG\",\n    \"en-SG\"\n)\n"),
            Some(Lang::Zh)
        );
        assert_eq!(first_language("(\n    \"en-US\"\n)"), Some(Lang::En));
        assert_eq!(first_language("()"), None);
    }
}

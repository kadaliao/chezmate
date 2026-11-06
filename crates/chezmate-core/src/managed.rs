//! `chezmoi managed` output and source-state attributes.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// What kind of entry the source state describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
    Script,
    /// A `modify_` script that edits an existing file.
    Modify,
    /// A `remove_` entry that deletes the target.
    Remove,
}

/// Attributes encoded in a source file name such as
/// `private_executable_dot_local.tmpl`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SourceAttributes {
    pub template: bool,
    pub encrypted: bool,
    pub private: bool,
    pub readonly: bool,
    pub executable: bool,
    pub exact: bool,
    pub create: bool,
    pub empty: bool,
    /// `run_once_` scripts.
    pub once: bool,
    /// `run_onchange_` scripts.
    pub onchange: bool,
}

/// A target managed by chezmoi.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManagedEntry {
    /// Target path relative to the destination directory.
    pub target: String,
    pub absolute: String,
    pub source_absolute: String,
    /// Source path relative to the source directory.
    pub source_relative: String,
    pub kind: EntryKind,
    pub attributes: SourceAttributes,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPaths {
    absolute: String,
    source_absolute: String,
    source_relative: String,
}

/// Parses `chezmoi managed --path-style all --format json`.
pub fn parse_managed(json: &str) -> Result<Vec<ManagedEntry>> {
    if json.trim().is_empty() {
        return Ok(Vec::new());
    }
    let raw: BTreeMap<String, RawPaths> =
        serde_json::from_str(json).map_err(|err| Error::Parse {
            what: "chezmoi managed",
            message: err.to_string(),
        })?;
    Ok(raw
        .into_iter()
        .map(|(target, paths)| {
            let name = paths
                .source_relative
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string();
            let is_dir = Path::new(&paths.source_absolute).is_dir();
            let (kind, attributes) = classify(&name, is_dir);
            ManagedEntry {
                target,
                absolute: paths.absolute,
                source_absolute: paths.source_absolute,
                source_relative: paths.source_relative,
                kind,
                attributes,
            }
        })
        .collect())
}

/// Decodes the attributes of one source path component.
///
/// `is_dir` tells whether the source entry is a directory, because directory
/// names carry no suffix that would say so.
pub fn classify(name: &str, is_dir: bool) -> (EntryKind, SourceAttributes) {
    let mut attrs = SourceAttributes::default();
    let mut kind = if is_dir {
        EntryKind::Directory
    } else {
        EntryKind::File
    };
    let mut rest = name;

    // Prefixes may appear in a fixed order; accept any order but stop at the
    // name itself (`dot_` or `literal_` end the prefix list).
    loop {
        let before = rest;
        for (prefix, apply) in PREFIXES {
            if let Some(stripped) = rest.strip_prefix(prefix) {
                apply(&mut kind, &mut attrs);
                rest = stripped;
                break;
            }
        }
        if rest == before {
            break;
        }
    }

    if !is_dir {
        let mut stem = rest;
        loop {
            if attrs.encrypted
                && let Some(s) = stem
                    .strip_suffix(".age")
                    .or_else(|| stem.strip_suffix(".asc"))
            {
                stem = s;
                continue;
            }
            if let Some(s) = stem.strip_suffix(".tmpl") {
                attrs.template = true;
                stem = s;
                continue;
            }
            break;
        }
    }
    (kind, attrs)
}

type Apply = fn(&mut EntryKind, &mut SourceAttributes);

const PREFIXES: &[(&str, Apply)] = &[
    ("after_", |_, _| {}),
    ("before_", |_, _| {}),
    ("create_", |_, a| a.create = true),
    ("empty_", |_, a| a.empty = true),
    ("encrypted_", |_, a| a.encrypted = true),
    ("exact_", |_, a| a.exact = true),
    ("executable_", |_, a| a.executable = true),
    ("external_", |_, _| {}),
    ("modify_", |k, _| *k = EntryKind::Modify),
    ("once_", |_, a| a.once = true),
    ("onchange_", |_, a| a.onchange = true),
    ("private_", |_, a| a.private = true),
    ("readonly_", |_, a| a.readonly = true),
    ("remove_", |k, _| *k = EntryKind::Remove),
    ("run_", |k, _| *k = EntryKind::Script),
    ("symlink_", |k, _| *k = EntryKind::Symlink),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_file_attributes() {
        let (kind, attrs) = classify("private_executable_dot_local.tmpl", false);
        assert_eq!(kind, EntryKind::File);
        assert!(attrs.private && attrs.executable && attrs.template);
        assert!(!attrs.encrypted);
    }

    #[test]
    fn decodes_scripts_and_symlinks() {
        let (kind, attrs) = classify("run_once_before_install.sh.tmpl", false);
        assert_eq!(kind, EntryKind::Script);
        assert!(attrs.once && attrs.template);

        assert_eq!(classify("symlink_dot_vimrc", false).0, EntryKind::Symlink);
        assert_eq!(classify("modify_dot_npmrc", false).0, EntryKind::Modify);
    }

    #[test]
    fn decodes_encrypted_suffixes() {
        let (_, attrs) = classify("encrypted_private_dot_netrc.tmpl.age", false);
        assert!(attrs.encrypted && attrs.private && attrs.template);
        let (_, attrs) = classify("encrypted_dot_env.asc", false);
        assert!(attrs.encrypted && !attrs.template);
    }

    #[test]
    fn stops_at_the_real_name() {
        // `dot_private_notes` is the file `.private_notes`, not a private file.
        let (_, attrs) = classify("dot_private_notes", false);
        assert!(!attrs.private);
        let (_, attrs) = classify("literal_run_me", false);
        assert_eq!(classify("literal_run_me", false).0, EntryKind::File);
        assert_eq!(attrs, SourceAttributes::default());
    }

    #[test]
    fn directories_do_not_strip_suffixes() {
        let (kind, attrs) = classify("exact_private_dot_config.tmpl", true);
        assert_eq!(kind, EntryKind::Directory);
        assert!(attrs.exact && attrs.private && !attrs.template);
    }

    #[test]
    fn parses_managed_json() {
        let json = r#"{".hello":{"absolute":"/h/.hello","sourceAbsolute":"/s/dot_hello.tmpl","sourceRelative":"dot_hello.tmpl"}}"#;
        let entries = parse_managed(json).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].target, ".hello");
        assert!(entries[0].attributes.template);
        assert!(parse_managed("").unwrap().is_empty());
        assert_eq!(parse_managed("[").unwrap_err().kind(), "parse_error");
    }
}

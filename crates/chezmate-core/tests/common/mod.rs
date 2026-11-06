//! An isolated chezmoi setup: its own destination, source, config, state
//! and cache inside a temporary directory, so tests never read or write the
//! developer's real dotfiles.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use chezmate_core::{Chezmoi, ChezmoiOptions};
use tempfile::TempDir;

pub struct Sandbox {
    pub root: TempDir,
    pub home: PathBuf,
    pub source: PathBuf,
    pub config_dir: PathBuf,
    pub chezmoi: Chezmoi,
}

impl Sandbox {
    pub fn new() -> Self {
        Self::with_env(Vec::new())
    }

    /// A sandbox whose chezmoi gets `extra_env` on top of `HOME`.
    pub fn with_env(extra_env: Vec<(std::ffi::OsString, std::ffi::OsString)>) -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        // Canonicalize so paths match what chezmoi reports (macOS /var -> /private/var).
        let base = root.path().canonicalize().unwrap();
        let home = base.join("home");
        let source = base.join("source");
        // The config directory must not contain `home`: chezmoi refuses to add
        // files from inside its own config directory.
        let config_dir = base.join("config");
        for dir in [&home, &source, &config_dir] {
            fs::create_dir_all(dir).unwrap();
        }
        let config = config_dir.join("chezmoi.toml");
        fs::write(&config, "").unwrap();

        let chezmoi = Chezmoi::new(ChezmoiOptions {
            source: Some(source.clone()),
            destination: Some(home.clone()),
            config: Some(config),
            persistent_state: Some(config_dir.join("chezmoistate.boltdb")),
            cache: Some(base.join("cache")),
            env: [("HOME".into(), home.clone().into())]
                .into_iter()
                .chain(extra_env)
                .collect(),
            ..Default::default()
        })
        .expect("chezmoi must be installed to run these tests");
        Self {
            root,
            home,
            source,
            config_dir,
            chezmoi,
        }
    }

    pub fn write_source(&self, relative: &str, contents: &str) {
        write(&self.source.join(relative), contents);
    }

    pub fn write_home(&self, relative: &str, contents: &str) {
        write(&self.home.join(relative), contents);
    }

    pub fn read_home(&self, relative: &str) -> String {
        fs::read_to_string(self.home.join(relative)).unwrap()
    }

    pub fn read_source(&self, relative: &str) -> String {
        fs::read_to_string(self.source.join(relative)).unwrap()
    }
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

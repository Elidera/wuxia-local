//! Where things live on disk.
//!
//! * install dir — next to the .exe (read-only under Program Files): bundled
//!   llama-server, default config, bundled glossaries;
//! * data dir — `%LOCALAPPDATA%\WuxiaLocal` (or `<exe dir>\data` when a file
//!   `portable.txt` sits next to the exe): config, user glossary, cache,
//!   corrections, logs, downloaded models.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Paths {
    pub install: PathBuf,
    pub data: PathBuf,
}

impl Paths {
    pub fn detect() -> anyhow::Result<Self> {
        let exe = std::env::current_exe()?;
        let install = exe.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
        let data = if install.join("portable.txt").exists() {
            install.join("data")
        } else if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            PathBuf::from(local).join("WuxiaLocal")
        } else {
            install.join("data")
        };
        let p = Paths { install, data };
        for d in [p.data.clone(), p.logs(), p.models(), p.user_glossary_dir()] {
            std::fs::create_dir_all(d)?;
        }
        Ok(p)
    }

    pub fn config(&self) -> PathBuf {
        self.data.join("config.toml")
    }
    pub fn default_config(&self) -> PathBuf {
        self.install.join("config.default.toml")
    }
    pub fn logs(&self) -> PathBuf {
        self.data.join("logs")
    }
    pub fn models(&self) -> PathBuf {
        self.data.join("models")
    }
    pub fn cache(&self) -> PathBuf {
        self.data.join("cache.tsv")
    }
    pub fn corrections(&self) -> PathBuf {
        self.data.join("corrections.tsv")
    }
    pub fn bundled_glossary_dir(&self) -> PathBuf {
        self.install.join("glossary")
    }
    pub fn user_glossary_dir(&self) -> PathBuf {
        self.data.join("glossary")
    }
    pub fn user_glossary(&self) -> PathBuf {
        self.user_glossary_dir().join("user.tsv")
    }
    /// Resolve a possibly relative path against the install dir.
    pub fn resolve(&self, p: &str) -> PathBuf {
        let pb = PathBuf::from(p);
        if pb.is_absolute() {
            pb
        } else {
            self.install.join(pb)
        }
    }
}

use std::{
    collections::HashMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use anyhow::{bail, Context as _, Result};
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

// TODO: Add extra enum variants to allow trusting a workspace with the current config file
// contents (fingerprinted with a cryptographic hash).

const FILENAME: &str = "workspace-trust.toml";

pub static WORKSPACE_TRUST: Lazy<RwLock<WorkspaceTrust>> = Lazy::new(|| {
    let trust = match WorkspaceTrust::load() {
        Ok(trust) => trust,
        Err(err) => {
            log::error!("Error loading workspace trust: {err}");
            WorkspaceTrust::default()
        }
    };
    RwLock::new(trust)
});

/// Return the current workspace directory if it contains local configuration and also has
/// not been trusted.
pub fn get_workspace_with_untrusted_config() -> Option<PathBuf> {
    if crate::workspace_config_file().exists() {
        let (workspace, _) = crate::find_workspace();
        let this = WORKSPACE_TRUST.read();
        if this.persistent.0.contains_key(&workspace) {
            None
        } else {
            Some(workspace)
        }
    } else {
        None
    }
}

/// A command which allows or denies _trust_ in a directory.
///
/// In this context, _trust_ means that Helix will run LSP language servers and load local
/// configuration files (currently `.helix/config.toml` and `.helix/languages.toml`).
#[derive(Default, Clone, Copy)]
pub enum TrustWorkspace {
    /// The workspace directory is perpetually void of trust.
    DenyAlways,
    /// The workspace directory is not allowed to load config or run language servers, but the
    /// user will be asked for permission the next time this workspace is opened.
    #[default]
    DenyOnce,
    /// The workspace directory is always allowed to load trusted content like config and language
    /// servers.
    AllowAlways,
    // TODO: allow trusting with the current exact contents of the config file.
}

impl AsRef<str> for TrustWorkspace {
    fn as_ref(&self) -> &str {
        match self {
            Self::DenyAlways => "Never",
            Self::DenyOnce => "Not now",
            Self::AllowAlways => "Always",
        }
    }
}

#[derive(Debug, Default)]
pub struct WorkspaceTrust {
    /// Whether a workspace directory is trusted in the current session.
    ///
    /// This currently corresponds to the lifetime of the Helix binary. A value of `true` means
    /// that the workspace is temporarily trusted. `false` means that the workspace directory has
    /// not been trusted (default).
    transient: HashMap<PathBuf, bool>,
    /// A persistent database of directories and whether or not they are trusted.
    persistent: TrustStore,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TrustStore(HashMap<PathBuf, Trust>);

impl WorkspaceTrust {
    fn load() -> Result<Self> {
        let mut path = crate::state_dir();
        path.push(FILENAME);
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => bail!(err),
        };
        let persistent: TrustStore = toml::from_str(&contents)
            .context("failed to deserialize the workspace trust file as TOML")?;

        Ok(WorkspaceTrust {
            transient: HashMap::new(),
            persistent,
        })
    }

    pub fn declare_trust(&mut self, path: PathBuf, command: TrustWorkspace) -> Result<()> {
        let level = match command {
            TrustWorkspace::DenyOnce => {
                self.transient.insert(path, false);
                return Ok(());
            }
            TrustWorkspace::DenyAlways => Trust::Never,
            TrustWorkspace::AllowAlways => Trust::Always,
        };

        self.persistent.0.insert(path, level);

        let contents = toml::to_string(&self.persistent).unwrap();
        let mut path = crate::state_dir();
        fs::create_dir_all(&path).context("failed to create the state directory")?;
        path.push(FILENAME);
        let mut file = fs::File::create(path).context("failed to create workspace trust file")?;
        file.write_all(contents.as_bytes())
            .context("failed to write contents to the workspace trust file")?;
        Ok(())
    }

    pub fn is_trusted(&self, path: &Path) -> bool {
        if let Some(is_trusted) = self.transient.get(path) {
            return *is_trusted;
        }

        self.persistent.0.get(path) == Some(&Trust::Always)
    }
}

/// The level of trust of a given path.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Trust {
    /// The path is explicitly designated as not trustworthy.
    ///
    /// Any attempts to start a language server under this directory or load configuration will
    /// always fail.
    Never,
    /// The path is explicitly designated as trustworthy.
    ///
    /// Any attempts to start a language server under this directory or load configuration will
    /// succeed.
    Always,
    // TODO: allow trusting with the current exact contents of the config file?
}

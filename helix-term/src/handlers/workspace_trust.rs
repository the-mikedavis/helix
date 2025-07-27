use std::{collections::HashSet, path::PathBuf};

use helix_event::register_hook;
use helix_loader::workspace_trust::TrustWorkspace;
use helix_view::{events::DocumentDidOpen, handlers::Handlers};
use once_cell::sync::Lazy;
use parking_lot::Mutex;

use crate::ui;

pub const ID: &str = "workspace-trust-select";

/// A set of workspaces which have been prompted for trust at runtime.
static PROMPTED_WORKSPACES: Lazy<Mutex<HashSet<PathBuf>>> =
    Lazy::new(|| Mutex::new(HashSet::new()));

pub(super) fn register_hooks(_handlers: &Handlers) {
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        todo!();
        // Ok(())
    });
}

const TRUST_MESSAGE: &str = "Trust this workspace?

Trusted workspaces may load config files and auto-start language servers. Config and language servers can cause arbitrary code execution. Only trust workspaces which you know contain harmless config and code.";

pub fn select(path: PathBuf) -> ui::Select<TrustWorkspace> {
    let mut workspaces = PROMPTED_WORKSPACES.lock();
    workspaces.insert(path);
    ui::Select::new(
        TRUST_MESSAGE,
        [
            TrustWorkspace::DenyAlways,
            TrustWorkspace::DenyOnce,
            TrustWorkspace::AllowAlways,
        ],
        |editor, option, event| {
            if event == ui::PromptEvent::Validate {
                let mut trust = helix_loader::WORKSPACE_TRUST.write();
                if let Err(err) =
                    trust.declare_trust(helix_stdx::env::current_working_dir(), *option)
                {
                    editor.set_status(format!("Failed to save workspace trust: {err}"));
                }
            }
        },
    )
}

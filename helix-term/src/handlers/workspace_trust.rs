use std::{collections::HashSet, path::PathBuf};

use helix_event::register_hook;
use helix_loader::workspace_trust::TrustWorkspace;
use helix_view::{events::DocumentDidOpen, handlers::Handlers};
use once_cell::sync::Lazy;
use parking_lot::Mutex;

use crate::{compositor::Compositor, job, ui};

const ID: &str = "workspace-trust-select";

/// A set of canonicalized workspace paths which have been prompted for trust at runtime.
static PROMPTED_WORKSPACES: Lazy<Mutex<HashSet<PathBuf>>> =
    Lazy::new(|| Mutex::new(HashSet::new()));

pub(super) fn register_hooks(_handlers: &Handlers) {
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        let config = event.editor.config();
        if config.auto_trust {
            return Ok(());
        }
        let doc = doc!(event.editor, &event.doc);
        if doc.language_servers().next().is_none() {
            let (workspace, _) = helix_loader::find_workspace();
            job::dispatch_blocking(|_editor, compositor| prompt(workspace, compositor));
        }
        Ok(())
    });
}

pub fn prompt(path: PathBuf, compositor: &mut Compositor) {
    let mut workspaces = PROMPTED_WORKSPACES.lock();
    if workspaces.contains(&path) {
        return;
    } else {
        workspaces.insert(path.clone());
    }
    let select = select(path);
    compositor.replace_or_push(ID, select);
}

const TRUST_MESSAGE: &str = "Trust this workspace?

Trusted workspaces may load local config files and auto-start language servers. Config and language servers can execute arbitrary code. Only trust workspaces which you know contain harmless config and code.";

fn select(path: PathBuf) -> ui::Select<TrustWorkspace> {
    ui::Select::new(
        TRUST_MESSAGE,
        [
            TrustWorkspace::DenyOnce,
            TrustWorkspace::DenyAlways,
            TrustWorkspace::AllowAlways,
        ],
        (),
        move |editor, option, event| {
            if event == ui::PromptEvent::Validate {
                let mut trust = helix_loader::WORKSPACE_TRUST.write();
                if let Err(err) = trust.declare_trust(path.clone(), *option) {
                    editor.set_status(format!("Failed to save workspace trust: {err}"));
                }
            }
        },
    )
}

impl crate::ui::menu::Item for TrustWorkspace {
    type Data = ();

    fn format(&self, _data: &Self::Data) -> tui::widgets::Row {
        match self {
            TrustWorkspace::DenyAlways => "Never",
            TrustWorkspace::DenyOnce => "Not now",
            TrustWorkspace::AllowAlways => "Always",
        }
        .into()
    }
}

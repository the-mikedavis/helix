use std::{borrow::Cow, cmp::Ordering, collections::HashSet, fmt, future::Future};

use futures_util::{stream::FuturesOrdered, FutureExt};
use helix_core::syntax::config::LanguageServerFeature;
use helix_lsp::{
    lsp,
    util::{diagnostic_to_lsp_diagnostic, range_to_lsp_range},
    LanguageServerId,
};
use tokio_stream::StreamExt as _;

use crate::Editor;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CodeAction {
    Lsp(LspCodeAction),
    Internal(InternalCodeAction),
}

impl CodeAction {
    pub fn internal(
        title: impl Into<Cow<'static, str>>,
        action: impl Fn(&mut Editor) + Send + Sync + 'static,
    ) -> Self {
        Self::Internal(InternalCodeAction {
            title: title.into(),
            action: Box::new(action),
        })
    }

    pub fn help_text(&self) -> Cow<'_, str> {
        match self {
            Self::Lsp(LspCodeAction { item, .. }) => match item {
                lsp::CodeActionOrCommand::CodeAction(action) => action.title.as_str().into(),
                lsp::CodeActionOrCommand::Command(command) => command.title.as_str().into(),
            },
            Self::Internal(action) => action.title.as_ref().into(),
        }
    }

    pub fn execute(&self, editor: &mut Editor) {
        match self {
            Self::Lsp(item) => item.execute(editor),
            Self::Internal(action) => (action.action)(editor),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct LspCodeAction {
    pub item: lsp::CodeActionOrCommand,
    pub provider: LanguageServerId,
    // pub resolved: bool,
}

impl PartialOrd for LspCodeAction {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for LspCodeAction {
    fn cmp(&self, other: &Self) -> Ordering {
        // Sort codeactions into a useful order. This behaviour is only partially described in the LSP spec.
        // Many details are modeled after vscode because language servers are usually tested against it.
        // VScode sorts the codeaction two times:
        //
        // First the codeactions that fix some diagnostics are moved to the front.
        // If both codeactions fix some diagnostics (or both fix none) the codeaction
        // that is marked with `is_preferred` is shown first. The codeactions are then shown in separate
        // submenus that only contain a certain category (see `action_category`) of actions.
        // sort actions by category
        let order = self.category().cmp(&other.category());
        if order != Ordering::Equal {
            return order;
        }
        // within the categories sort by relevancy.
        // Modeled after the `codeActionsComparator` function in vscode:
        // https://github.com/microsoft/vscode/blob/eaec601dd69aeb4abb63b9601a6f44308c8d8c6e/src/vs/editor/contrib/codeAction/browser/codeAction.ts

        // if one code action fixes a diagnostic but the other one doesn't show it first
        let order = self
            .fixes_diagnostic()
            .cmp(&other.fixes_diagnostic())
            .reverse();
        if order != Ordering::Equal {
            return order;
        }

        // if one of the codeactions is marked as preferred show it first
        // otherwise keep the original LSP sorting
        self.is_preferred().cmp(&other.is_preferred()).reverse()
    }
}

impl LspCodeAction {
    pub fn execute(&self, editor: &mut Editor) {
        let Some(language_server) = editor.language_server_by_id(self.provider) else {
            editor.set_error("Language Server disappeared");
            return;
        };
        let offset_encoding = language_server.offset_encoding();

        match &self.item {
            lsp::CodeActionOrCommand::Command(command) => {
                log::debug!("code action command: {:?}", command);
                editor.execute_lsp_command(command.clone(), self.provider);
            }
            lsp::CodeActionOrCommand::CodeAction(code_action) => {
                log::debug!("code action: {:?}", code_action);
                // we support lsp "codeAction/resolve" for `edit` and `command` fields

                let mut code_action = if code_action.edit.is_none() || code_action.command.is_none()
                {
                    language_server
                        .resolve_code_action(code_action.clone())
                        .and_then(|future| helix_lsp::block_on(future).ok())
                        .and_then(|response| {
                            serde_json::from_value::<lsp::CodeAction>(response).ok()
                        })
                        .unwrap_or(code_action.clone())
                } else {
                    code_action.clone()
                };

                if let Some(ref workspace_edit) = code_action.edit {
                    let _ = editor.apply_workspace_edit(offset_encoding, workspace_edit);
                }

                // if code action provides both edit and command first the edit
                // should be applied and then the command
                if let Some(command) = code_action.command.take() {
                    editor.execute_lsp_command(command, self.provider);
                }
            }
        }
    }

    /// Determines the category of the `CodeAction` using the `CodeAction::kind` field.
    /// Returns a number that represent these categories.
    /// Categories with a lower number should be displayed first.
    ///
    ///
    /// While the `kind` field is defined as open ended in the LSP spec (any value may be used)
    /// in practice a closed set of common values (mostly suggested in the LSP spec) are used.
    /// VSCode displays each of these categories separately (separated by a heading in the codeactions picker)
    /// to make them easier to navigate. Helix does not display these  headings to the user.
    /// However it does sort code actions by their categories to achieve the same order as the VScode picker,
    /// just without the headings.
    ///
    /// The order used here is modeled after the [vscode sourcecode](https://github.com/microsoft/vscode/blob/eaec601dd69aeb4abb63b9601a6f44308c8d8c6e/src/vs/editor/contrib/codeAction/browser/codeActionWidget.ts>)
    fn category(&self) -> u32 {
        if let lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction {
            kind: Some(kind), ..
        }) = &self.item
        {
            let mut components = kind.as_str().split('.');
            match components.next() {
                Some("quickfix") => 0,
                Some("refactor") => match components.next() {
                    Some("extract") => 1,
                    Some("inline") => 2,
                    Some("rewrite") => 3,
                    Some("move") => 4,
                    Some("surround") => 5,
                    _ => 7,
                },
                Some("source") => 6,
                _ => 7,
            }
        } else {
            7
        }
    }

    fn is_preferred(&self) -> bool {
        matches!(
            self.item,
            lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction {
                is_preferred: Some(true),
                ..
            })
        )
    }

    fn fixes_diagnostic(&self) -> bool {
        matches!(
            &self.item,
            lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction {
                diagnostics: Some(diagnostics),
                ..
            }) if !diagnostics.is_empty()
        )
    }
}

/// Finds the available code actions given the current selection range.
pub fn find_under_primary_selection(
    editor: &Editor,
) -> Option<impl Future<Output = Vec<CodeAction>>> {
    let (view, doc) = current_ref!(editor);
    let selection = doc.selection(view.id).primary();

    // TODO: why do we guard the wall?
    let mut seen_language_servers = HashSet::new();

    let mut futures: FuturesOrdered<_> = doc
        .language_servers_with_feature(LanguageServerFeature::CodeAction)
        .filter(|ls| seen_language_servers.insert(ls.id()))
        .filter_map(|language_server| {
            let offset_encoding = language_server.offset_encoding();
            let language_server_id = language_server.id();
            let range = range_to_lsp_range(doc.text(), selection, offset_encoding);
            // Filter and convert overlapping diagnostics
            let context = lsp::CodeActionContext {
                diagnostics: doc
                    .diagnostics()
                    .iter()
                    .filter(|&diag| {
                        diag.provider.language_server_id() == Some(language_server_id)
                            && selection
                                .overlaps(&helix_core::Range::new(diag.range.start, diag.range.end))
                    })
                    .map(|diag| diagnostic_to_lsp_diagnostic(doc.text(), diag, offset_encoding))
                    .collect(),
                only: None,
                trigger_kind: Some(lsp::CodeActionTriggerKind::INVOKED),
            };
            let future = language_server.code_actions(doc.identifier(), range, context)?;
            Some((future, language_server_id))
        })
        .map(|(future, ls_id)| {
            async move {
                let Some(actions) = future.await? else {
                    return anyhow::Ok(Vec::new());
                };

                let actions: Vec<_> = actions
                    .into_iter()
                    .filter(|action| {
                        // remove disabled code actions
                        matches!(
                            action,
                            lsp::CodeActionOrCommand::Command(_)
                                | lsp::CodeActionOrCommand::CodeAction(lsp::CodeAction {
                                    disabled: None,
                                    ..
                                })
                        )
                    })
                    .map(move |action| {
                        CodeAction::Lsp(LspCodeAction {
                            item: action,
                            provider: ls_id,
                        })
                    })
                    .collect();

                Ok(actions)
            }
            .boxed()
        })
        .chain(crate::handlers::spelling::code_actions(editor))
        .collect();

    if futures.is_empty() {
        return None;
    }

    Some(async move {
        let mut actions = Vec::new();
        loop {
            match futures.try_next().await {
                Ok(next) => match next {
                    Some(items) => actions.extend(items),
                    None => break,
                },
                Err(_) => continue,
            }
        }
        actions.sort();
        actions
    })
}

pub struct InternalCodeAction {
    title: Cow<'static, str>,
    action: Box<dyn Fn(&mut Editor) + Send + Sync + 'static>,
}

impl InternalCodeAction {}

impl PartialEq for InternalCodeAction {
    fn eq(&self, other: &Self) -> bool {
        self.title == other.title
    }
}

impl Eq for InternalCodeAction {}

impl PartialOrd for InternalCodeAction {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for InternalCodeAction {
    fn cmp(&self, other: &Self) -> Ordering {
        self.title.cmp(&other.title)
    }
}

impl fmt::Debug for InternalCodeAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InternalCodeAction")
            .field("title", &self.title)
            .finish_non_exhaustive()
    }
}

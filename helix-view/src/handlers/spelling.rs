use std::{borrow::Cow, collections::HashMap};

use futures_util::{future::BoxFuture, FutureExt as _};
use helix_core::{diagnostic::DiagnosticProvider, SpellingLanguage, Tendril, Transaction};
use helix_event::{TaskController, TaskHandle};
use tokio::sync::mpsc::Sender;

use crate::{CodeAction, DocumentId, Editor};

#[derive(Debug)]
pub struct SpellingHandler {
    pub event_tx: Sender<SpellingEvent>,
    pub requests: HashMap<DocumentId, TaskController>,
}

impl SpellingHandler {
    pub fn new(event_tx: Sender<SpellingEvent>) -> Self {
        Self {
            event_tx,
            requests: Default::default(),
        }
    }

    pub fn open_request(&mut self, document: DocumentId) -> TaskHandle {
        let mut controller = TaskController::new();
        let handle = controller.restart();
        self.requests.insert(document, controller);
        handle
    }
}

#[derive(Debug)]
pub enum SpellingEvent {
    /*
    DictionaryUpdated {
        word: String,
        language: SpellingLanguage,
    },
    */
    DictionaryLoaded { language: SpellingLanguage },
    DocumentOpened { doc: DocumentId },
    DocumentChanged { doc: DocumentId },
}

pub(crate) fn code_actions(
    editor: &Editor,
) -> Option<BoxFuture<'static, anyhow::Result<Vec<crate::CodeAction>>>> {
    let (view, doc) = current_ref!(editor);
    let doc_id = doc.id();
    let view_id = view.id;
    let language = doc.spelling_language()?;
    let diagnostics = doc.diagnostics();
    // TODO: consider fixes for all selections?
    let range = doc.selection(view_id).primary();
    let text = doc.text().clone();
    let dictionary = editor.dictionaries.get(&language)?.clone();
    // TODO: can do this faster with partition_point + take_while
    let selected_diagnostics: Vec<_> = diagnostics
        .iter()
        .filter(|d| {
            range.overlaps(&helix_core::Range::new(d.range.start, d.range.end))
                && d.provider == DiagnosticProvider::Spelling
        })
        .map(|d| d.range)
        .collect();

    let future = tokio::task::spawn_blocking(move || {
        let text = text.slice(..);
        let dictionary = dictionary.read();
        let mut suggest_buffer = Vec::new();
        selected_diagnostics
            .into_iter()
            .flat_map(|range| {
                suggest_buffer.clear();
                let word = Cow::from(text.slice(range.start..range.end));
                dictionary.suggest(&word, &mut suggest_buffer);

                let mut actions = Vec::with_capacity(suggest_buffer.len() + 1);
                for suggestion in suggest_buffer.drain(..) {
                    actions.push(CodeAction::internal(
                        format!("Replace '{word}' with '{suggestion}'"),
                        move |editor| {
                            let doc = doc_mut!(editor, &doc_id);
                            let view = view_mut!(editor, view_id);
                            let transaction = Transaction::change(
                                doc.text(),
                                [(range.start, range.end, Some(Tendril::from(suggestion.as_str())))].into_iter(),
                            );
                            doc.apply(&transaction, view_id);
                            doc.append_changes_to_history(view);
                            // TODO: get rid of the diagnostic for this word.
                        },
                    ));
                }
                let word = word.to_string();
                actions.push(CodeAction::internal(
                    format!("Add '{word}' to dictionary '{language}'"),
                    move |editor| {
                        let Some(dictionary) = editor.dictionaries.get(&language) else {
                            log::error!("Failed to add '{word}' to dictionary '{language}' because the dictionary does not exist");
                            return;
                        };
                        // TODO: fire an event?
                        if let Err(err) = dictionary.write().add(&word) {
                            log::error!("Failed to add '{word}' to dictionary '{language}': {err}");
                        }
                    }
                ));
                actions
            })
            .collect()
    });
    Some(async move { Ok(future.await?) }.boxed())
}

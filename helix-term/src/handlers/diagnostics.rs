use std::collections::HashSet;
use std::time::Duration;

use helix_core::diagnostic::DiagnosticProvider;
use helix_core::syntax::LanguageServerFeature;
use helix_core::Uri;
use helix_event::{register_hook, send_blocking};
use helix_lsp::{lsp, LanguageServerId};
use helix_view::document::Mode;
use helix_view::events::{
    DiagnosticsDidChange, DocumentDidChange, DocumentDidOpen, LanguageServerInitialized,
};
use helix_view::handlers::diagnostics::DiagnosticEvent;
use helix_view::handlers::Handlers;
use helix_view::{Document, DocumentId, Editor};
use tokio::time::Instant;

use crate::events::OnModeSwitch;
use crate::job;

pub(super) fn register_hooks(handlers: &Handlers) {
    register_hook!(move |event: &mut DiagnosticsDidChange<'_>| {
        if event.editor.mode != Mode::Insert {
            for (view, _) in event.editor.tree.views_mut() {
                send_blocking(&view.diagnostics_handler.events, DiagnosticEvent::Refresh)
            }
        }
        Ok(())
    });
    register_hook!(move |event: &mut OnModeSwitch<'_, '_>| {
        for (view, _) in event.cx.editor.tree.views_mut() {
            view.diagnostics_handler.active = event.new_mode != Mode::Insert;
        }
        Ok(())
    });

    let tx = handlers.pull_diagnostics.document_tx.clone();
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        if event
            .doc
            .has_language_server_with_feature(LanguageServerFeature::PullDiagnostics)
        {
            send_blocking(&tx, event.doc.id());
        }
        Ok(())
    });

    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        let doc = doc!(event.editor, &event.doc);
        for language_server in
            doc.language_servers_with_feature(LanguageServerFeature::PullDiagnostics)
        {
            pull_diagnostics_for_document(doc, language_server);
        }

        Ok(())
    });

    register_hook!(move |event: &mut LanguageServerInitialized<'_>| {
        let language_server = event.editor.language_server_by_id(event.server_id).unwrap();
        if language_server.supports_feature(LanguageServerFeature::PullDiagnostics) {
            for doc in event
                .editor
                .documents()
                .filter(|doc| doc.supports_language_server(event.server_id))
            {
                pull_diagnostics_for_document(doc, language_server);
            }
        }
        Ok(())
    });
}

enum RequestKind {
    /// Diagnostics are being requested because a document has been opened or changed.
    Document,
    /// Diagnostics are being requested for all visible documents because the server declared
    /// inter-file dependencies in the capabilities.
    AllVisibleDocuments,
}

#[derive(Debug, Default)]
pub(super) struct DocumentDiagnosticsHandler {
    documents: HashSet<DocumentId>,
}

impl helix_event::AsyncHook for DocumentDiagnosticsHandler {
    type Event = DocumentId;

    fn handle_event(
        &mut self,
        doc: Self::Event,
        _timeout: Option<tokio::time::Instant>,
    ) -> Option<tokio::time::Instant> {
        self.documents.insert(doc);
        Some(Instant::now() + Duration::from_millis(125))
    }

    fn finish_debounce(&mut self) {
        let documents = std::mem::take(&mut self.documents);
        job::dispatch_blocking(move |editor, _| {
            for doc in documents {
                let Some(doc) = editor.document(doc) else {
                    continue;
                };

                for server in
                    doc.language_servers_with_feature(LanguageServerFeature::PullDiagnostics)
                {
                    pull_diagnostics_for_document(doc, server);
                }
            }
        });
    }
}

#[derive(Debug, Default)]
pub(super) struct InterFileDependencyDiagnosticsHandler {
    servers: HashSet<LanguageServerId>,
}

impl helix_event::AsyncHook for InterFileDependencyDiagnosticsHandler {
    type Event = LanguageServerId;

    fn handle_event(
        &mut self,
        server_id: Self::Event,
        _timeout: Option<tokio::time::Instant>,
    ) -> Option<tokio::time::Instant> {
        self.servers.insert(server_id);
        Some(Instant::now() + Duration::from_secs(1))
    }

    fn finish_debounce(&mut self) {
        let servers = std::mem::take(&mut self.servers);
        job::dispatch_blocking(move |editor, _| {
            for server_id in servers {
                let Some(server) = editor.language_server_by_id(server_id) else {
                    continue;
                };
                for doc in editor.documents() {
                    if doc.supports_language_server(server_id) {
                        pull_diagnostics_for_document_impl(
                            doc,
                            server,
                            RequestKind::AllVisibleDocuments,
                        );
                    }
                }
            }
        });
    }
}

pub fn pull_diagnostics_for_document(doc: &Document, language_server: &helix_lsp::Client) {
    pull_diagnostics_for_document_impl(doc, language_server, RequestKind::Document);
}

fn pull_diagnostics_for_document_impl(
    doc: &Document,
    language_server: &helix_lsp::Client,
    request_kind: RequestKind,
) {
    if !language_server.is_initialized() {
        return;
    }
    if !language_server.supports_feature(LanguageServerFeature::PullDiagnostics) {
        return;
    }
    let Some(features) = doc.language_config().and_then(|config| {
        config
            .language_servers
            .iter()
            .find(|features| features.name == language_server.name())
    }) else {
        return;
    };
    if !features.has_feature(LanguageServerFeature::PullDiagnostics) {
        return;
    }
    let Some(uri) = doc.uri() else {
        return;
    };
    let Some(capabilities) = language_server.capabilities().diagnostic_provider.as_ref() else {
        return;
    };
    let identifier = match capabilities {
        lsp::DiagnosticServerCapabilities::Options(options) => options.identifier.clone(),
        lsp::DiagnosticServerCapabilities::RegistrationOptions(options) => {
            options.diagnostic_options.identifier.clone()
        }
    };
    let inter_file_dependencies = match capabilities {
        lsp::DiagnosticServerCapabilities::Options(options) => options.inter_file_dependencies,
        lsp::DiagnosticServerCapabilities::RegistrationOptions(options) => {
            options.diagnostic_options.inter_file_dependencies
        }
    };
    let language_server_id = language_server.id();
    let provider = DiagnosticProvider::Lsp {
        server_id: language_server_id,
        identifier,
    };
    let document_id = doc.id();
    let Some(future) = language_server
        .text_document_diagnostic(doc.identifier(), doc.previous_diagnostic_id.clone())
    else {
        return;
    };

    tokio::spawn(async move {
        match future.await {
            Ok(result) => {
                job::dispatch(move |editor, _| {
                    handle_pull_diagnostics_response(editor, result, provider, uri, document_id);
                    // If this pull request was triggered by a document changing and the server
                    // declares inter-file dependencies in its capabilities, queue up a request
                    // for any open documents.
                    if matches!(request_kind, RequestKind::Document) && inter_file_dependencies {
                        editor
                            .handlers
                            .pull_diagnostics
                            .debounce_pull_visible_documents(language_server_id);
                    }
                })
                .await
            }
            Err(err) => {
                let cancellation_data = if let helix_lsp::Error::Rpc(error) = err {
                    error.data.and_then(|data| {
                        serde_json::from_value::<lsp::DiagnosticServerCancellationData>(data).ok()
                    })
                } else {
                    log::error!("Pull diagnostic request failed: {err}");
                    return;
                };

                if cancellation_data.is_some_and(|data| data.retrigger_request) {
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    job::dispatch(move |editor, _| {
                        if let Some((doc, language_server)) = editor
                            .document(document_id)
                            .zip(editor.language_server_by_id(language_server_id))
                        {
                            pull_diagnostics_for_document(doc, language_server);
                        }
                    })
                    .await;
                }
            }
        }
    });
}

fn handle_pull_diagnostics_response(
    editor: &mut Editor,
    result: lsp::DocumentDiagnosticReportResult,
    provider: DiagnosticProvider,
    uri: Uri,
    document_id: DocumentId,
) {
    let related_documents = match result {
        lsp::DocumentDiagnosticReportResult::Report(report) => {
            let (result_id, related_documents) = match report {
                lsp::DocumentDiagnosticReport::Full(report) => {
                    editor.handle_lsp_diagnostics(
                        &provider,
                        uri,
                        None,
                        report.full_document_diagnostic_report.items,
                    );

                    (
                        report.full_document_diagnostic_report.result_id,
                        report.related_documents,
                    )
                }
                lsp::DocumentDiagnosticReport::Unchanged(report) => (
                    Some(report.unchanged_document_diagnostic_report.result_id),
                    report.related_documents,
                ),
            };

            if let Some(doc) = editor.document_mut(document_id) {
                doc.previous_diagnostic_id = result_id;
            };

            related_documents
        }
        lsp::DocumentDiagnosticReportResult::Partial(partial_report) => {
            partial_report.related_documents
        }
    };

    for (url, report) in related_documents.into_iter().flatten() {
        let result_id = match report {
            lsp::DocumentDiagnosticReportKind::Full(report) => {
                let Ok(uri) = Uri::try_from(url) else {
                    continue;
                };

                editor.handle_lsp_diagnostics(&provider, uri, None, report.items);
                report.result_id
            }
            lsp::DocumentDiagnosticReportKind::Unchanged(report) => Some(report.result_id),
        };

        if let Some(doc) = editor.document_mut(document_id) {
            doc.previous_diagnostic_id = result_id;
        }
    }
}

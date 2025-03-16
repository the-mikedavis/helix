use helix_core::diagnostic::{DiagnosticProvider, Severity};
use helix_lsp::{lsp, util::lsp_severity_to_severity, LanguageServerId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Diagnostic {
    Core(helix_core::Diagnostic),
    Lsp {
        inner: lsp::Diagnostic,
        server_id: LanguageServerId,
    },
}

impl Diagnostic {
    pub fn language_server_id(&self) -> Option<LanguageServerId> {
        match self {
            Self::Lsp { server_id, .. } => Some(*server_id),
            _ => None,
        }
    }

    pub fn source(&self) -> Option<&str> {
        match self {
            Self::Lsp { inner, .. } => inner.source.as_deref(),
            Self::Core(inner) => inner.source.as_deref(),
        }
    }

    pub fn is_provider(&self, provider: DiagnosticProvider) -> bool {
        match self {
            Self::Core(inner) => inner.provider == provider,
            Self::Lsp { server_id, .. } => match provider {
                DiagnosticProvider::Lsp(id) => *server_id == id,
                _ => false,
            },
        }
    }

    pub fn severity(&self) -> Option<Severity> {
        match self {
            Self::Core(inner) => inner.severity,
            Self::Lsp { inner, .. } => inner.severity.and_then(lsp_severity_to_severity),
        }
    }
}

impl PartialOrd for Diagnostic {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Diagnostic {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;

        match (self, other) {
            (Self::Core(self_inner), Self::Core(other_inner)) => (
                self_inner.severity,
                self_inner.range.start,
                self_inner.provider,
            )
                .cmp(&(
                    other_inner.severity,
                    other_inner.range.start,
                    other_inner.provider,
                )),
            (Self::Core(_), Self::Lsp { .. }) => Ordering::Less,
            (Self::Lsp { .. }, Self::Core(_)) => Ordering::Greater,
            (
                Self::Lsp {
                    inner: self_inner,
                    server_id: self_id,
                },
                Self::Lsp {
                    inner: other_inner,
                    server_id: other_id,
                },
            ) => (self_inner.severity, self_inner.range.start, self_id).cmp(&(
                other_inner.severity,
                other_inner.range.start,
                other_id,
            )),
        }
    }
}

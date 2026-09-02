use dashmap::DashMap;
use std::path::PathBuf;
use tower_lsp::jsonrpc::Result as RpcResult;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::analysis::{self, AnalysisResult};
use crate::position::{offset_to_position, position_to_offset};
use crate::semantic;

pub struct Backend {
    client: Client,
    /// Live buffer text per open document, updated on every keystroke.
    /// Independent of `analysis` below — see `analysis.rs`'s module doc.
    docs: DashMap<Url, String>,
    /// Last disk-based analysis (diagnostics, symbols) per open document,
    /// refreshed on open/change/save.
    analysis: DashMap<Url, AnalysisResult>,
}

/// One malformed file must degrade one feature, not kill the whole server:
/// tower-lsp drives every request off the same root future, so an
/// uncaught panic anywhere unwinds all the way out of `main`, taking every
/// other open file's highlighting and diagnostics down with it (see the
/// `position.rs` trailing-newline-position bug this guarded against). Every
/// request handler below routes its real work through this instead of
/// calling straight into `analysis`/`semantic`/`position`.
fn guarded<T>(f: impl FnOnce() -> T) -> Option<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(value) => Some(value),
        Err(_) => None, // already logged to stderr by the default panic hook
    }
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Self { client, docs: DashMap::new(), analysis: DashMap::new() }
    }

    fn path_of(uri: &Url) -> Option<PathBuf> {
        uri.to_file_path().ok()
    }

    async fn refresh(&self, uri: &Url) {
        let Some(path) = Self::path_of(uri) else { return };
        let overlay = self.docs.get(uri).map(|entry| entry.value().clone());
        let Some(result) = guarded(|| analysis::analyze_file(&path, overlay.as_deref())) else {
            self.client
                .log_message(MessageType::ERROR, format!("bitterasm-lsp: analysis panicked on {uri}"))
                .await;
            return;
        };

        let diagnostics = result
            .diagnostics
            .iter()
            .filter_map(|diagnostic| to_lsp_diagnostic(diagnostic, &result))
            .collect();
        self.analysis.insert(uri.clone(), result);
        self.client.publish_diagnostics(uri.clone(), diagnostics, None).await;
    }
}

fn to_lsp_diagnostic(
    diagnostic: &bitterasm::diagnostics::Diagnostic, result: &AnalysisResult,
) -> Option<Diagnostic> {
    let primary = diagnostic
        .labels
        .iter()
        .find(|label| label.style == bitterasm::diagnostics::LabelStyle::Primary)
        .or_else(|| diagnostic.labels.first())?;
    if primary.source != result.entry_source {
        // A label pointing into an imported file, not the entry file this
        // diagnostics list belongs to. Rare (most errors are reported
        // against the file that actually names the problem) — skipped for
        // now rather than mis-anchored to the wrong document.
        return None;
    }
    let file = result.sources.get(primary.source)?;
    let range = Range {
        start: offset_to_position(&file.source, primary.span.start),
        end: offset_to_position(&file.source, primary.span.end),
    };
    let severity = Some(match diagnostic.severity {
        bitterasm::diagnostics::Severity::Error => DiagnosticSeverity::ERROR,
        bitterasm::diagnostics::Severity::Warning => DiagnosticSeverity::WARNING,
        bitterasm::diagnostics::Severity::Note => DiagnosticSeverity::INFORMATION,
        bitterasm::diagnostics::Severity::Help => DiagnosticSeverity::HINT,
    });
    Some(Diagnostic {
        range,
        severity,
        code: diagnostic.lint.map(|lint| NumberOrString::String(lint.as_str().to_string())),
        source: Some("bitterasm".to_string()),
        message: diagnostic.message.clone(),
        ..Default::default()
    })
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> RpcResult<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(SemanticTokensOptions {
                        legend: semantic::legend(),
                        full: Some(SemanticTokensFullOptions::Bool(true)),
                        ..Default::default()
                    }),
                ),
                definition_provider: Some(OneOf::Left(true)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "bitterasm-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client.log_message(MessageType::INFO, "bitterasm-lsp ready").await;
    }

    async fn shutdown(&self) -> RpcResult<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        self.docs.insert(uri.clone(), params.text_document.text);
        self.refresh(&uri).await;
    }

    async fn did_change(&self, mut params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        // Full sync: `initialize` advertised TextDocumentSyncKind::FULL, so
        // there's always exactly one change carrying the whole new text.
        if let Some(change) = params.content_changes.pop() {
            self.docs.insert(uri.clone(), change.text);
        }
        self.refresh(&uri).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        self.refresh(&params.text_document.uri).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.docs.remove(&uri);
        self.analysis.remove(&uri);
        self.client.publish_diagnostics(uri, Vec::new(), None).await;
    }

    async fn semantic_tokens_full(
        &self, params: SemanticTokensParams,
    ) -> RpcResult<Option<SemanticTokensResult>> {
        let uri = params.text_document.uri;
        let Some(text) = self.docs.get(&uri).map(|entry| entry.value().clone()) else {
            return Ok(None);
        };
        let symbols = self.analysis.get(&uri);
        let Some(data) =
            guarded(|| semantic::tokenize(&text, symbols.as_ref().and_then(|a| a.symbols.as_ref())))
        else {
            return Ok(None);
        };
        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens { result_id: None, data })))
    }

    async fn goto_definition(
        &self, params: GotoDefinitionParams,
    ) -> RpcResult<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let Some(text) = self.docs.get(&uri).map(|entry| entry.value().clone()) else {
            return Ok(None);
        };
        let Some(result) = self.analysis.get(&uri) else { return Ok(None) };

        let response = guarded(|| {
            let offset = position_to_offset(&text, position);

            // A click on `from <module path> import ...` jumps to that
            // file (top, since there's no single "declaration" to land on)
            // rather than going through the symbol table at all — a module
            // path segment like `c_like` in `std.riscv.c_like` isn't a
            // declared macro/type/const, so `identifier_at` +
            // `find_definition` below would never find it.
            if let Some(target_path) = analysis::find_import_target(&result, offset) {
                let target_uri = Url::from_file_path(&target_path).ok()?;
                let range = Range { start: Position { line: 0, character: 0 }, end: Position { line: 0, character: 0 } };
                return Some(GotoDefinitionResponse::Scalar(Location { uri: target_uri, range }));
            }

            let name = analysis::identifier_at(&text, offset)?;
            let (path, span) = analysis::find_definition(&result, &name)?;
            let source_id = result.sources.locate_span(span, Some(&name)).unwrap_or(result.entry_source);
            let file = result.sources.get(source_id)?;
            let target_uri = Url::from_file_path(&path).ok()?;
            let range = Range {
                start: offset_to_position(&file.source, span.start),
                end: offset_to_position(&file.source, span.end),
            };
            Some(GotoDefinitionResponse::Scalar(Location { uri: target_uri, range }))
        });
        Ok(response.flatten())
    }

    async fn hover(&self, params: HoverParams) -> RpcResult<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let Some(text) = self.docs.get(&uri).map(|entry| entry.value().clone()) else {
            return Ok(None);
        };
        let Some(result) = self.analysis.get(&uri) else { return Ok(None) };

        let response = guarded(|| {
            let offset = position_to_offset(&text, position);
            let name = analysis::identifier_at(&text, offset)?;
            let symbols = result.symbols.as_ref()?;
            let symbol = symbols.get(symbols.lookup(&name)?);
            Some(Hover {
                contents: HoverContents::Scalar(MarkedString::String(format!(
                    "```basm\n{:?} {}\n```",
                    symbol.kind, symbol.name
                ))),
                range: None,
            })
        });
        Ok(response.flatten())
    }
}

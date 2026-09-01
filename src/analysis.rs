//! Disk-based analysis of a single `.basm` file: treats the file as its own
//! entry point (mirroring `bitterasm check <file>`), flattens its imports,
//! resolves symbols, and produces diagnostics — everything go-to-definition
//! and publishDiagnostics need. Re-run on open/change/save; nothing here
//! depends on unsaved buffer state in *imported* files, only the entry file
//! itself (see `overlay`).
//!
//! Live syntax highlighting does not go through this module at all — it
//! only needs `bitterasm::lexer::lex` on the current buffer text, so it
//! stays correct on every keystroke even while this module's last analysis
//! is stale. See `semantic.rs`.

use std::path::{Path, PathBuf};

use bitterasm::diagnostics::{self, Diagnostic, LintConfig, SourceId, SourceMap};
use bitterasm::resolver::{self, SymbolId, SymbolTable};
use bitterasm::{formatter, loader};

pub struct AnalysisResult {
    pub sources: SourceMap,
    pub entry_source: SourceId,
    pub diagnostics: Vec<Diagnostic>,
    pub symbols: Option<SymbolTable>,
}

/// `overlay` is the entry file's own unsaved buffer text, if the editor has
/// it open with pending edits; every other file in the import graph is
/// re-read from disk. This keeps go-to-definition and diagnostics correct
/// for the file you're actively typing in without needing to fork the
/// compiler's loader to support a general overlay filesystem.
pub fn analyze_file(entry: &Path, overlay: Option<&str>) -> AnalysisResult {
    let mut sources = SourceMap::default();
    let entry_text = match overlay {
        Some(text) => text.to_string(),
        None => std::fs::read_to_string(entry).unwrap_or_default(),
    };
    let entry_source = sources.add(entry, entry_text);

    let config = match formatter::discover_config(entry) {
        Some(config_path) => diagnostics::load_lint_config(&config_path).unwrap_or_default(),
        None => LintConfig::default(),
    };

    let mut diags = Vec::new();

    let program = match loader::load_entry_program(entry) {
        Ok(program) => program,
        Err(error) => {
            diags.push(diagnostics::load_error(error, &mut sources));
            return AnalysisResult { sources, entry_source, diagnostics: diags, symbols: None };
        }
    };
    diags.extend(diagnostics::lint_program(&program, entry_source, &config));

    if let Ok(files) = loader::load_sources(entry) {
        for (path, text) in files {
            sources.add(path, text);
        }
    }

    let flattened = match loader::load_program(entry) {
        Ok(program) => program,
        Err(error) => {
            diags.push(diagnostics::load_error(error, &mut sources));
            return AnalysisResult { sources, entry_source, diagnostics: diags, symbols: None };
        }
    };
    let flattened = match resolver::unroll_top_level(flattened) {
        Ok(program) => program,
        Err(error) => {
            let source = sources.locate_span(error.span(), error.source_needle());
            diags.push(diagnostics::resolve_error(error, source));
            return AnalysisResult { sources, entry_source, diagnostics: diags, symbols: None };
        }
    };
    if let Err(error) = resolver::validate_facets(&flattened) {
        let source = sources.locate_span(error.span(), error.source_needle());
        diags.push(diagnostics::resolve_error(error, source));
        return AnalysisResult { sources, entry_source, diagnostics: diags, symbols: None };
    }

    let symbols = match resolver::collect_symbols(&flattened) {
        Ok(symbols) => symbols,
        Err(error) => {
            let source = sources.locate_span(error.span(), error.source_needle());
            diags.push(diagnostics::resolve_error(error, source));
            return AnalysisResult { sources, entry_source, diagnostics: diags, symbols: None };
        }
    };

    if let Err(error) = resolver::ConstEvaluator::new(&flattened, &symbols).evaluate_all() {
        let source = sources.locate_span(error.span(), error.source_needle());
        diags.push(diagnostics::resolve_error(error, source));
    }

    AnalysisResult { sources, entry_source, diagnostics: diags, symbols: Some(symbols) }
}

/// The identifier token, if any, whose span covers `offset` (or touches it
/// at either edge, matching how editors report a cursor sitting right
/// after a word). Used to turn a goto-definition click into a name to look
/// up in the `SymbolTable`.
pub fn identifier_at(text: &str, offset: usize) -> Option<String> {
    let tokens = bitterasm::lexer::lex(text).ok()?;
    tokens.into_iter().find_map(|token| {
        if token.span.start <= offset && offset <= token.span.end {
            if let bitterasm::token::TokenKind::Identifier(name) = token.kind {
                return Some(name);
            }
        }
        None
    })
}

/// Resolves an identifier's name to the file + byte span of its top-level
/// declaration (macro, type, struct, enum, or const). Local bindings —
/// macro parameters, `match` arms — aren't in the whole-program
/// `SymbolTable` at all, so they fall through to `None`; jumping to those
/// isn't supported yet.
pub fn find_definition(
    result: &AnalysisResult, name: &str,
) -> Option<(PathBuf, bitterasm::token::Span)> {
    let symbols = result.symbols.as_ref()?;
    let id: SymbolId = symbols.lookup(name)?;
    let symbol = symbols.get(id);
    let source_id = result.sources.locate_span(symbol.span, Some(&symbol.name))?;
    let file = result.sources.get(source_id)?;
    Some((file.name.clone(), symbol.span))
}

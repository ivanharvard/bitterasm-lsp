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

use bitterasm::ast::{ImportStatement, Statement};
use bitterasm::diagnostics::{self, Diagnostic, LintConfig, SourceId, SourceMap};
use bitterasm::resolver::{self, SymbolId, SymbolTable};
use bitterasm::{formatter, loader};

pub struct AnalysisResult {
    pub sources: SourceMap,
    pub entry_source: SourceId,
    pub entry_path: PathBuf,
    pub diagnostics: Vec<Diagnostic>,
    pub symbols: Option<SymbolTable>,
    /// The entry file's own `from ... import ...` statements, unflattened
    /// (straight from `load_entry_program`, before the loader merges every
    /// imported declaration into one program and the import statements
    /// themselves disappear) — see `find_import_target`.
    pub imports: Vec<ImportStatement>,
}

/// `overlay` is the entry file's own unsaved buffer text, if the editor has
/// it open with pending edits; every other file in the import graph is
/// re-read from disk. This keeps go-to-definition and diagnostics correct
/// for the file you're actively typing in without needing to fork the
/// compiler's loader to support a general overlay filesystem.
pub fn analyze_file(entry: &Path, overlay: Option<&str>) -> AnalysisResult {
    let entry_path = entry.to_path_buf();
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

    macro_rules! bail {
        () => {
            return AnalysisResult {
                sources, entry_source, entry_path, diagnostics: diags, symbols: None,
                imports: Vec::new(),
            }
        };
        ($imports:expr) => {
            return AnalysisResult {
                sources, entry_source, entry_path, diagnostics: diags, symbols: None,
                imports: $imports,
            }
        };
    }

    let program = match loader::load_entry_program(entry) {
        Ok(program) => program,
        Err(error) => {
            diags.push(diagnostics::load_error(error, &mut sources));
            bail!();
        }
    };
    diags.extend(diagnostics::lint_program(&program, entry_source, &config));

    let imports: Vec<ImportStatement> = program
        .statements
        .iter()
        .filter_map(|statement| match statement {
            Statement::Import(import) => Some(import.clone()),
            _ => None,
        })
        .collect();

    if let Ok(files) = loader::load_sources(entry) {
        for (path, text) in files {
            sources.add(path, text);
        }
    }

    let flattened = match loader::load_program(entry) {
        Ok(program) => program,
        Err(error) => {
            diags.push(diagnostics::load_error(error, &mut sources));
            bail!(imports);
        }
    };
    let flattened = match resolver::unroll_top_level(flattened) {
        Ok(program) => program,
        Err(error) => {
            let source = sources.locate_span(error.span(), error.source_needle());
            diags.push(diagnostics::resolve_error(error, source));
            bail!(imports);
        }
    };
    if let Err(error) = resolver::validate_facets(&flattened) {
        let source = sources.locate_span(error.span(), error.source_needle());
        diags.push(diagnostics::resolve_error(error, source));
        bail!(imports);
    }

    let symbols = match resolver::collect_symbols(&flattened) {
        Ok(symbols) => symbols,
        Err(error) => {
            let source = sources.locate_span(error.span(), error.source_needle());
            diags.push(diagnostics::resolve_error(error, source));
            bail!(imports);
        }
    };

    if let Err(error) = resolver::ConstEvaluator::new(&flattened, &symbols).evaluate_all() {
        let source = sources.locate_span(error.span(), error.source_needle());
        diags.push(diagnostics::resolve_error(error, source));
    }

    AnalysisResult { sources, entry_source, entry_path, diagnostics: diags, symbols: Some(symbols), imports }
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

/// The file a `from <module path> import ...` statement's module path
/// points at, if `offset` falls inside that path — reimplements bitterasm's
/// own (crate-private) `loader::resolve_module_path`/`module_base_dir`
/// rather than calling it, since neither is reachable from outside the
/// compiler crate. A non-relative path (`relative_level == 0`, e.g. `from
/// std.riscv.c_like import *`) resolves against the process's current
/// directory — the same "must be the workspace root" requirement
/// `analyze_file`'s callers already have to satisfy for the loader itself
/// to find anything. A relative path (`from .foo import *`, `relative_level
/// == 1`) resolves against the importing file's own directory, walking up
/// one more parent per extra leading dot.
///
/// Doesn't (yet) handle the submodule sugar `from std import u8string`
/// reaching for `std/u8string.basm` — that's a click on an imported *name*
/// after `import`, not on the module path itself, a different span than
/// what this checks against.
pub fn find_import_target(result: &AnalysisResult, offset: usize) -> Option<PathBuf> {
    let import = result
        .imports
        .iter()
        .find(|import| import.module.span.start <= offset && offset <= import.module.span.end)?;

    let base = if import.module.relative_level == 0 {
        std::env::current_dir().ok()?
    } else {
        let mut dir = result.entry_path.parent()?.to_path_buf();
        for _ in 1..import.module.relative_level {
            dir = dir.parent().map(Path::to_path_buf).unwrap_or(dir);
        }
        dir
    };

    let mut candidate = base;
    for segment in &import.module.segments {
        candidate.push(segment);
    }
    candidate.set_extension("basm");
    candidate.canonicalize().ok()
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

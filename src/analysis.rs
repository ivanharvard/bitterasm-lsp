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

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bitterasm::ast::{ImportStatement, Statement};
use bitterasm::diagnostics::{self, Diagnostic, LintConfig, SourceId, SourceMap};
use bitterasm::resolver::{self, SymbolId, SymbolKind, SymbolTable};
use bitterasm::token::{Span, Token, TokenKind};
use bitterasm::{formatter, loader};

pub struct AnalysisResult {
    pub sources: SourceMap,
    pub entry_source: SourceId,
    pub entry_path: PathBuf,
    pub diagnostics: Vec<Diagnostic>,
    pub symbols: Option<SymbolTable>,
    /// Imported public labels are represented in the compiler's symbol table
    /// by a synthetic declaration at the import site. Keep their real source
    /// files so go-to-definition can land on the declaring label instead.
    pub extern_labels: HashMap<String, PathBuf>,
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
                extern_labels: HashMap::new(),
                imports: Vec::new(),
            }
        };
        ($imports:expr) => {
            return AnalysisResult {
                sources, entry_source, entry_path, diagnostics: diags, symbols: None,
                extern_labels: HashMap::new(),
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

    // `origins` is what lets `collect_symbols` (below) tag each declaration
    // with the module that actually wrote it — without it, every
    // non-`pub` struct field would look like it belongs to whichever
    // module happens to occupy id 0, silently passing (or wrongly
    // failing) `bitterasm`'s cross-module field-visibility check instead
    // of matching what `bitterasm check` itself would report.
    let (flattened, origins) = match loader::load_program_with_modules(entry) {
        Ok(result) => result,
        Err(error) => {
            diags.push(diagnostics::load_error(error, &mut sources));
            bail!(imports);
        }
    };
    let extern_labels = flattened
        .statements
        .iter()
        .filter_map(|statement| match statement {
            Statement::ExternLabel(label) => {
                Some((label.name.clone(), PathBuf::from(&label.file)))
            }
            _ => None,
        })
        .collect();
    let (flattened, statement_modules) = match resolver::unroll_top_level(flattened, origins.all()) {
        Ok(result) => result,
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

    let symbols = match resolver::collect_symbols(&flattened, &statement_modules) {
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

    AnalysisResult {
        sources, entry_source, entry_path, diagnostics: diags, symbols: Some(symbols),
        extern_labels, imports,
    }
}

/// Every name-shaped token in `tokens`, with its span: plain identifiers,
/// plus dotted label names (`.loop`), which the compiler lexes as a `.`
/// glued to a following identifier or keyword and only fuses in the parser.
/// A `.` counts as a label's leading dot when it's glued to the word after
/// it but *not* to a value before it — `jmp .loop` and `.loop:` fuse,
/// `str.len` stays member access.
pub fn names(tokens: &[Token]) -> Vec<(String, Span)> {
    let mut names = Vec::new();
    let mut index = 0;

    while index < tokens.len() {
        let token = &tokens[index];

        if let TokenKind::Identifier(name) = &token.kind {
            names.push((name.clone(), token.span));
        } else if token.kind == TokenKind::Dot {
            let glued_after = tokens
                .get(index + 1)
                .filter(|next| next.span.start == token.span.end)
                .and_then(|next| next.kind.word_text().map(|word| (word, next.span)));
            let glued_before = index > 0
                && tokens[index - 1].span.end == token.span.start
                && ends_value(&tokens[index - 1].kind);

            if let (Some((word, word_span)), false) = (glued_after, glued_before) {
                names.push((format!(".{word}"), Span::new(token.span.start, word_span.end)));
                index += 2;
                continue;
            }
        }

        index += 1;
    }

    names
}

fn ends_value(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Identifier(_)
            | TokenKind::Integer(_)
            | TokenKind::String(_)
            | TokenKind::RParen
            | TokenKind::RBracket
            | TokenKind::RBrace
    )
}

/// The name (see `names`), if any, whose span covers `offset` (or touches
/// it at either edge, matching how editors report a cursor sitting right
/// after a word). Used to turn a goto-definition click into a name to look
/// up in the `SymbolTable`.
pub fn identifier_at(text: &str, offset: usize) -> Option<String> {
    let tokens = bitterasm::lexer::lex(text).ok()?;
    names(&tokens)
        .into_iter()
        .find(|(_, span)| span.start <= offset && offset <= span.end)
        .map(|(name, _)| name)
}

/// The file a `from <module path> import ...` statement's module path
/// points at, if `offset` falls inside that path — resolved by bitterasm's
/// own `loader::resolve_module_path`, so an absolute path finds the same file
/// the compiler would: under the current directory, a `BITTERASM_PATH`
/// entry, or the installed `~/.bitterasm/std`.
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

    loader::resolve_module_path(&import.module, &result.entry_path).ok()
}

/// Resolves an identifier's name to the file + byte span of its top-level
/// declaration (macro, type, struct, enum, const, or label). Imported public
/// labels need the `extern_labels` side table because their compiler symbol
/// is synthesized at the import site rather than at the real declaration.
/// Local bindings — macro parameters, `match` arms — aren't in the whole-
/// program `SymbolTable` at all, so they fall through to `None`; jumping to
/// those isn't supported yet.
pub fn find_definition(
    result: &AnalysisResult, name: &str,
) -> Option<(PathBuf, bitterasm::token::Span)> {
    let symbols = result.symbols.as_ref()?;
    let id: SymbolId = symbols.lookup(name)?;
    let symbol = symbols.get(id);

    if symbol.kind == SymbolKind::ExternLabel {
        let path = result.extern_labels.get(name)?;
        let program = loader::load_entry_program(path).ok()?;
        let span = program.statements.into_iter().find_map(|statement| match statement {
            Statement::Label(label) if label.is_pub && label.name == name => Some(label.span),
            _ => None,
        })?;
        return Some((path.clone(), span));
    }

    let source_id = result.sources.locate_span(symbol.span, Some(&symbol.name))?;
    let file = result.sources.get(source_id)?;
    Some((file.name.clone(), symbol.span))
}

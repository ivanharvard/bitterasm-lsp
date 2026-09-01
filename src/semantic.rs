//! Semantic-token syntax highlighting — but *only* for the one thing
//! `syntaxes/basm.tmLanguage.json` structurally cannot know: which plain
//! identifier names a real top-level macro, type, struct, enum, const, or
//! label, per a fresh `analysis::AnalysisResult`'s resolved `SymbolTable`.
//!
//! Earlier this also reclassified every keyword, operator, number, string,
//! and comment straight from the lexer. That was redundant with the
//! TextMate grammar for anything that classification could already tell
//! apart lexically — and worse than redundant in practice: VS Code layers
//! semantic tokens *over* TextMate scopes, so a theme with sparse
//! `semanticTokenColors` (most of them, for a colorless type like the
//! generic LSP `variable`/`operator`) ends up flatter than the TextMate-only
//! view a viewer briefly sees before the language server attaches, instead
//! of richer. Emitting tokens only for the symbol-resolved upgrade keeps
//! semantic highlighting strictly additive: every other scope stays exactly
//! what the grammar already gives it, in any theme.
//!
//! A consequence: with no `AnalysisResult` yet (nothing has resolved this
//! file successfully so far), there is nothing to upgrade and this returns
//! no tokens at all — that's correct, not a fallback to guard against; the
//! grammar alone is already a complete, live view of a file that doesn't
//! parse or resolve yet.

use bitterasm::lexer;
use bitterasm::resolver::{SymbolKind, SymbolTable};
use bitterasm::token::TokenKind;
use tower_lsp::lsp_types::{SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend};

pub const TOKEN_TYPES: &[SemanticTokenType] = &[
    SemanticTokenType::MACRO,
    SemanticTokenType::TYPE,
    SemanticTokenType::VARIABLE,
    SemanticTokenType::EVENT,
];

const MACRO: u32 = 0;
const TYPE: u32 = 1;
const VARIABLE: u32 = 2;
const EVENT: u32 = 3;

pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: TOKEN_TYPES.to_vec(),
        token_modifiers: vec![SemanticTokenModifier::READONLY],
    }
}

fn upgrade_from_symbols(name: &str, symbols: &SymbolTable) -> Option<(u32, u32)> {
    let symbol = symbols.get(symbols.lookup(name)?);
    Some(match symbol.kind {
        SymbolKind::Macro => (MACRO, 0),
        SymbolKind::Struct | SymbolKind::Enum | SymbolKind::TypeAlias => (TYPE, 0),
        SymbolKind::Const => (VARIABLE, 1 << 0), // readonly
        SymbolKind::Label => (EVENT, 0),
    })
}

pub fn tokenize(source: &str, symbols: Option<&SymbolTable>) -> Vec<SemanticToken> {
    let Some(symbols) = symbols else { return Vec::new() };
    let Ok(tokens) = lexer::lex(source) else { return Vec::new() };

    let mut raw = Vec::new();
    for token in &tokens {
        let TokenKind::Identifier(name) = &token.kind else { continue };
        let Some((ty, modifiers)) = upgrade_from_symbols(name, symbols) else { continue };
        raw.push((token.span.start, token.span.end, ty, modifiers));
    }

    encode(source, &raw)
}

/// Converts absolute byte spans into the LSP semantic-tokens wire format:
/// each entry is a delta from the previous token's *start* position
/// (line delta, then UTF-16 char delta — reset to an absolute column
/// whenever the line changes), plus a UTF-16 length, per the spec. An
/// identifier token never spans a newline, so length is always measured
/// within one line.
fn encode(source: &str, raw: &[(usize, usize, u32, u32)]) -> Vec<SemanticToken> {
    let lines: Vec<&str> = source.split('\n').collect();
    let mut line_starts = Vec::with_capacity(lines.len());
    let mut offset = 0usize;
    for line in &lines {
        line_starts.push(offset);
        offset += line.len() + 1;
    }

    let mut result = Vec::with_capacity(raw.len());
    let mut prev_line = 0u32;
    let mut prev_char = 0u32;

    for &(start, end, ty, modifiers) in raw {
        let line_index = line_starts.partition_point(|line_start| *line_start <= start).saturating_sub(1);
        let line_start = line_starts[line_index];
        let char_start = source[line_start..start].encode_utf16().count() as u32;
        let length = source[start..end].encode_utf16().count() as u32;
        let line = line_index as u32;

        let delta_line = line - prev_line;
        let delta_char = if delta_line == 0 { char_start - prev_char } else { char_start };

        result.push(SemanticToken {
            delta_line,
            delta_start: delta_char,
            length,
            token_type: ty,
            token_modifiers_bitset: modifiers,
        });

        prev_line = line;
        prev_char = char_start;
    }

    result
}

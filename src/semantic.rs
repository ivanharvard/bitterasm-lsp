//! Semantic-token syntax highlighting, computed straight from
//! `bitterasm::lexer::lex` on the live buffer text — so highlighting stays
//! correct on every keystroke even for code that doesn't parse or resolve
//! yet. When a fresh `analysis::AnalysisResult` is available for the same
//! file, `classify` additionally upgrades plain identifiers that match a
//! resolved top-level symbol (macro, type, const, ...) to a more specific
//! token type, the same way an IDE lights up a known function call
//! differently from an arbitrary name.

use bitterasm::lexer;
use bitterasm::resolver::{SymbolKind, SymbolTable};
use bitterasm::token::{Token, TokenKind};
use tower_lsp::lsp_types::{SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend};

pub const TOKEN_TYPES: &[SemanticTokenType] = &[
    SemanticTokenType::KEYWORD,
    SemanticTokenType::MACRO,
    SemanticTokenType::TYPE,
    SemanticTokenType::VARIABLE,
    SemanticTokenType::PARAMETER,
    SemanticTokenType::NUMBER,
    SemanticTokenType::STRING,
    SemanticTokenType::OPERATOR,
    SemanticTokenType::COMMENT,
    SemanticTokenType::DECORATOR,
    SemanticTokenType::EVENT,
];

const KEYWORD: u32 = 0;
const MACRO: u32 = 1;
const TYPE: u32 = 2;
const VARIABLE: u32 = 3;
#[allow(dead_code)]
const PARAMETER: u32 = 4;
const NUMBER: u32 = 5;
const STRING: u32 = 6;
const OPERATOR: u32 = 7;
const COMMENT: u32 = 8;
const DECORATOR: u32 = 9;
const EVENT: u32 = 10;

pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: TOKEN_TYPES.to_vec(),
        token_modifiers: vec![SemanticTokenModifier::READONLY, SemanticTokenModifier::DEFINITION],
    }
}

struct RawToken {
    start: usize,
    end: usize,
    ty: u32,
    modifiers: u32,
}

fn lexical_type(kind: &TokenKind) -> Option<u32> {
    use TokenKind::*;
    Some(match kind {
        From | Import | As | Pub | Macro | Type | Struct | Enum | Const => KEYWORD,
        Integer(_) => NUMBER,
        String(_) => STRING,
        Identifier(_) => VARIABLE,
        At => DECORATOR,
        Dollar | Backtick | Escaped(_) => OPERATOR,
        Dot | DotDot | DotDotEq | Ellipsis | Comma | Colon | Semicolon | Star | Plus | Minus
        | Slash | Percent | Equal | EqualEqual | Bang | BangEqual | Less | LessEqual | Greater
        | GreaterEqual | Ampersand | AndAnd | Pipe | OrOr | Caret | Tilde | ShiftLeft
        | ShiftRight | Arrow | FatArrow => OPERATOR,
        LParen | RParen | LBracket | RBracket | LBrace | RBrace | Newline | Eof => return None,
    })
}

/// Comments are skipped entirely by `lexer::lex` (they're not tokens), so
/// they're recovered here from the gaps between real tokens: any `#` found
/// in a gap starts a line comment running to the next newline, since a `#`
/// that's part of a string or identifier is already inside some token's own
/// span and therefore never appears in a gap.
fn comment_spans(source: &str, tokens: &[Token]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut cursor = 0usize;
    let scan_gap = |gap: &str, base: usize, spans: &mut Vec<(usize, usize)>| {
        if let Some(hash) = gap.find('#') {
            let start = base + hash;
            let end = gap[hash..].find('\n').map(|i| start + i).unwrap_or(base + gap.len());
            spans.push((start, end));
        }
    };
    for token in tokens {
        if token.span.start > cursor {
            scan_gap(&source[cursor..token.span.start], cursor, &mut spans);
        }
        cursor = cursor.max(token.span.end);
    }
    if cursor < source.len() {
        scan_gap(&source[cursor..], cursor, &mut spans);
    }
    spans
}

fn upgrade_from_symbols(name: &str, symbols: Option<&SymbolTable>) -> Option<(u32, u32)> {
    let symbols = symbols?;
    let symbol = symbols.get(symbols.lookup(name)?);
    Some(match symbol.kind {
        SymbolKind::Macro => (MACRO, 0),
        SymbolKind::Struct | SymbolKind::Enum | SymbolKind::TypeAlias => (TYPE, 0),
        SymbolKind::Const => (VARIABLE, 1 << 0), // readonly
        SymbolKind::Label => (EVENT, 0),
    })
}

pub fn tokenize(source: &str, symbols: Option<&SymbolTable>) -> Vec<SemanticToken> {
    let Ok(tokens) = lexer::lex(source) else { return Vec::new() };

    let mut raw: Vec<RawToken> = Vec::new();
    for token in &tokens {
        let Some(mut ty) = lexical_type(&token.kind) else { continue };
        let mut modifiers = 0u32;
        if let TokenKind::Identifier(name) = &token.kind {
            if let Some((upgraded, mods)) = upgrade_from_symbols(name, symbols) {
                ty = upgraded;
                modifiers = mods;
            }
        }
        raw.push(RawToken { start: token.span.start, end: token.span.end, ty, modifiers });
    }
    for (start, end) in comment_spans(source, &tokens) {
        raw.push(RawToken { start, end, ty: COMMENT, modifiers: 0 });
    }
    raw.sort_by_key(|t| t.start);

    encode(source, &raw)
}

/// Converts absolute byte spans into the LSP semantic-tokens wire format:
/// each entry is a delta from the previous token's *start* position
/// (line delta, then UTF-16 char delta — reset to an absolute column
/// whenever the line changes), plus a UTF-16 length, per the spec. Multi-
/// line tokens can't occur here (every kind above ends at or before its
/// line's newline), so length is always measured within one line.
fn encode(source: &str, raw: &[RawToken]) -> Vec<SemanticToken> {
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

    for token in raw {
        let line_index = line_starts.partition_point(|start| *start <= token.start).saturating_sub(1);
        let line_start = line_starts[line_index];
        let char_start = source[line_start..token.start].encode_utf16().count() as u32;
        let length = source[token.start..token.end].encode_utf16().count() as u32;
        let line = line_index as u32;

        let delta_line = line - prev_line;
        let delta_char = if delta_line == 0 { char_start - prev_char } else { char_start };

        result.push(SemanticToken {
            delta_line,
            delta_start: delta_char,
            length,
            token_type: token.ty,
            token_modifiers_bitset: token.modifiers,
        });

        prev_line = line;
        prev_char = char_start;
    }

    result
}

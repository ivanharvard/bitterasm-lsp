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
//! file successfully so far), there are no symbols to upgrade. That's
//! correct, not a fallback to guard against; the grammar alone is already a
//! complete, live view of a file that doesn't parse or resolve yet.
//!
//! The one other thing the grammar can't know is whether `source` or
//! `target` is an implicit name, the way `self` is in Rust. Inside a struct's
//! or type alias's facets the compiler binds `source` to the value itself,
//! and inside its `to`/`from` facets it binds `target` to the destination
//! type's const parameters. Everywhere else they're ordinary names: every
//! RISC-V branch takes a `target: int`. Telling the two apart needs to know
//! which declaration a facet belongs to, so it's done here from the tokens
//! alone (see `implicit_names`), without waiting for an analysis.

use bitterasm::lexer;
use bitterasm::resolver::SymbolKind;
use bitterasm::token::{Span, Token, TokenKind};

use crate::analysis::AnalysisResult;
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

const READONLY: u32 = 1 << 0;
const DEFAULT_LIBRARY: u32 = 1 << 1;

pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: TOKEN_TYPES.to_vec(),
        token_modifiers: vec![SemanticTokenModifier::READONLY, SemanticTokenModifier::DEFAULT_LIBRARY],
    }
}

fn upgrade_from_symbols(name: &str, result: &AnalysisResult) -> Option<(u32, u32)> {
    let symbol = result.symbols.as_ref()?.get(result.lookup(name)?);
    Some(match symbol.kind {
        SymbolKind::Macro => (MACRO, 0),
        SymbolKind::Struct | SymbolKind::Enum | SymbolKind::TypeAlias => (TYPE, 0),
        SymbolKind::Const => (VARIABLE, READONLY),
        SymbolKind::Label | SymbolKind::ExternLabel => (EVENT, 0),
    })
}

pub fn tokenize(source: &str, result: Option<&AnalysisResult>) -> Vec<SemanticToken> {
    let Ok(tokens) = lexer::lex(source) else { return Vec::new() };
    let result = result.filter(|result| result.symbols.is_some());
    let implicit = implicit_names(&tokens);

    let mut raw = Vec::new();
    for (name, span) in crate::analysis::names(&tokens) {
        // An implicit name wins over any same-named declaration: inside the
        // facet, `source` means the value being converted, not a const.
        let classified = if implicit.contains(&span) {
            Some((VARIABLE, READONLY | DEFAULT_LIBRARY))
        } else {
            result.and_then(|result| upgrade_from_symbols(&name, result))
        };
        let Some((ty, modifiers)) = classified else { continue };
        raw.push((span.start, span.end, ty, modifiers));
    }

    encode(source, &raw)
}

/// Spans of every `source` and `target` the compiler binds implicitly:
/// `source` anywhere in a struct's or type alias's facets, and `target` in
/// their `to`/`from` facets. Macro and enum facets bind neither.
///
/// A declaration's facets sit between its header and its body (a struct's
/// `{`) or the end of its statement (a type alias). Each starts with a `|`
/// outside any brackets, followed by the facet's name; a facet runs until
/// the next such `|`, or a line break outside brackets.
pub(crate) fn implicit_names(tokens: &[Token]) -> Vec<Span> {
    #[derive(PartialEq)]
    enum Owner {
        StructOrAlias,
        Other,
    }

    let mut spans = Vec::new();
    let mut owner: Option<Owner> = None;
    let mut facet: Option<&str> = None;
    // `(`/`[` nesting, plus `{` inside a facet (a struct literal). Before
    // the first facet, `<...>` counts too, so a generic parameter list
    // spanning lines doesn't end the header early.
    let mut depth = 0usize;

    let mut index = 0;
    while index < tokens.len() {
        let kind = &tokens[index].kind;
        let next = tokens.get(index + 1).map(|token| &token.kind);

        match kind {
            TokenKind::Struct | TokenKind::Type => {
                owner = Some(Owner::StructOrAlias);
                facet = None;
                depth = 0;
            }
            // `macro(int) -> int` is a type, not a declaration.
            TokenKind::Macro if next != Some(&TokenKind::LParen) => {
                owner = Some(Owner::Other);
                facet = None;
                depth = 0;
            }
            TokenKind::Enum => {
                owner = Some(Owner::Other);
                facet = None;
                depth = 0;
            }
            // A `const` statement has no facets. Inside brackets it's a
            // const generic parameter (`<const width: int>`) instead.
            TokenKind::Const if depth == 0 => {
                owner = None;
                facet = None;
            }
            _ if owner.is_none() => {}

            TokenKind::Pipe if depth == 0 => {
                facet = next.and_then(TokenKind::word_text).or(Some(""));
                // Skip the facet's name so `from` isn't read as anything else.
                index += 1;
            }
            TokenKind::LParen | TokenKind::LBracket => depth += 1,
            TokenKind::Less if facet.is_none() => depth += 1,
            TokenKind::RParen | TokenKind::RBracket => depth = depth.saturating_sub(1),
            TokenKind::Greater if facet.is_none() => depth = depth.saturating_sub(1),

            // A `{` opening a line, or right after the header, starts the
            // body. Inside a facet's expression it's a struct literal.
            TokenKind::LBrace => {
                let opens_line = index == 0 || tokens[index - 1].kind == TokenKind::Newline;
                if facet.is_some() && depth == 0 && !opens_line || depth > 0 {
                    depth += 1;
                } else {
                    owner = None;
                    facet = None;
                }
            }
            TokenKind::RBrace => depth = depth.saturating_sub(1),

            TokenKind::Newline if depth == 0 => {
                facet = None;
                let following = tokens[index..].iter().find(|token| token.kind != TokenKind::Newline);
                match following.map(|token| &token.kind) {
                    Some(TokenKind::Pipe) | Some(TokenKind::LBrace) => {}
                    _ => owner = None,
                }
            }

            TokenKind::Identifier(name) if owner == Some(Owner::StructOrAlias) => {
                let implicit = match (name.as_str(), facet) {
                    ("source", Some(_)) => true,
                    ("target", Some("to" | "from")) => true,
                    _ => false,
                };
                if implicit {
                    spans.push(tokens[index].span);
                }
            }

            _ => {}
        }

        index += 1;
    }

    spans
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

#[cfg(test)]
mod tests {
    use super::implicit_names;
    use bitterasm::lexer;

    /// The text of every implicit `source`/`target`, with the line it's on.
    fn implicit(source: &str) -> Vec<(usize, String)> {
        let tokens = lexer::lex(source).unwrap();
        implicit_names(&tokens)
            .into_iter()
            .map(|span| (source[..span.start].matches('\n').count() + 1, source[span.start..span.end].to_string()))
            .collect()
    }

    fn named(pairs: &[(usize, &str)]) -> Vec<(usize, String)> {
        pairs.iter().map(|(line, name)| (*line, name.to_string())).collect()
    }

    #[test]
    fn struct_conversion_facets_bind_source_and_target() {
        let source = "pub struct signed<const width: int>\n    | from signed_from_int(source, target.width)\n    | to signed_to_int(source)\n{\n    pub bits: int\n}\n";
        assert_eq!(implicit(source), named(&[(2, "source"), (2, "target"), (3, "source")]));
    }

    #[test]
    fn invariants_bind_source_but_not_target() {
        let source = "pub struct Pair\n    | invariant check(source, target)\n{\n    a: int\n}\n";
        assert_eq!(implicit(source), named(&[(2, "source")]));
    }

    #[test]
    fn type_alias_facets_bind_source() {
        let source = "pub type Cents = int\n    | to cents_to_int(source)\nconst source = 1\n";
        assert_eq!(implicit(source), named(&[(2, "source")]));
    }

    #[test]
    fn several_facets_on_one_line() {
        let source = "pub type Small = int | invariant source > 0 | to shrink(source, target.n)\n";
        assert_eq!(implicit(source), named(&[(1, "source"), (1, "source"), (1, "target")]));
    }

    #[test]
    fn a_bitwise_or_inside_parentheses_is_not_a_facet() {
        let source = "pub type Flags = int\n    | invariant (source | 1) > 0\n";
        assert_eq!(implicit(source), named(&[(2, "source")]));
    }

    #[test]
    fn ordinary_parameters_named_source_or_target_are_not_implicit() {
        let source = "macro beq(rs1: int, rs2: int, target: int)\n    | before check(target)\n{\n    @emit target\n}\n\nmacro copy(source: int) -> int {\n    @return source\n}\n";
        assert!(implicit(source).is_empty());
    }

    #[test]
    fn struct_bodies_and_later_statements_are_not_facets() {
        let source = "pub struct Wrap\n    | to unwrap(source)\n{\n    source: int\n}\n\nmacro use_it(target: int) {\n    @emit target\n}\n";
        assert_eq!(implicit(source), named(&[(2, "source")]));
    }

    #[test]
    fn a_macro_type_in_an_alias_does_not_end_its_facets() {
        let source = "pub type Handler = macro(int) -> int\n    | to call_it(source)\n";
        assert_eq!(implicit(source), named(&[(2, "source")]));
    }

    #[test]
    fn a_generic_header_spanning_lines_keeps_its_facets() {
        let source = "pub struct Fixed<\n    const scale: int\n>\n    | from int_to_fixed(source, target.scale)\n{\n    raw: int\n}\n";
        assert_eq!(implicit(source), named(&[(4, "source"), (4, "target")]));
    }
}

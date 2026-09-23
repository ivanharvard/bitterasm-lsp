//! Not run by default — it needs a sibling checkout of
//! https://github.com/ivanharvard/bitterasm to analyze real `.basm` files
//! against, at a path this crate doesn't control. Point `BITTERASM_CHECKOUT`
//! at one and run with `cargo test --test smoke -- --ignored --nocapture`.

use std::path::PathBuf;

#[path = "../src/analysis.rs"]
mod analysis;
#[path = "../src/semantic.rs"]
mod semantic;

#[test]
#[ignore]
fn reg_definition_is_found() {
    let checkout: PathBuf = std::env::var("BITTERASM_CHECKOUT")
        .expect("set BITTERASM_CHECKOUT to a bitterasm checkout to run this test")
        .into();

    // Non-relative `from std.binary import *` resolves against the
    // process's CWD (see bitterasm's loader::module_base_dir), same as
    // running `bitterasm compile` from the project root — so the LSP
    // server needs its CWD set to the workspace root by its client.
    std::env::set_current_dir(&checkout).unwrap();
    let path = checkout.join("std/riscv/impl.basm");
    let text = std::fs::read_to_string(&path).unwrap();
    let result = analysis::analyze_file(&path, Some(&text));

    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert!(result.symbols.is_some());

    let idx = text.find("Reg(i)").unwrap();
    let offset = idx + 1; // inside "Reg"
    let name = analysis::identifier_at(&text, offset);
    assert_eq!(name.as_deref(), Some("Reg"));

    let def = analysis::find_definition(&result, "Reg");
    assert!(def.is_some(), "expected `Reg`'s declaration to be found");
    assert_eq!(def.unwrap().0, path);
}

/// `std.array`'s `mapped<T, S, F: Fn(T) -> S>(arr: Array<T, ...>, f: F)` —
/// a generic type parameter bound by `Fn(...)`, the newest bit of grammar
/// this server has to parse and resolve through. Not a dedicated fixture:
/// `std/array.basm` already carries it in real, checked-in code.
#[test]
#[ignore]
fn generic_fn_bound_parses_and_resolves_cleanly() {
    let checkout: PathBuf = std::env::var("BITTERASM_CHECKOUT")
        .expect("set BITTERASM_CHECKOUT to a bitterasm checkout to run this test")
        .into();

    std::env::set_current_dir(&checkout).unwrap();
    let path = checkout.join("std/array.basm");
    let text = std::fs::read_to_string(&path).unwrap();
    let result = analysis::analyze_file(&path, Some(&text));

    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert!(result.symbols.is_some());

    // An ordinary type reference elsewhere in `mapped`'s own signature
    // still resolves normally — the new bound syntax shouldn't have
    // knocked anything else in the same declaration out of alignment.
    let idx = text.find("Array<T, ...>").unwrap();
    let offset = idx + 1; // inside "Array"
    let name = analysis::identifier_at(&text, offset);
    assert_eq!(name.as_deref(), Some("Array"));

    let def = analysis::find_definition(&result, "Array");
    assert!(def.is_some(), "expected `Array`'s declaration to be found");
    assert_eq!(def.unwrap().0, path);
}

/// `std/riscv/impl.basm`'s instruction macros declare `| emits
/// LittleEndian<...>` rather than a `-> Type` return annotation — an
/// `@emit`-only macro was never actually checked against `-> T`, so the
/// facet replaced it. The newest bit of grammar this server has to parse
/// and resolve through. Not a dedicated fixture: `std/riscv/impl.basm`
/// already carries it in real, checked-in code.
#[test]
#[ignore]
fn emits_facet_parses_and_resolves_cleanly() {
    let checkout: PathBuf = std::env::var("BITTERASM_CHECKOUT")
        .expect("set BITTERASM_CHECKOUT to a bitterasm checkout to run this test")
        .into();

    std::env::set_current_dir(&checkout).unwrap();
    let path = checkout.join("std/riscv/impl.basm");
    let text = std::fs::read_to_string(&path).unwrap();
    let result = analysis::analyze_file(&path, Some(&text));

    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert!(result.symbols.is_some());

    // The type an `emits` facet names still resolves normally — same
    // `LittleEndian` a `-> Type` annotation elsewhere in this file would
    // have named, just declared through the new facet instead.
    let idx = text.find("emits LittleEndian<IType, 32>").unwrap();
    let offset = idx + "emits ".len() + 1; // inside "LittleEndian"
    let name = analysis::identifier_at(&text, offset);
    assert_eq!(name.as_deref(), Some("LittleEndian"));

    let def = analysis::find_definition(&result, "LittleEndian");
    assert!(def.is_some(), "expected `LittleEndian`'s declaration to be found");
}

/// Sections, public labels, the `leaks_section` facet, and imported public
/// labels arrived together as the source-level pieces of multi-file linking.
/// Analyze a small real pair of files so this catches drift in both parsing
/// and the external-label symbol kind used by semantic highlighting.
#[test]
#[ignore]
fn sections_and_external_labels_parse_resolve_and_highlight() {
    let checkout: PathBuf = std::env::var("BITTERASM_CHECKOUT")
        .expect("set BITTERASM_CHECKOUT to a bitterasm checkout to run this test")
        .into();

    std::env::set_current_dir(&checkout).unwrap();
    let path = checkout.join("tests/fixtures/emit/extern_label_importer.basm");
    let text = std::fs::read_to_string(&path).unwrap();
    let result = analysis::analyze_file(&path, Some(&text));

    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    let symbols = result.symbols.as_ref().expect("expected a resolved symbol table");
    let target = symbols.get(symbols.lookup("target").expect("expected imported public label"));
    assert_eq!(target.kind, bitterasm::resolver::SymbolKind::ExternLabel);

    let definition = analysis::find_definition(&result, "target")
        .expect("expected the imported label's definition");
    assert_eq!(
        definition.0,
        checkout
            .join("tests/fixtures/emit/extern_label_dep.basm")
            .canonicalize()
            .unwrap()
    );

    let semantic_tokens = semantic::tokenize(&text, Some(symbols));
    assert!(
        semantic_tokens.iter().any(|token| token.token_type == 3),
        "the imported external label should receive semantic highlighting"
    );

    // Keep the source-only additions covered too; this sample is deliberately
    // local because the checked-in linking fixture does not need a section-
    // leaking macro itself.
    let source = "section .text\npub entry:\nmacro switch() | leaks_section {\n    section .data\n}\n";
    let tokens = bitterasm::lexer::lex(source).expect("new syntax should lex");
    bitterasm::parser::parse(tokens).expect("new syntax should parse");
}

/// Dotted label names (`.loop:`, `jmp .skip`) are one name to the LSP even
/// though the compiler lexes them as a `.` plus a word — including a word
/// that's a keyword on its own (`skip`).
#[test]
#[ignore]
fn dotted_labels_resolve_and_highlight() {
    let checkout: PathBuf = std::env::var("BITTERASM_CHECKOUT")
        .expect("set BITTERASM_CHECKOUT to a bitterasm checkout to run this test")
        .into();

    std::env::set_current_dir(&checkout).unwrap();
    let dir = std::env::temp_dir().join(format!("bitterasm-lsp-dotted-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("dotted.basm");
    let text = "jmp_to .skip\n.skip:\nmacro jmp_to(target: int) {\n    @emit target\n}\n";
    std::fs::write(&path, text).unwrap();

    let result = analysis::analyze_file(&path, Some(text));
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);

    let offset = text.find(".skip").unwrap() + 3;
    let name = analysis::identifier_at(text, offset);
    assert_eq!(name.as_deref(), Some(".skip"));

    let definition = analysis::find_definition(&result, ".skip")
        .expect("expected the dotted label's definition");
    assert_eq!(&text[definition.1.start..definition.1.start + 6], ".skip:");

    let symbols = result.symbols.as_ref().expect("expected a resolved symbol table");
    let semantic_tokens = semantic::tokenize(text, Some(symbols));
    assert!(
        semantic_tokens.iter().any(|token| token.token_type == 3 && token.length == 5),
        "the dotted label should be highlighted as one 5-char token"
    );

    // member access stays member access
    let tokens = bitterasm::lexer::lex("x str.len").unwrap();
    let names: Vec<String> = analysis::names(&tokens).into_iter().map(|(name, _)| name).collect();
    assert_eq!(names, vec!["x", "str", "len"]);

    std::fs::remove_dir_all(&dir).ok();
}

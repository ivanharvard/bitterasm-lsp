//! Not run by default — it needs a sibling checkout of
//! https://github.com/ivanharvard/bitterasm to analyze real `.basm` files
//! against, at a path this crate doesn't control. Point `BITTERASM_CHECKOUT`
//! at one and run with `cargo test --test smoke -- --ignored --nocapture`.

use std::path::PathBuf;

// The server uses parts of these that the tests don't.
#[allow(dead_code)]
#[path = "../src/analysis.rs"]
mod analysis;
#[allow(dead_code)]
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

/// `std.array`'s `mapped<T, U>(arr: Array<T, ...>, f: macro(T) -> U)` —
/// a macro type with generics inside it, the newest bit of grammar
/// this server has to parse and resolve through. Not a dedicated fixture:
/// `std/array.basm` already carries it in real, checked-in code.
#[test]
#[ignore]
fn generic_macro_type_parses_and_resolves_cleanly() {
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
    let target = symbols.get(result.lookup("target").expect("expected imported public label"));
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

    let semantic_tokens = semantic::tokenize(&text, Some(&result));
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

    assert!(result.symbols.is_some(), "expected a resolved symbol table");
    let semantic_tokens = semantic::tokenize(text, Some(&result));
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

/// `pub from .impl import *` re-exports: a file importing only
/// `std.x86_64.nasm` still names `impl.basm`'s `Reg` and `mov`, which only
/// reach it through two re-exports (`nasm` → `intel` → `impl`). Both must
/// jump to their real declarations and get symbol highlighting, and a name
/// that's only imported *inside* a dependency (`std.binary`'s `bits`, which
/// `impl.basm` imports privately) must not resolve here at all.
#[test]
#[ignore]
fn names_reached_through_pub_from_resolve_and_highlight() {
    let checkout: PathBuf = std::env::var("BITTERASM_CHECKOUT")
        .expect("set BITTERASM_CHECKOUT to a bitterasm checkout to run this test")
        .into();

    std::env::set_current_dir(&checkout).unwrap();
    let dir = std::env::temp_dir().join(format!("bitterasm-lsp-pub-from-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("reexports.basm");
    let text = "from std.x86_64.nasm import *\n\nmacro zero(r: Reg) {\n    mov r, 0\n}\n\nzero rax\n";
    std::fs::write(&path, text).unwrap();

    let result = analysis::analyze_file(&path, Some(text));
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);

    let impl_path = checkout.join("std/x86_64/impl.basm").canonicalize().unwrap();
    for name in ["Reg", "mov"] {
        let definition = analysis::find_definition(&result, name)
            .unwrap_or_else(|| panic!("expected `{name}` to resolve through the re-exports"));
        assert_eq!(definition.0.canonicalize().unwrap(), impl_path, "`{name}`");
    }

    assert!(result.lookup("bits").is_none(), "`bits` is only imported inside impl.basm");

    let semantic_tokens = semantic::tokenize(text, Some(&result));
    let reg_line = text[..text.find("Reg").unwrap()].matches('\n').count() as u32;
    let mut line = 0;
    let highlighted_reg = semantic_tokens.iter().any(|token| {
        line += token.delta_line;
        line == reg_line && token.token_type == 1
    });
    assert!(highlighted_reg, "`Reg` should be highlighted as a type");

    std::fs::remove_dir_all(&dir).ok();
}

/// Every `source`/`target` the highlighter marks as implicit across `std`
/// should sit on a facet line, and plain parameters with those names (every
/// RISC-V branch's `target: int`) shouldn't be marked at all.
#[test]
#[ignore]
fn implicit_names_in_std_are_only_in_facets() {
    let checkout: PathBuf = std::env::var("BITTERASM_CHECKOUT")
        .expect("set BITTERASM_CHECKOUT to a bitterasm checkout to run this test")
        .into();

    let mut files = Vec::new();
    let mut dirs = vec![checkout.join("std")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|ext| ext == "basm") {
                files.push(path);
            }
        }
    }

    let mut found = 0;
    for path in files {
        let text = std::fs::read_to_string(&path).unwrap();
        let Ok(tokens) = bitterasm::lexer::lex(&text) else { continue };
        for span in semantic::implicit_names(&tokens) {
            let line_start = text[..span.start].rfind('\n').map_or(0, |at| at + 1);
            let line_end = text[span.start..].find('\n').map_or(text.len(), |at| span.start + at);
            let line = &text[line_start..line_end];
            println!("{}: {}", path.display(), line.trim());
            assert!(line.contains('|'), "{}: `{}` isn't on a facet line: {line}", path.display(), &text[span.start..span.end]);
            found += 1;
        }
    }

    assert!(found > 0, "expected some implicit names in std");
}

/// Spaces, sections in them shared as consts from another file, and facets
/// written as macros: all resolve, and go-to-definition finds a section's
/// const in the file it came from.
#[test]
#[ignore]
fn spaces_sections_and_facets_resolve_and_highlight() {
    let checkout: PathBuf = std::env::var("BITTERASM_CHECKOUT")
        .expect("set BITTERASM_CHECKOUT to a bitterasm checkout to run this test")
        .into();

    std::env::set_current_dir(&checkout).unwrap();
    let path = checkout.join("tests/fixtures/spaces/imported_sections.basm");
    let text = std::fs::read_to_string(&path).unwrap();
    let result = analysis::analyze_file(&path, Some(&text));
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);

    // `VECTORS`, and the section `.vectors` it holds, are declared in the
    // library they're imported from.
    let library = checkout.join("tests/fixtures/spaces/section_lib.basm").canonicalize().unwrap();
    let library_text = std::fs::read_to_string(&library).unwrap();
    for name in ["VECTORS", ".vectors"] {
        let (file, span) = analysis::find_definition(&result, name).unwrap_or_else(|| panic!("expected `{name}`'s declaration"));
        assert_eq!(file.canonicalize().unwrap(), library);
        let found = &library_text[span.start..span.end];
        assert!(found.contains("VECTORS = section .vectors in rom"), "`{name}` found {found:?}");
    }

    // So is the space it's in.
    let (file, _) = analysis::find_definition(&result, "rom").expect("expected `rom`'s declaration");
    assert_eq!(file.canonicalize().unwrap(), library);

    // A space's facets are macros, found like any other.
    let path = checkout.join("tests/fixtures/spaces/facets.basm");
    let text = std::fs::read_to_string(&path).unwrap();
    let result = analysis::analyze_file(&path, Some(&text));
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    let (file, _) = analysis::find_definition(&result, "start").expect("expected `start`'s declaration");
    assert!(file.ends_with("std/spaces.basm"), "{file:?}");

    // `stream`, the default space, is highlighted as a built-in.
    let tokens = semantic::tokenize(&text, Some(&result));
    let stream = text.find("stream").unwrap();
    let mut line = 0;
    let mut column = 0;
    let found = tokens.iter().any(|token| {
        line += token.delta_line;
        column = if token.delta_line == 0 { column + token.delta_start } else { token.delta_start };
        let offset = text.lines().take(line as usize).map(|l| l.len() + 1).sum::<usize>() + column as usize;
        offset == stream && token.token_modifiers_bitset == 0b11
    });
    assert!(found, "expected `stream` to be highlighted as a built-in");
}

/// A file that fails to parse, or imports one that does, still gets a
/// symbol table from the statements that parsed: go-to-definition keeps
/// working, and every parse error is shown, not just the first.
#[test]
#[ignore]
fn definitions_survive_parse_errors() {
    let checkout: PathBuf = std::env::var("BITTERASM_CHECKOUT")
        .expect("set BITTERASM_CHECKOUT to a bitterasm checkout to run this test")
        .into();

    std::env::set_current_dir(&checkout).unwrap();
    let dir = std::env::temp_dir().join(format!("bitterasm-lsp-recovery-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("lib.basm"),
        "const broken = )\n\npub macro helper(x: int) {\n    @emit x\n}\n",
    )
    .unwrap();
    let path = dir.join("main.basm");
    let text = "from .lib import helper\n\nmacro show(x: int) {\n    helper x +\n    helper x\n}\n\nconst also_broken = (1\nshow 1\n";
    std::fs::write(&path, text).unwrap();

    let result = analysis::analyze_file(&path, Some(text));
    assert_eq!(result.diagnostics.len(), 3, "{:#?}", result.diagnostics);
    assert!(result.symbols.is_some(), "expected a symbol table despite the errors");

    let (file, span) = analysis::find_definition(&result, "show").expect("expected `show`'s declaration");
    assert_eq!(file, path);
    assert!(text[span.start..].starts_with("macro show"));

    let (file, _) = analysis::find_definition(&result, "helper").expect("expected `helper`'s declaration");
    assert_eq!(file, dir.join("lib.basm").canonicalize().unwrap());

    std::fs::remove_dir_all(&dir).ok();
}

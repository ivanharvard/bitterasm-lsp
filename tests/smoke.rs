//! Not run by default — it needs a sibling checkout of
//! https://github.com/ivanharvard/bitterasm to analyze real `.basm` files
//! against, at a path this crate doesn't control. Point `BITTERASM_CHECKOUT`
//! at one and run with `cargo test --test smoke -- --ignored --nocapture`.

use std::path::PathBuf;

#[path = "../src/analysis.rs"]
mod analysis;

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

# bitterasm-lsp

A language server for [BitterASM](https://github.com/ivanharvard/bitterasm), plus a
VS Code extension that runs it. Standard LSP over stdio, so it isn't VS Code-specific —
any LSP-capable editor can point at the same binary.

It reuses BitterASM's own lexer, loader, and resolver as a library (not a separate
grammar someone has to keep in sync by hand), so highlighting and navigation reflect
exactly what the compiler itself sees.

## Features

- **Syntax highlighting** via LSP semantic tokens, computed live from
  `bitterasm::lexer::lex` on every keystroke — correct even for code that doesn't parse.
  When the file last resolved cleanly, plain identifiers that match a top-level macro,
  type, struct, enum, or const are additionally colored by that specific kind, the way
  an IDE distinguishes a known function call from an arbitrary name.
- **Go to definition** for macros, types, structs, enums, and consts, backed by the
  real resolver's `SymbolTable` — not a text search. Works across `from x.y import *`
  imports, since the loader's whole flattened import graph is resolved.
- **Diagnostics** — lexer, parser, loader, and resolver errors, plus the compiler's
  lint set (`unused_import`, `unreachable_code`, etc.), reported the moment you open,
  edit, or save a file.
- **Hover** — shows a symbol's kind and name.

## Known limitations

- **Go to definition only covers top-level declarations.** Local bindings — macro
  parameters, `match` arms — aren't in the whole-program symbol table BitterASM's own
  resolver builds, so jumping to those isn't supported.
- **Diagnostics and go-to-definition re-analyze from disk** on open/change/save; only
  the file you're actively editing is read from its live buffer instead (an overlay),
  because BitterASM's loader doesn't support a general in-memory filesystem. Unsaved
  edits to a file *other* than the one you're editing won't be reflected until you save
  it.
- **No incremental re-analysis or debouncing.** Every keystroke that touches
  diagnostics/go-to-definition re-runs the whole load → resolve pipeline for the file.
  Fine for anything BitterASM-sized today; would need real work for very large programs.
- **Module resolution depends on the server's working directory.** BitterASM resolves
  a non-relative `from x.y import *` against the process's current directory — the same
  way `bitterasm compile` only finds `std/...` when run from the project root. The VS
  Code extension sets the server's cwd to your first workspace folder for exactly this
  reason; wiring up a different editor means doing the same.

## Building the server

```sh
cargo build --release
# binary at target/release/bitterasm-lsp
```

`Cargo.toml` depends on `bitterasm` as a pinned git dependency (a specific `rev`, not
a branch), so this repo tracks a known-good commit of the compiler rather than
whatever `main` happens to be. Bump the `rev` in `Cargo.toml` deliberately when you
want a newer compiler.

## VS Code extension

```sh
cd editors/vscode
npm install
npm run compile
```

Then either:
- **Dev loop:** open `editors/vscode/` in VS Code and press F5 (Run Extension) — the
  extension activates for `.basm` files in the new window.
- **Install locally:** `npx vsce package` to produce a `.vsix`, then
  `code --install-extension bitterasm-vscode-*.vsix`.

By default the extension looks for `bitterasm-lsp` on `PATH`. Point it at a specific
binary with the `bitterasm-lsp.serverPath` setting if it's not installed globally —
e.g. `target/release/bitterasm-lsp` from a `cargo build --release` in this repo.

## Other editors

Any LSP client works the same way: run `bitterasm-lsp` (stdio transport) with its
working directory set to the BitterASM project root, for `.basm` files. No special
initialization options are required.

## Layout

```
src/            the language server (Rust, tower-lsp)
tests/smoke.rs  an ignored integration test against a real bitterasm checkout —
                see the file for how to run it
editors/vscode/ the VS Code client extension (TypeScript)
```

#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Same install root bitterasm's own install.sh uses, so bitterasm-lsp lands
# on the same PATH entry as bitterasm/bitter instead of needing a second one.
install_root="$HOME/.bitterasm"
bin_dir="$install_root/bin"
vscode_dir="$repo_root/editors/vscode"

ask() {
    local prompt="$1"
    local reply

    read -r -p "$prompt [Y/n] " reply

    case "$reply" in
        [nN]*) return 1 ;;
        *) return 0 ;;
    esac
}

install_server=false
install_vscode=false

if ask "Install the bitterasm-lsp language server?"; then
    install_server=true
fi

if command -v code >/dev/null 2>&1; then
    if ask "Install the BitterASM VS Code extension?"; then
        install_vscode=true
    fi
elif [ -d "$vscode_dir" ]; then
    echo "VS Code ('code' on PATH) not found — skipping the extension prompt."
    echo "You can still build a .vsix by hand later; see editors/vscode/README or the repo README."
fi

if [ "$install_server" = true ]; then
    echo "Building bitterasm-lsp (release)..."
    cargo build --release --manifest-path "$repo_root/Cargo.toml"

    mkdir -p "$bin_dir"
    staged_server="$bin_dir/.bitterasm-lsp.new"
    install -m 755 "$repo_root/target/release/bitterasm-lsp" "$staged_server"
    mv -f "$staged_server" "$bin_dir/bitterasm-lsp"
    echo "  installed $bin_dir/bitterasm-lsp"
fi

if [ "$install_vscode" = true ]; then
    echo "Building the VS Code extension..."
    ( cd "$vscode_dir" && npm install && npm run compile && npx vsce package -o bitterasm-vscode.vsix )

    vsix="$vscode_dir/bitterasm-vscode.vsix"
    echo "Installing $vsix into VS Code..."
    code --install-extension "$vsix"
    echo "  installed the BitterASM VS Code extension"
fi

echo
echo "Done."

if [ "$install_server" = true ]; then
    case ":$PATH:" in
        *":$bin_dir:"*) ;;
        *)
            echo
            echo "$bin_dir isn't on your PATH yet. Add it with:"
            echo
            echo "    export PATH=\"$bin_dir:\$PATH\""
            if [ "$install_vscode" = true ]; then
                echo
                echo "A GUI-launched VS Code often won't pick that up even after a shell"
                echo "restart. If the extension can't find the server, set it explicitly"
                echo "instead, in VS Code's settings.json:"
                echo
                echo "    \"bitterasm-lsp.serverPath\": \"$bin_dir/bitterasm-lsp\""
            fi
            ;;
    esac
fi

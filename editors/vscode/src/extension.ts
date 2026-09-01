import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

export function activate(context: vscode.ExtensionContext) {
  const config = vscode.workspace.getConfiguration("bitterasm-lsp");
  const command = config.get<string>("serverPath", "bitterasm-lsp");

  // BitterASM's own module resolution (`from x.y import *` with no leading
  // dots) resolves against the compiler process's current directory, the
  // same way `bitterasm compile` only finds `std/...` when run from the
  // project root — so the server needs to be spawned with the workspace
  // folder as its cwd, not whatever directory happens to own the editor
  // process, or imports fail to resolve for every open file.
  const workspaceFolder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;

  const serverOptions: ServerOptions = {
    command,
    args: [],
    options: workspaceFolder ? { cwd: workspaceFolder } : undefined,
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "basm" }],
  };

  client = new LanguageClient(
    "bitterasm-lsp",
    "BitterASM Language Server",
    serverOptions,
    clientOptions
  );

  client.start().then(
    () => undefined,
    (error: unknown) => {
      vscode.window.showErrorMessage(
        `Failed to start bitterasm-lsp (looked for "${command}" on PATH). ` +
          `Set "bitterasm-lsp.serverPath" if it's installed elsewhere. ${error}`
      );
    }
  );

  context.subscriptions.push({
    dispose: () => {
      void client?.stop();
    },
  });
}

export function deactivate(): Thenable<void> | undefined {
  return client?.stop();
}

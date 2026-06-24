// Minimal F5 dev loop. Spawns the locally-built LSP binary from the repo's
// target/release, resolved relative to this extension folder (extensionPath
// is <repo>/editors/vscode, so ../../target/release). Rebuild the binary
// (cargo build --release --bin roundhouse-lsp), reload the Extension
// Development Host, and you're testing the fresh build. No copy, no symlink,
// no discovery — this is for local F5 use only.

const path = require('path');
const { LanguageClient } = require('vscode-languageclient/node');

let client;

function activate(context) {
  const server = path.join(
    context.extensionPath, '..', '..', 'target', 'release', 'roundhouse-lsp'
  );
  client = new LanguageClient(
    'roundhouse',
    'Roundhouse LSP',
    { command: server, args: [] },
    { documentSelector: [{ scheme: 'file', language: 'ruby' }] }
  );
  return client.start();
}

function deactivate() {
  return client ? client.stop() : undefined;
}

module.exports = { activate, deactivate };

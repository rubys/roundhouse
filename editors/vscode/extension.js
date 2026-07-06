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
    {
      // Templates too: the analyzer's view spans point into the
      // template files, so hover/completion/diagnostics work inside
      // ERB and HAML buffers exactly as in Ruby ones. `erb`/`haml`
      // are contributed by the common Ruby/HAML extensions; the
      // pattern selectors catch the files when no such extension has
      // claimed a language id.
      documentSelector: [
        { scheme: 'file', language: 'ruby' },
        { scheme: 'file', language: 'erb' },
        { scheme: 'file', language: 'haml' },
        { scheme: 'file', pattern: '**/*.html.erb' },
        { scheme: 'file', pattern: '**/*.html.haml' },
        { scheme: 'file', pattern: '**/*.json.jbuilder' },
      ],
    }
  );
  return client.start();
}

function deactivate() {
  return client ? client.stop() : undefined;
}

module.exports = { activate, deactivate };

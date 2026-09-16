// Client for the roundhouse Language Server. The server is found, in
// order: the `roundhouse.serverPath` setting; a `roundhouse` binary on
// PATH (run as `roundhouse lsp` — the multi-call binary the release
// tarball installs); the repo's own target/release/roundhouse-lsp,
// resolved relative to this extension folder (extensionPath is
// <repo>/editors/vscode, so ../../target/release) for the F5 dev loop.
// Rebuild the binary (cargo build --release --bin roundhouse-lsp), reload
// the Extension Development Host, and you're testing the fresh build.

const fs = require('fs');
const path = require('path');
const vscode = require('vscode');
const { LanguageClient } = require('vscode-languageclient/node');

let client;

// The first directory on PATH holding an executable `roundhouse`
// (`roundhouse.exe` on Windows), or null.
function onPath(name) {
  const exts = process.platform === 'win32' ? ['.exe', ''] : [''];
  for (const dir of (process.env.PATH || '').split(path.delimiter)) {
    if (!dir) continue;
    for (const ext of exts) {
      const candidate = path.join(dir, name + ext);
      try {
        fs.accessSync(candidate, fs.constants.X_OK);
        return candidate;
      } catch (_) { /* not here */ }
    }
  }
  return null;
}

function serverOptions(context) {
  const configured = vscode.workspace.getConfiguration('roundhouse').get('serverPath');
  if (configured) {
    // A bare `roundhouse` (or anything not ending in -lsp) is the
    // multi-call binary and needs the subcommand.
    const args = path.basename(configured).startsWith('roundhouse-lsp') ? [] : ['lsp'];
    return { command: configured, args };
  }
  const installed = onPath('roundhouse');
  if (installed) return { command: installed, args: ['lsp'] };
  const dev = path.join(
    context.extensionPath, '..', '..', 'target', 'release', 'roundhouse-lsp'
  );
  return { command: dev, args: [] };
}

function activate(context) {
  client = new LanguageClient(
    'roundhouse',
    'Roundhouse LSP',
    serverOptions(context),
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
      // The publish gate: errors and N+1 findings always; the warning
      // ledger only when `roundhouse.warnings` is on. Sent at startup
      // and again (as workspace/didChangeConfiguration) on change.
      initializationOptions: {
        warnings: vscode.workspace.getConfiguration('roundhouse').get('warnings', false),
      },
      synchronize: { configurationSection: 'roundhouse' },
    }
  );
  return client.start();
}

function deactivate() {
  return client ? client.stop() : undefined;
}

module.exports = { activate, deactivate };

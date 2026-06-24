# Roundhouse LSP — VS Code dev client

A minimal client for exercising [`roundhouse-lsp`](../../src/bin/roundhouse-lsp.rs)
under VS Code's F5 dev loop. It spawns the locally-built binary and attaches
it to Ruby files, so you get inferred-type hovers, inlay hints, nil-safety
diagnostics, find-references, and go-to-definition over a whole Rails app.

This is **for local development only** — there is no packaging, no binary
discovery, and no cross-platform support here. It points straight at
`../../target/release/roundhouse-lsp`. Distributing the LSP to other people
(a packaged `.vsix` bundling a native binary, or a WASM build) is a separate
concern and deliberately out of scope for this folder.

## One-time setup

From the repo root, build the server:

```sh
cargo build --release --bin roundhouse-lsp
```

Then install the one client dependency (`vscode-languageclient`):

```sh
cd editors/vscode && npm install
```

## Run it (the F5 loop)

1. Open **this folder** (`editors/vscode`) in VS Code.
2. Press **F5** (or run the "Run Roundhouse LSP" launch config). A second
   window — the **Extension Development Host** — opens with `fixtures/real-blog`
   already loaded as the workspace.
3. In that window, open e.g. `app/controllers/articles_controller.rb` and
   hover over `@article` → ` Article `.

Everything runs in the Extension Development Host window; the window you pressed
F5 in is just the launcher/debugger.

## Iterate

After changing the Rust:

```sh
cargo build --release --bin roundhouse-lsp
```

Then press **Cmd+R** in the Extension Development Host window. That reloads the
extension, which respawns the binary — so you're testing the fresh build. No
copy, no repackage. (Editing `extension.js` is the same: Cmd+R picks it up.)

## Tips

- **Test a different Rails app:** in the dev-host window, File → Open Folder.
  The LSP's workspace root follows whatever folder is open.
- **Faster rebuilds:** switch `'release'` → `'debug'` in `extension.js` and
  build with `cargo build --bin roundhouse-lsp` (quicker compile, slower binary).
- **Hide the inline `: Type` annotations** (inlay hints) without losing hover —
  add to your `settings.json`:
  ```json
  "[ruby]": { "editor.inlayHints.enabled": "off" }
  ```
  (Scoping to `[ruby]` leaves inlay hints on for other languages.)
- **Server errors:** the dev host's Output panel → "Roundhouse LSP" channel
  shows the server's stderr.
- `.rb` files get the `ruby` language id from VS Code's built-in Ruby grammar,
  so no other extension is required for the client to attach.

## Files

- `extension.js` — spawns the binary, attaches to `ruby` documents.
- `package.json` — activation (`onLanguage:ruby`) and the client dependency.
- `.vscode/launch.json` — the F5 config; opens `fixtures/real-blog` in the host.

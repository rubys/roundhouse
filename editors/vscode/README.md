# Roundhouse LSP — VS Code dev client

A minimal client for exercising [`roundhouse-lsp`](../../src/bin/roundhouse-lsp.rs)
under VS Code's F5 dev loop. It spawns the locally-built binary and attaches
it to Ruby files, so you get inferred-type hovers, inlay hints, nil-safety
diagnostics, find-references, go-to-definition, and a request-trace
CodeLens above every controller action over a whole Rails app.

The server is found in this order: the `roundhouse.serverPath` setting;
a `roundhouse` binary on PATH (run as `roundhouse lsp` — the multi-call
binary a release tarball installs); and, for the F5 dev loop, the repo's
own `../../target/release/roundhouse-lsp`. There is no packaging here yet:
a published `.vsix` is the natural next step once a release exists, and
needs nothing more than `vsce package` on this folder.

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

## What you see

- **Diagnostics** — errors, syntax errors and static N+1 findings, in
  the Problems panel. The analyzer's warning ledger (`gradual_untyped`,
  `unresolved_type`; hundreds on a real app) is off by default: turn on
  `roundhouse.warnings` to publish it too. The setting takes effect on
  the next analysis pass, no reload.
- **Trace CodeLens** — `▶ trace GET /rooms/:id · 9 filters · 2 skipped ·
  trace complete` above each action (on the `class` line for actions the
  controller inherits). Click it: the request chain — route, every
  filter with its guard and what it assigns, the action, the view with
  its partials, the layout, and the coverage footer — opens as a
  Markdown document (written under the OS temp dir, never into the app).

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

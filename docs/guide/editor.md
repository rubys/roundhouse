# The editor — `roundhouse lsp`

`roundhouse lsp` serves the analyzer over the Language Server
Protocol on stdio. Open a Rails app in an editor with the client
attached and every Ruby, ERB and HAML file in it gets whole-app
inferred types: hover on `@article` and see `Article`; hover on
`Article.find_by(slug:)` and see `Article | nil`. There are no
annotations to write and no server to warm up — the analysis is a
whole-app pass that takes about a second on a large codebase and
re-runs as you edit.

## What you get

- **Hover** — the inferred type of the expression under the cursor,
  and whether it can be nil. On a `def` line, the method's signature.
- **Inlay hints** — inline `: Type` annotations after assignments.
  Turn them off without losing hover with
  `"[ruby]": { "editor.inlayHints.enabled": "off" }` in VS Code.
- **Go to definition / find references** — resolved by the analyzer's
  bindings, not by name: a local resolves to its exact binding, an
  instance variable to every read and write across the controller and
  the views it feeds, a method to its `def`. F12 on a bare call lands
  on the definition; references from a `def` header find the callers.
- **Completion** — members after `.`, instance variables after `@`,
  typed keyword arguments inside a call's parentheses, drawn from the
  inferred receiver type.
- **Diagnostics** — in the Problems panel: analysis errors, syntax
  errors, and static N+1 findings (`missing_preload`, with the
  `.includes` fix). The analyzer's warning ledger (`gradual_untyped`,
  `unresolved_type` — hundreds on a real app) is off by default; the
  `roundhouse.warnings` setting turns it on, effective on the next
  analysis pass. [`check.md`](check.md) explains each diagnostic kind.
- **Trace CodeLens** — `▶ trace GET /rooms/:id · 9 filters · 2 skipped ·
  trace complete` above each controller action (on the `class` line
  for actions a controller inherits). Click it and the request chain
  opens as a Markdown document: the route, every before/around/after
  filter with its guard and what it assigns, the action, the view with
  its partials, the layout, and a coverage footer saying which hops
  resolved and what blocks the rest. The document is written under
  the OS temp directory, never into the app.

## VS Code

The client lives in the repository at
[`editors/vscode/`](../../editors/vscode/). It is not on the
Marketplace yet; installing it takes the repository and `npm`:

```sh
git clone https://github.com/rubys/roundhouse
cd roundhouse/editors/vscode
npm install
```

Then either open that folder in VS Code and press **F5** — an
Extension Development Host window opens with the client loaded, and
File → Open Folder points it at your app — or package it once and
install the result in your everyday VS Code:

```sh
npx @vscode/vsce package     # writes roundhouse-lsp-client-0.0.1.vsix
code --install-extension roundhouse-lsp-client-0.0.1.vsix
```

The client activates on Ruby, ERB and HAML files and on any workspace
containing `config/routes.rb`. It finds the server in this order:

1. the `roundhouse.serverPath` setting, if set — a path to the
   `roundhouse` binary (run as `roundhouse lsp`) or to a source-built
   `roundhouse-lsp`;
2. a `roundhouse` on `PATH` — the snapshot binary from
   [`install.md`](install.md);
3. for the F5 loop only, the repository's own
   `target/release/roundhouse-lsp`.

Nothing else is required: `.rb` files get the `ruby` language id from
VS Code's built-in grammar. The server's stderr goes to the Output
panel's "Roundhouse LSP" channel; that is where to look if hovers
never appear.

## Other editors

Any LSP client can run it. The server is `roundhouse lsp` with no
arguments, over stdio; the client names the workspace root at
`initialize`, and the root should be the Rails app (the directory
holding `config/routes.rb`). Attach it to the `ruby`, `erb` and `haml`
language ids — or just `ruby` — and everything above except the
CodeLens command works with no client-side code. The CodeLens needs
the client to forward `workspace/executeCommand` for
`roundhouse.traceroute` and open the returned file, which most clients
do by default.

For Neovim 0.11 or later, for example:

```lua
vim.lsp.config('roundhouse', {
  cmd = { 'roundhouse', 'lsp' },
  filetypes = { 'ruby', 'eruby', 'haml' },
  root_markers = { 'config/routes.rb' },
})
vim.lsp.enable('roundhouse')
```

## What it is not

It is not a replacement for ruby-lsp. Roundhouse knows types, effects
and request flow; it does not know formatting, refactoring, snippets,
or anything about Ruby files that are not part of a Rails app. Running
both is fine: they answer different questions, and neither one's
diagnostics collide with the other's.

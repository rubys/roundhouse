# Install

Roundhouse ships as one binary, `roundhouse`, with the analyzer, the
LSP server, the MCP server and every transpile target inside it. There
are two ways to get it. The snapshot binaries cover most needs; a
source build is always available for everything else.

## The snapshot binaries

Releases are dated snapshots, in the style of Spinel's: a tag names the
day a release was cut, and what it promises is in
[`RELEASES.md`](../../RELEASES.md) and the CI that ran on that commit.
There is no semantic version to interpret — a newer date is a newer
snapshot, and each one carries prebuilt binaries for:

| Platform | Archive |
|---|---|
| macOS on Apple silicon | `roundhouse-aarch64-apple-darwin.tar.xz` |
| Linux x86-64 (static, musl) | `roundhouse-x86_64-unknown-linux-musl.tar.xz` |

These two are the platforms the project itself runs on daily; every
CI lane that exercises the binary runs on one or the other. The release
matrix also builds macOS on Intel and Windows x86-64 archives, and they
are attached to each snapshot, but nothing in CI exercises them beyond
`roundhouse --version` — treat them as a courtesy, and report what
breaks.

The installer script picks the right archive and puts `roundhouse` in
`~/.local/bin`:

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/rubys/roundhouse/releases/latest/download/roundhouse-installer.sh | sh
```

Or download the archive from the
[releases page](https://github.com/rubys/roundhouse/releases), unpack
it, and put the one file somewhere on your `PATH`. There is nothing
else in the archive that the binary needs.

Then:

```sh
roundhouse --version
```

prints the snapshot and the commit it was built from — for example
`roundhouse 2026.9.16 (f6aa410a)`. That line is the first thing to
include in a bug report; it is also what the MCP server reports as its
`serverInfo` and what the LSP reports at `initialize`, so an editor's
log names the same build.

The binary is self-contained: no Ruby, no gems, no database, nothing
to download at first run. It reads Rails source and writes files. The
things it *produces* have their own prerequisites — a transpiled Rust
project needs `cargo`, a Spinel binary needs the Spinel compiler — and
each door's page lists them.

## Building from source

The binaries cover the common case. Build from source when they don't:
another platform or CPU, a commit newer than the last snapshot, a fix
you are testing before it lands, or the developer tools (`roundhouse-ast`,
`dump_ir`, `emit_preview`) that the snapshot deliberately leaves out.

You need a Rust toolchain (1.85 or later) and a working libclang: the
`ruby-prism-sys` and `ruby-rbs-sys` build scripts generate their C
bindings with bindgen, which loads clang's own resource headers.

- **macOS**: Xcode Command Line Tools (`xcode-select --install`) is
  all that's needed.
- **Debian / Ubuntu**: `sudo apt install clang libclang-dev` — both
  packages. Installing only `libclang-dev` fails with
  `fatal error: 'stddef.h' file not found`, because clang's builtin
  headers ship in the `clang` package.

Then:

```sh
git clone https://github.com/rubys/roundhouse
cd roundhouse
cargo build --release --bin roundhouse
./target/release/roundhouse --version
```

A source build stamps `--version` with the checkout's commit, so a
build from between snapshots is told apart from the snapshot by that
commit; the same file works anywhere the snapshot binary does,
including as the editor's `roundhouse.serverPath`.

`cargo build --release` without `--bin` also builds the cargo-only
aliases (`roundhouse-check`, `roundhouse-lsp`, `roundhouse-mcp`) and the
IR inspection tools; [`DEVELOPMENT.md`](../../DEVELOPMENT.md) covers
those and the test suite.

## What is not in the box

- **Spinel.** The compile door needs the Spinel compiler and its `spin`
  project tool on `PATH`, built from Spinel's own source snapshots. Each
  roundhouse snapshot names the Spinel release it was tested against in
  [`RELEASES.md`](../../RELEASES.md); [`spinel.md`](spinel.md) walks
  through the install.
- **Target toolchains.** Transpiling to Go produces a Go project; running
  it needs Go. [`targets.md`](targets.md) has the per-target list.
- **The editor extension.** [`editor.md`](editor.md) — the VS Code
  client is small enough to install from the repository today.
- **A Rails app to point it at.** Any checkout works. If you want a
  known-good one, the Rails Guides store fixture (`fixtures/store` in
  the repository) checks clean, and it is the app used in the examples
  throughout this guide.

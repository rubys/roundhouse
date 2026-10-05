# Debugging

First locate the broken boundary: ingest IR, typed/lowered IR, emitted source,
or the running program. Build the relevant tool once or invoke it with
`cargo run --bin <name> --`.

## Inspect a Ruby expression

[`roundhouse-ast`](../../src/bin/roundhouse-ast.rs) exposes Prism, template
compilation, ingest, and Ruby expression emission:

```sh
cargo run --bin roundhouse-ast -- --help
cargo run --bin roundhouse-ast -- -e '[:a, :b]'
cargo run --bin roundhouse-ast -- --stage prism -e '@x.y do end'
cargo run --bin roundhouse-ast -- --stage compile-erb view.html.erb
cargo run --bin roundhouse-ast -- --stage emit-ruby -e '"a#{x}b"'
cargo run --bin roundhouse-ast -- --stages --erb -e '<%= x %>'
cargo run --bin roundhouse-ast -- --round-trip -e '[:a, :b]'
```

A positional `.rb`/`.erb` file replaces `-e`; `.erb` selects template mode.
IR stages print JSON. `--round-trip` accepts plain Ruby only and compares
ingest → emit-ruby → ingest IR; it does not prove whole-app source equality
or runtime correctness. On divergence it prints both IRs and a diff.

## Inspect lowered IR

[`dump_ir`](../../src/bin/dump_ir.rs) analyzes and lowers a fixture before
dumping the shape consumed by application emitters. Narrow it to the failing
class/method rather than reading the entire app:

```sh
cargo run --bin dump_ir -- --help
cargo run --bin dump_ir -- fixtures/real-blog --select 'ArticlesController#create'
cargo run --bin dump_ir -- fixtures/real-blog --format json --select Article
```

`--raw-views` instead shows pre-lowering view bodies. Use the same selector
as the failing regression to distinguish a wrong lowering from a wrong emitter.

## Inspect emitted files

[`emit_preview`](../../src/bin/emit_preview.rs) is a narrow bench/browser
debugging tool. It analyzes source IR directly and emits bare files, **not**
the complete packaged application or the shared post-analyze lowering path:

```sh
cargo run --bin emit_preview -- --target rust --out /tmp/rh-preview fixtures/real-blog
```

It has no `--help`; its source lists supported targets and TypeScript profiles.
It replaces the output directory, so use a disposable path. For normal
application assembly, including Ruby/Spinel, use `bin/rh transpile <target>`.

## Compare running output or measure phases

`bin/rh compare <target>` drives Rails and the emitted server together.
The raw comparator is a [standalone crate](../../tools/compare/), not a root
Cargo binary. Using it on your own app: [the compare guide](../guide/verifying.md).
Do not mask a genuine translation bug as a variable value.

To see compiler phase wall times and process peak RSS:

```sh
ROUNDHOUSE_TIMINGS=1 cargo test --test emit_and_run the_unedited_blog_runs -- --nocapture
```

Timing is opt-in; a process high-water RSS is not per-phase allocated memory.
Other debugging controls are discoverable at their readers:
`rg -n 'ROUNDHOUSE_' src/ scripts/`. Matches include comments and generated
code; inspect the actual read before assuming a variable is live.

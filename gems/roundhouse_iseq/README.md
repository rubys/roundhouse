# roundhouse_iseq

MRI-only gem that compiles lowered Ruby **units** to
`RubyVM::InstructionSequence` binaries and loads them by **manifest key**.

This is Roundhouse’s CRuby delivery vehicle — the Bootsnap-shaped piece for
emitted trees, not a `Bootsnap.setup` drop-in for unmodified Rails.

## Why

Roundhouse lowers Rails to shape-stable Ruby. Shipping that as `.rb` still
makes MRI parse it at boot, and backtraces name the lowered tree. This gem:

1. Compiles each unit with `InstructionSequence.compile(source, file, …)` so
   `file` can be the **original** app path from IR spans.
2. Loads by logical key from `manifest.json`, so require does not need those
   paths on disk.

## Layout

```text
out/
  manifest.json
  iseq/<key>.iseq
```

## Usage

```ruby
require "roundhouse_iseq"

RoundhouseIseq::Compiler.compile!(
  units: [
    {
      "key" => "app/models/article",
      "source" => "class Article; end\n",
      "file" => "real-blog/app/models/article.rb",
      "first_lineno" => 1,
    },
  ],
  out_dir: "/tmp/app",
)

loader = RoundhouseIseq::Loader.install!(root: "/tmp/app")
loader.boot!("app/models/article")
```

CLI (used by Roundhouse at emit time):

```sh
roundhouse-iseq-compile --out /tmp/app --units units.json
# or: … < units.json
```

## Tests

```sh
cd gems/roundhouse_iseq
ruby -Ilib:test test/compiler_loader_test.rb
```

## Limits

- MRI only (`RUBY_ENGINE == "ruby"`).
- One ISeq has one `file`; mixed-origin bodies need split units or CRuby `#line`.
- ISeq binaries are tied to `RUBY_DESCRIPTION`; rebuild after Ruby upgrades.

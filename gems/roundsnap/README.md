# roundsnap

MRI-only gem that compiles lowered Ruby **units** to
`RubyVM::InstructionSequence` binaries and loads them by **manifest key**.

Roundsnap is Roundhouse’s CRuby delivery vehicle — the Bootsnap analogue for
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
require "roundsnap"

Roundsnap::Compiler.compile!(
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

loader = Roundsnap::Loader.install!(root: "/tmp/app")
loader.boot!("app/models/article")
```

CLI (used by Roundhouse at emit time):

```sh
roundsnap-compile --out /tmp/app --units units.json
# or: … < units.json
```

## Opt-in at emit time

```sh
ROUNDSNAP=1 bin/rh transpile ruby --app ~/git/once-campfire --out /tmp/campfire-roundsnap
```

(`ROUNDHOUSE_RUBY_ISEQ=1` remains accepted as an alias.)

## Tests

```sh
cd gems/roundsnap
ruby -Ilib:test test/compiler_loader_test.rb
```

## Campfire prototype check

```sh
scripts/campfire-roundsnap
```

Emits once-campfire with Roundsnap, bundles, and boots. That is the
standard smoke for this gem — not a one-off manual trial.

## Limits

- MRI only (`RUBY_ENGINE == "ruby"`).
- One ISeq has one `file`; mixed-origin bodies need split units or CRuby `#line`.
- ISeq binaries are tied to `RUBY_DESCRIPTION`; rebuild after Ruby upgrades.

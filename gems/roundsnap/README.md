# roundsnap

MRI-only gem that compiles lowered Ruby **units** to
`RubyVM::InstructionSequence` binaries and loads them by **manifest key**.

Roundsnap is Roundhouse’s CRuby delivery vehicle — the Bootsnap analogue for
emitted trees, not a `Bootsnap.setup` drop-in for unmodified Rails.

## Why

Roundhouse lowers Rails to shape-stable Ruby. Shipping that as `.rb` still
makes MRI parse it at boot, and backtraces name the lowered tree. This gem:

1. Compiles each unit with `InstructionSequence.compile(source, file, …)`,
   expanding `#<SPINEL_SOURCE>` spans so `file` + line numbers match the
   **original** app `.rb` / `.erb`.
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

## Source mapping

When unit source carries `#<SPINEL_SOURCE>file:line` markers (Roundhouse
emits them under `ROUNDSNAP=1`), `SourceMap` expands each contiguous
span: unmarked prefix (requires / `module` wrappers) stays at the top,
marked statements are blank-padded onto their absolute source lines, and
`first_lineno = 1 - prefix_len` so backtraces report the app line —
including ERB templates. Unmarked units keep the **emit** path (honest
emitted lines; never original path + emitted linenos).

## Limits

- MRI only (`RUBY_ENGINE == "ruby"`).
- Contiguous-span alignment (not yet one ISeq per method). Held markers
  with several statements auto-increment after the first exact line.
- One original file per marked unit — mixed `#<SPINEL_SOURCE>` files in
  the same source raise (flushing mid-wrapper would break `end` balance).
- ISeq binaries are tied to `RUBY_DESCRIPTION` and
  `InstructionSequence.compile_option`; rebuild after Ruby upgrades or
  option changes.
- No `$LOAD_PATH` pre-scan and no YAML/JSON compile cache — the emitted
  tree resolves by manifest key. Those Bootsnap features stay out of scope.
- The loader still uses `Kernel.prepend` require hooks so `require_relative`
  against original ISeq `file` paths can resolve emit keys. Absolute host
  paths are never rewritten to manifest keys.

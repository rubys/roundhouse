# roundsnap

Opt-in MRI ISeq delivery for **Roundhouse-emitted** Ruby, not a
`Bootsnap.setup` replacement for unmodified Rails. Throughput is unchanged
by construction; avoiding parse-at-boot is a small cold-start optimization.

## Delivery

```sh
ROUNDSNAP=1 bin/rh transpile ruby --app path/to/app --out /tmp/emitted-app
# Legacy alias: ROUNDHOUSE_RUBY_ISEQ=1
# Keep app/runtime sources too: ROUNDSNAP_KEEP_SOURCE=1
```

The gem is embedded in the Roundhouse executable and vendored into the
output. Compilation uses `ruby` on the emitter's PATH, through that exact
vendored compiler. Deploy with the same MRI version, platform and compile
options. A mismatch fails with a rebuild hint; YJIT on/off is compatible.

```text
out/
  boot.rb                     original boot chain + loader installation
  main.rb, config.ru          unchanged entry points
  config/, db/, test/, tools/  retained source
  vendor/roundsnap/
  manifest.json               units + emitted-to-source sidecar
  iseq/<generation>/<key>.iseq
```

App/runtime units compile **without changing their source bytes**. Requires
load them lazily, in the original program's order, including conditional
requires. There is no ad-hoc require-graph parser or Campfire-specific stub.
Seeds, tools and other uncompiled files are not deleted. `ROUNDSNAP_KEEP_SOURCE=1`
is necessary for consumers that read or `load` app/runtime source directly;
`Kernel.load` is not an ISeq require and keeps normal MRI behavior.

Each compiler run stages a fresh generation, then atomically publishes its
manifest. A compilation failure leaves the previous manifest and binaries
intact. Older generations stay available to already-installed loaders; use a
fresh output directory for deployment artifacts rather than accumulating
build generations indefinitely. Re-emission is not an atomic deployment of
the entire tree (config, assets and other text files are written separately).

ISeq `__FILE__` / `__dir__` are absolute **emitted** paths fixed at compile
time, independent of the server's working directory. Requires still resolve
after moving the output, but code reading files via `__dir__` needs compilation
at its final deployment path. The original-source labels in the sidecar are
diagnostic metadata, never load paths.

## Source locations

`#<SPINEL_SOURCE>file:line` comments from the IR become a sidecar in each
manifest entry. Backward markers, repeated lines and spliced concerns from
multiple files cannot reorder code. Blank lines and heredocs stay untouched;
line-zero metadata is rejected, not silently dropped. Held markers retain the
last IR span position (as Spinel does), **not** guessed auto-incremented lines.

Native backtraces, Coverage and profiler locations remain honest emitted
file:line locations. Source formatting is explicit:

```ruby
begin
  Article.new.some_method
rescue => error
  warn Roundsnap::Loader.current.format_backtrace(error.backtrace).join("\n")
end
```

This does not mutate the exception. Unmapped frames are unchanged. It is
span-level mapping, not native one-ISeq-per-method mapping or transparent
Coverage remapping. Synthesized code can inherit the last pinned span.

## Gem API

```ruby
require "roundsnap"
Roundsnap::Compiler.compile!(
  units: [{ "key" => "app/probe", "source" => "class Probe; end\n",
            "file" => "app/probe.rb", "first_lineno" => 1 }],
  out_dir: "/tmp/app",
)
loader = Roundsnap::Loader.install!(root: "/tmp/app")
loader.require("app/probe")
# boot! loads only its entry and that entry's requires, not every unit.
```

CLI: `roundsnap-compile --out /tmp/app --units units.json` (or JSON on stdin).
ISeq binaries execute code: manifests and binaries must be trusted build
artifacts. Path validation is not a sandbox for untrusted bytecode.

## Verification

```sh
for file in gems/roundsnap/test/*_test.rb; do ruby -Igems/roundsnap/lib "$file"; done
cargo test --locked --test roundsnap_delivery -- --nocapture
scripts/campfire-roundsnap --out /tmp/campfire-roundsnap path/to/once-campfire
scripts/campfire-suite --reuse /tmp/campfire-roundsnap --no-stubs --jobs 1
```

The default delivery gate emits real-blog, boots without app/runtime `.rb`,
runs its model/controller suites, and checks a model + ERB source frame through
the full ingest/lower/emit/ISeq pipeline. CI's existing `campfire-conformance`
job also runs the pinned Campfire app's full emitted suite in both plain Ruby
and Roundsnap modes, without stubs, and requires matching per-file results
and per-test failures. The Campfire script is only a boot smoke,
not proof that all Campfire tests pass. `campfire-suite` reports a ledger and
exits zero on completion even when tests fail; compare identities and results
against the same pinned app's plain Ruby lane, not only its exit status.

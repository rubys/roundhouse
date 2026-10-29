# Fractal: compile a Rails app in place, one file at a time

`fractal emit --file PATH APP -o OUT` writes only changed files under
their app-relative paths. The output is a file overlay, not a complete
app. Emit leaves the base app unchanged.

`fractal apply OUT APP` copies those files onto the base app and replaces
files with the same paths. Rails runs the rewritten source with the same
gems and line numbers. Nothing installs code at boot or checks assumptions
at load. Each rewritten file contains the code that runs.

The point is application knowledge. Roundhouse ingests and analyzes the
files it compiles and what its passes need, so at build time it knows
things a JIT cannot learn from inside one method, and that Rails
recomputes on every boot: the types flowing through a body, its effects,
which definitions exist and where, which descriptions (an `enum`
mapping, a column list) are fixed. A Fractal pass uses that to remove
work: framework machinery that interprets a description fixed at build
time, an intermediate object nothing can observe, a method Rails
generates at boot from a description the build already read. Inlining
for its own sake, devirtualization and constant folding are the JIT's
job, not Fractal's.

Because a rewrite is an edit to source, a rewrite of a class body (work
Rails does at boot) and a rewrite of a method body (work it does at run
time) are the same kind of edit, applied the same way.

## Emit, then apply

1. Emit the file overlay:

   ```sh
   fractal emit --file app/models/article.rb path/to/app -o out/fractal
   ```

2. Apply the overlay to the base app:

   ```sh
   fractal apply out/fractal path/to/app
   ```

The Cargo binary is `fractal`. The shipped binary accepts the same
commands as `roundhouse fractal emit`, `roundhouse fractal apply` and
`roundhouse fractal verify`.
`roundhouse --target fractal` also uses the same emission step.

Emit defaults to the current app and `out/fractal`. Apply uses the same
defaults. The overlay can be inside the base app. Apply does not create
a second app. It rejects symbolic links and incompatible destination
types before it copies files.

Apply never deletes files or restores earlier source. An overlay with
no changed app files leaves the base app unchanged. Removing a `--file`
removes that file from the next overlay, not from the base app.
Use version control to restore source before compiling a different selection.

The overlay includes `.roundhouse-fractal.json` with the compiled files,
passes, and changed files. Apply keeps this metadata in the overlay.
An unchanged build emits only the metadata file.

## Pipeline

All of it lives in [`src/emit/fractal/`](../../src/emit/fractal/).

1. **Scope** ([`scope.rs`](../../src/emit/fractal/scope.rs)). The build
   ingests the files it compiles, plus what each pass declares it needs
   ([`Pass::scope`](../../src/emit/fractal/pass.rs)), found through a
   Rubydex index of the app's constants (the files defining the
   ancestors of the classes the compiled files define, say), or the
   whole app when a pass needs it or `--whole-app` asks. A `--file` that
   does not exist, or lies outside the app, is an error.
2. **Ingest and analyze** those files, without lowering. Files ingest
   can only partly read (a `--survey` gap, a syntax error) stay unchanged.
   The overlay does not include them.
3. **Select** ([`select.rs`](../../src/emit/fractal/select.rs)). The
   compiled files, and every `class`/`module` body in them, so a pass
   knows which class each site belongs to: the innermost body around it.
4. **Passes** ([`pass.rs`](../../src/emit/fractal/pass.rs)). Each
   compiled file is a [`Unit`](../../src/emit/fractal/pass.rs): its text,
   its class bodies, and its methods with the analyzer's facts
   ([`region.rs`](../../src/emit/fractal/region.rs)). Each pass proposes
   edits anywhere in it, and declines what it cannot show equivalent,
   with a reason.
5. **Rewrite** ([`rewrite.rs`](../../src/emit/fractal/rewrite.rs)). The
   driver applies the proposals in pass order. A proposal is refused
   whole, and reported, when an edit falls outside the file, overlaps
   another edit, or changes the number of lines; or when the rewritten
   file parses with an error or warning the original did not have.
6. **Emit** ([`tree.rs`](../../src/emit/fractal/tree.rs)). The output
   contains only changed files and `.roundhouse-fractal.json`.
   A rebuild replaces an earlier overlay. It refuses other non-empty
   directories, the app itself, and directories containing the app.
   It rejects rewritten paths with parent or absolute components before
   it changes an earlier output.
7. **Apply** ([`tree.rs`](../../src/emit/fractal/tree.rs)). The separate
   command overwrites only files present in the overlay. It leaves
   unrelated files unchanged and does not copy the build metadata.

`--report PATH` writes the build's decisions as JSON, after the overlay:
the compiled files, every method in them with the analyzer's facts, files
skipped and why, every rewrite (`path:line:col`, pass, old and new text)
and every declined site with the pass and reason. A report path inside
the overlay is refused before anything is read or written: a rebuild
would delete it, and one named like the metadata file would make an
unrelated directory look like an earlier build.

A compiled file that names a namespace by a relative path the compiled
files do not define (`class B::C` inside `module A`, with `A::B` defined
elsewhere) is named against the app's sources, since a scoped ingest may
not have read the file that settles it.

Source discovery keeps nested namespaces such as `app/models/public/`
and `app/models/test/`. It excludes test and state directories only at
the app root. It excludes `node_modules`, hidden directories, and earlier
output overlays at any depth.

## Line numbers

Every edit keeps the number of lines it replaces, so a line of the
compiled app is the same line of the source. Backtraces, error grouping,
coverage, `__LINE__` and `source_location` point at the code a person
wrote, and the compiled file can be read side by side with the original.
A pass that needs more room writes several statements on one line.

## No runtime fallback

The compiled app has no guard that declines a rewrite when an assumption
fails at load, and no original to fall back to. Every assumption a
rewrite rests on is checked at build time, from the app's source and
lockfile, and anything a pass cannot check is declined. That makes the
build, not the boot, responsible for correctness:

- a pass must account for every way the app could make its rewrite
  wrong (an app definition of a framework method it relies on, a
  subclass that overrides it, a version of Rails that behaves
  differently), and decline where it cannot;
- the applied app is what gets tested: apply the overlay, then run
  the app's test suite;
- the build must see what the deploy will run: the same source, the same
  `Gemfile.lock`.

`fractal verify` checks a booted app's state against the source's (see
below). Stamping a build with what it was compiled from is a natural next
step for running this on a large app; this proof of concept does not do
it yet.

## Verifying an overlay

```sh
# From the app's root, with the report of the build that wrote the overlay
fractal emit --file app/models/article.rb --report report.json . -o out/fractal
fractal verify --report report.json --ruby 'bundle exec ruby' \
  --require config/environment out/fractal .
```

[`verify.rs`](../../src/emit/fractal/verify.rs) boots the app twice in
fresh Ruby processes, before and after copying the overlay onto it, and
restores the app's files afterwards whether or not the second boot
succeeds. While the overlay is applied, Ctrl+C stops the Ruby boot but not
`verify`, which still restores the files (on Unix); copies of the
originals are also written to `originals/` in the working directory
(`--keep DIR`, or a temporary directory removed after a successful run).
Each boot requires the `--require` files, then the compiled files, and
[`fingerprint.rb`](../../src/emit/fractal/verify/fingerprint.rb)
fingerprints every named module with a method in a compiled file (every
named module with `--all`): its ancestors, singleton ancestors, constants
and instance variable names, and for each of its own methods the owner,
visibility, parameters, arity, original name, source location and a
digest of its bytecode (`RubyVM::InstructionSequence#to_a`). The bytecode
keeps line numbers, which a rewrite preserves, and drops column and node
positions, the absolute path (code compiled from a string has none), and
memory addresses (a builtin's C function pointer, an embedded object's
inspect). `--keep DIR` keeps both fingerprints as JSON.

The overlay is verified, and the command exits 0, only when:

- the two fingerprints are equal; a difference names the rewrite on the
  method's line, if any;
- every rewrite in the report was observed: some method in both boots is
  defined on its line. A rewrite no method sits on was never checked, so
  it fails rather than passing vacuously;
- no method changed from compiled-from-a-string to compiled-from-the-file
  while naming a constant by a relative path. Constant lookup depends on
  the lexical scope the code was compiled in, which the bytecode does not
  record, so equal bytecode does not settle such a method.
  `inline-delegate` writes every constant with a leading `::` for this
  reason.

The report and the overlay's `.roundhouse-fractal.json` must name the same
rewritten files, or `verify` refuses them as different builds before
booting anything. The overlay must also hold exactly those files, and
`--keep` must lie outside it: only the reported files are saved and
restored, so an extra file would be left in the app, and a missing one
would compare the original with itself.

A pass that lowers an expression (`lower-duration`) changes the bytecode
of the line it rewrites by design
([`Pass::changes_bytecode`](../../src/emit/fractal/pass.rs)), so for its
rewrites the first two conditions read differently. A rewrite is observed
when a method that both boots define in the same place spans its line,
and that method's bytecode may differ on the rewritten lines only. Its
other lines (each one's instructions, across the method's blocks) and the
shape of its instruction sequences (headers and catch tables, without the
stack depth) must compile as before. A jump label is named by the line it
is placed on rather than by offset or by its order in the sequence, in the
body, the catch tables and the optional parameters' defaults, so neither
more instructions on a rewritten line nor more branches (a conditional
the rewrite writes) renames the labels another line jumps to. On its own
line a label is also numbered among that line's labels; from another line,
a header or a catch table it is only "a label on line *n*". That the
rewritten line does what it did rests on
the pass and its tests, not on `verify`. A rewrite in a block outside a
method (a `scope` lambda) has no method to sit in, and is unobserved.

Equal fingerprints show the rewritten app defines the same methods with
the same bytecode. They do not cover state the fingerprint leaves out:
values of class-level instance variables (only their names), methods Rails
generates lazily after boot (attribute methods appear only once a model is
instantiated), or code paths the boot never loads. `verify` is not in the
browser build.

## Passes

Two passes are registered, and run in this order: `inline-delegate` and
`lower-duration`.

### `inline-delegate`

[`passes/inline_delegate.rs`](../../src/emit/fractal/passes/inline_delegate.rs).
`delegate :name, to: :author` makes Active Support build Ruby source for
each method at boot and `module_eval` it. The pass writes that exact
source where the call was, so Rails does not generate it, Bootsnap caches
it with the file, and the JIT sees a plain `def`. The methods go on the
call's first line, which is the line Active Support gives them, and the
call's other lines are left empty.

It rewrites only what it can reproduce exactly, and declines everything
else with a reason:

- the call is a bare statement of a `class`, `module` or `class << self`
  body, not the last statement of that body, and nothing before it in the
  body may change the default visibility (a `private`, `protected`,
  `public` or `module_function` with no arguments, however it is called:
  `self.private`, `private if ...`, `send(:private)`);
- the file activates no refinement with `using`: a `def` written in the
  file would see it, Active Support's `module_eval`'d methods do not;
- methods are symbol literals naming plain methods (no writers, operators
  or reserved names), and the only option is `to:` with a symbol literal
  naming a plain method (no `prefix:`, `allow_nil:`, `private:`, instance
  variables, constants, or `:class`);
- `Gemfile.lock` resolves activesupport 8.2.0.alpha, whose generator it
  was checked against (rails/rails main); any other version is declined
  rather than assumed to generate the same source;
- the app's source defines no `delegate` that a class body could call
  instead
  ([`passes/delegate_definitions.rs`](../../src/emit/fractal/passes/delegate_definitions.rs)):
  a singleton method, one in `class << X`, in `Module`, `Class`, `Object`,
  `BasicObject` or `Kernel`, at the top level, in a block, through a
  naming call with an explicit receiver or `send`, through
  `delegate :delegate`, an `undef_method :delegate` that would send the
  call to `method_missing`, or in a module that can reach a singleton
  chain (extended, copied onto its own singleton by `module_function`,
  mixed into a singleton class or into `Class` or `Module`, or included
  into a module that can, to a fixed point).
  Instance methods of ordinary classes (a controller action named
  `delegate`) don't count. Names computed at run time
  (`define_method(name)`, `delegate(*NAMES, ...)`) can't be seen, so they
  aren't counted.

The checks read calls directly and through `send`, `__send__` and
`public_send` with a literal name. Other dynamic dispatch
(`method(:private).call`, `eval`, `instance_eval` with a string) and
names computed at run time are beyond what the source shows; the pass
trusts that the app doesn't use them to change visibility, activate
refinements or define `delegate`.

The pass reads the compiled file, the lockfile and the app's Ruby files
from source (`Context`), so it widens no ingest scope. A file ingest
records a gap in keeps the app's code before the pass sees it, as every
skipped file does.

[`tests/fractal_inline_delegate.rs`](../../tests/fractal_inline_delegate.rs)
pins the rewrites and every decline. It also loads the original and the
rewritten class in Ruby with Active Support's delegation, and compares
each method's owner, visibility, parameters, arity, source line, return
values, a `NoMethodError` raised inside the target, and the error for a
nil target. That comparison runs only where `ruby -e 'require
"active_support/core_ext/module/delegation"'` works.

### `lower-duration`

[`passes/lower_duration.rs`](../../src/emit/fractal/passes/lower_duration.rs).
`5.days.ago` builds an `ActiveSupport::Duration` and its parts hash in
`Numeric#days`, then `Duration#ago` calls the private `#sum`, which builds
a second hash to pass `Time#advance` its keywords. For a Duration with one
part, `sum` comes down to one call on `::Time.current`, which is
`::Time.zone ? ::Time.zone.now : ::Time.now`. The pass follows that call
down to Ruby's own `Time` arithmetic where it can, and writes the result,
with the number folded, in place of the chain:

| Written | Rewritten |
| --- | --- |
| `5.days.ago` | `(::Time.zone ? ::Time.zone.now.plus_without_duration(-432000) : ::Time.now.advance(days: -5))` |
| `1.fortnight.after` | `(::Time.zone ? ::Time.zone.now.plus_without_duration(1209600) : ::Time.now.advance(weeks: 2))` |
| `3.months.from_now` | `::Time.current.advance(months: 3)` |
| `2.hours.ago` | `(::Time.zone ? ::ActiveSupport::TimeWithZone.new(::Time.now.utc + -7200, ::Time.zone) : ::Time.now + -7200)` |
| `30.seconds.until` | `(::Time.zone ? ::ActiveSupport::TimeWithZone.new(::Time.now.utc + -30, ::Time.zone) : ::Time.now + -30)` |

- **Seconds, minutes and hours.** `TimeWithZone#since` adds the seconds
  to its UTC time and wraps the sum in its zone, and `Time.zone.now` is
  `Time.now.utc` wrapped in the zone, so the pass writes one wrap around
  `::Time.now.utc + s`, skipping the intermediate `TimeWithZone`. With no
  zone, `Time#since` is `Time.now + s`.
- **Days, weeks and fortnights.** These stay calendar arithmetic, so a day
  across a DST change is still a day of the wall clock, as in Rails.
  `TimeWithZone#advance` hands the call to its local time, a UTC `Time`
  that holds the wall clock, through `method_missing`, which wraps the
  result back in the zone. If the result's wall-clock time occurs twice,
  the wrap keeps the original's offset. If it is skipped, it moves forward
  an hour. A UTC `Time` has no DST, so there a day is exactly 86,400
  seconds. Instead of `advance` (three `Date`s, a hash and the
  `Rational`s for the subsecond), the pass forwards Ruby's own `Time#+` the same way, under
  the name Active Support keeps for it, `plus_without_duration`. The wrap
  is the one `advance` gets. With no zone, `Time.now` is a local time,
  which does have DST, so that branch keeps `::Time.now.advance`.
- **Months and years.** These stay `::Time.current.advance`. Ruby has no
  month arithmetic on `Time`. Doing it with `Date#>>` needs the time twice,
  and an expression cannot hold it without a local variable, which would
  change the method's header.

Writing a day as 86,400 seconds of the zone's UTC time (`since(-432000)`,
or `::Time.zone.now + -432000`) would give a different result for windows
that cross a DST change. The runtime test checks that it catches both.

The conditional is `Time.current`'s own, so the rewrite reads `Time.zone`
as many times as the original does. Its branches add labels to the
rewritten line, which `verify` allows (see above).

It rewrites only what it can reproduce exactly, and declines everything
else with a reason:

- the receiver is an integer literal that fits 32 bits (not a variable, a
  constant or a float, whose class the source does not settle), the unit
  is one Active Support defines on `Numeric` or `Integer` (`second` to
  `year`, singular or plural, and `fortnight`), and the direction is
  `ago`, `until`, `before`, `since`, `from_now` or `after`;
- both calls use `.` (not `&.`), pass no block, and the direction gets no
  argument (`5.days.ago(time)` checks `time.acts_like?` first);
- the chain is on one line, inside a `def`, block or lambda: a chain run
  once, as the file loads (a constant's value), saves nothing, and
  `verify` could not check it;
- the file activates no refinement with `using`;
- `Gemfile.lock` resolves activesupport 8.2.0.alpha, whose `Duration` it
  was checked against (rails/rails main);
- the app's source defines none of the methods the rewrite takes out or
  newly calls (the unit, its `Duration` constructor, the direction, `sum`,
  `new`, `initialize`; for seconds, minutes and hours also `current`,
  `now`, `since`, `+` and `in_time_zone`; for days, weeks and fortnights
  also `current`, `advance`, `change` and `plus_without_duration`) where it
  could run instead of Active Support's
  ([`passes/duration_definitions.rs`](../../src/emit/fractal/passes/duration_definitions.rs)):
  in a reopening of `Integer`, `Numeric`, `ActiveSupport::Duration`,
  `Time`, `ActiveSupport::TimeWithZone` or `ActiveSupport::TimeZone` (by
  full lexical name, so an app's own `Billing::Duration` does not count),
  on one of them as an explicit receiver or in a block passed to a call on
  one (`Integer.class_eval do`), or in a module mixed into one of them,
  followed through modules that include it. Only the sites relying on a
  defined name are declined.

The scan sees literal names and constant receivers only; a method defined
on a class held in a variable, or under a computed name, is beyond what the
source shows. Before parsing, it reads only files that name `Duration`,
`TimeWithZone` or `TimeZone`, or that name `Integer`, `Numeric` or `Time`
after `class` or `<<`, or before `.`, `&.` or `::`, however spaced (not a
type annotation's `::Integer`, nor a whole-line comment). `Time.now` and
`Time.current` put most of a Rails app's files through that filter, so the
scan parses most of the app once per build. The pass also trusts that
`Time.now` returns a time, which `Duration#sum` checks with `acts_like?`
and the rewrite does not, and that the app leaves Ruby's `Time` and `Date`
and the rest of Active Support's time methods as they are.

Per call, against rails/rails main (f39c8aa) on Ruby 4.0.9 with
`Time.zone = "America/Chicago"`, 500k calls per process and one expression
per process (time is the median of three runs; peak RSS is interpreter to
ZJIT, measured in separate runs without allocation tracing). For each unit
the table shows the chain, the previous rewrite (`::Time.current`), and
this one:

| Expression | Objects / bytes | ns interp / YJIT / ZJIT | Site ISEQ | YJIT code + metadata | ZJIT code + metadata | Peak RSS |
| --- | --- | --- | --- | --- | --- | --- |
| `5.days.ago` | 37 / 2,456 B | 13,237 / 6,969 / 9,635 | 568 B | 416 + 1,167 KB | 368 + 2,102 KB | 25-29 MB |
| `::Time.current.advance(days: -5)` | 34 / 2,096 B | 11,885 / 6,395 / 8,609 | 640 B | 412 + 1,141 KB | 356 + 2,081 KB | 24-29 MB |
| `(::Time.zone ? ::Time.zone.now.plus_without_duration(-432000) : ::Time.now.advance(days: -5))` | 15 / 840 B | 6,502 / 2,616 / 4,089 | 1,048 B | 368 + 1,054 KB | 308 + 2,023 KB | 24-28 MB |
| `5.hours.ago` | 6 / 520 B | 1,943 / 681 / 1,108 | 568 B | 316 + 932 KB | 280 + 1,937 KB | 23-29 MB |
| `::Time.current.since(-18000)` | 4 / 320 B | 1,203 / 307 / 540 | 640 B | 304 + 899 KB | 268 + 1,915 KB | 23-28 MB |
| `(::Time.zone ? ::ActiveSupport::TimeWithZone.new(::Time.now.utc + -18000, ::Time.zone) : ::Time.now + -18000)` | 3 / 240 B | 493 / 219 / 291 | 1,432 B | 288 + 867 KB | 256 + 1,889 KB | 23-28 MB |
| `3.months.from_now` | 42 / 2,656 B | 13,713 / 7,246 / 9,852 | 568 B | 416 + 1,164 KB | 372 + 2,101 KB | 25-29 MB |
| `::Time.current.advance(months: 3)` (unchanged) | 39 / 2,296 B | 13,035 / 7,292 / 9,914 | 640 B | 400 + 1,138 KB | 360 + 2,080 KB | 25-29 MB |

For days, weeks and fortnights, the chain's Duration (40 B) and two hashes
(160 B each) are gone, and so is everything `Time#advance` allocates
between the zone's local time and the wrap: the copy of its options, three
`Date`s (`to_date`, `gregorian` and `Date#advance`), `change`'s hash, the `Rational`s it
builds for the subsecond, and the `Time.utc` they make. Against the chain
that is 22 objects and 1,616 B a call fewer, and 2.4 to 2.7 times faster
under YJIT and ZJIT; against the previous rewrite, 19 objects and 1,256 B
fewer, and 2.1 to 2.4 times faster. Most of the 15 objects left are the
zone's work, which the original does too: `Time.now`, the `TimeWithZone`
it wraps, the local time, Ruby's `+`, TZInfo's two period lookups (a
`Timestamp`, two `Rational`s and a period each, and the array of candidate
periods for the local time) and the result. For seconds, minutes and
hours, the intermediate `TimeWithZone` is gone too: 3 objects and 240 B,
about 70% of the YJIT time and half of the ZJIT time of the previous
rewrite. Nothing is retained per call either way (3-8 live slots after
10,000 calls and a full GC, which is interpreter and JIT state). The JITs
no longer compile `Time#advance`, `#change` and the `Date` methods, so their
code and metadata shrink by 44 + 87 KB and 48 + 58 KB in the days process.
Peak RSS moves by about 1 MB or less. The cost is the call site, which
now holds both branches and the constant lookups: its ISEQ grows from
640 B to 1,048 B for days and 1,432 B for seconds, once per site rather
than per call. Months and years are as before.

[`tests/fractal_lower_duration.rs`](../../tests/fractal_lower_duration.rs)
pins the rewrites and every decline. Where Ruby can load
`active_support/time`, it also builds a model with every unit, direction
and a spread of numbers (576 chains), runs the pass, and compares each
result before and after under `travel_to`: in six zones and with none (so
`Time.current` is the local `Time.now`), at instants whose windows cross
DST changes (Lord Howe's half-hour one among them), a month end and a
leap day. That is 27,648 observations of class, value, offset, `utc?`,
nanoseconds and zone name, and it checks that the comparison fails for the
two seconds lowerings above.

### Removed: `inline-enum-predicates`

The first pass, `inline-enum-predicates`, was removed. It rewrote a call
to an Active Record enum predicate inside the enum's own class
(`published?`) into the comparison Rails' generated method runs
(`(status_for_database == 1)`), because Rails defines each predicate as
a `define_method` block calling `public_send`, which ZJIT cannot call
directly. [rails/rails#59250](https://github.com/rails/rails/pull/59250)
proposes plain `def` predicates so every app can get the benefit without
a build step. The proposal remains open; released Rails versions still
use block-backed predicates. The pass, with its build and runtime tests,
remains in the git history.

## Writing a pass

Implement [`Pass`](../../src/emit/fractal/pass.rs) in a module of
[`passes/`](../../src/emit/fractal/passes/): a name, a one-line
`summary` (what the pass explorer shows), a `scope`, `changes_bytecode`
when its rewrites compile to different bytecode by design (so `verify`
confines the difference to its lines instead of reporting it), and a
`run` that reads a [`Unit`](../../src/emit/fractal/pass.rs) and
returns a `Proposal` of `Edit`s (spans of the unit's file and their
replacement text) and declined sites. [`Context`](../../src/emit/fractal/pass.rs)
gives app-wide facts read from source without ingest: every Ruby file of
the program, a Rubydex index of classes and their ancestors and
descendants, every method name the app defines, and the lockfile's gem
versions. Decline, with a reason, whatever cannot be shown to behave as
the original does for every caller of the compiled app. Register the pass
in `pass::registered()`, pin its rewrites and declines in a build test,
and add a runtime test that compares the original and the rewritten code
in Ruby (`inline-delegate`'s is one to start from; the removed
`inline-enum-predicates` had one against Active Record and SQLite).

## Pass explorer

[`wasm/fractal/`](../../wasm/fractal/) runs the passes in the browser,
on the compiler wasm, over a small app a person can edit: pick the file
to compile and step through the stages, from the source to the file
after each pass, with what each pass rewrote and what it declined and
why. It calls the wasm's `fractal_stages` export, which runs
[`stages.rs`](../../src/emit/fractal/stages.rs): the same selection,
passes and driver as a build, over sources already in memory
(`Context::in_memory`, since the browser has no app on disk), keeping what
each pass added. Stage *n* is the passes `1..=n` applied together, the
way a build applies them. It writes nothing. The explorer shows the
complete selected file at each stage, but `fractal emit` writes only
changed files. Use `fractal apply` to overwrite those files in the base app.

The examples exercise each pass. The Article model has a simple
`delegate` call that `inline-delegate` writes out, and a `prefix:` call
it declines. The Event model has Duration chains in methods and a
`scope` that `lower-duration` rewrites, and ones it declines: a constant's
value, a non-literal receiver, a time passed to `ago`.
[`site/fractal.html`](../../site/fractal.html) introduces Fractal and
links to the explorer.

## JIT shape

Fractal emits Ruby for CRuby's JITs to compile, so its output shape is a
choice. Where YJIT and ZJIT would want different shapes, it takes ZJIT's:
plain `def`s rather than `define_method`, direct calls rather than
`send`, and no singleton classes created on instances. Measuring a pass
means comparing the app with the JIT against the compiled app with the
same JIT, after warmup, for CPU, allocated objects and bytes, GC time and
retained memory, and for boot time when the pass removes boot work.

# Bounded MRI `define_method` observer

Independent, host-only experiment. No exporter/compiler integration, patches to
MRI, new VM, gems, or changes to the generic Ruby hooks are required. Verified
on MRI 3.4.8 (Linux x86-64), with the interpreter and YJIT. The checkout uses
canonical compiler commit `83b6b1458adde2b2db728507fcabc0c2c0557612`; the compiler
is not executed by this proof.

## Build and prove

From the repository root, with MRI development headers, `cc`, and `make`:

```sh
source_dir="$PWD/tools/native-observer"
build_dir=$(mktemp -d)
(cd "$build_dir" && ruby "$source_dir/extconf.rb" && make V=1)
RUBYLIB="$build_dir" ruby "$source_dir/contract.rb"
RUBYLIB="$build_dir" RUBYOPT=--yjit ruby "$source_dir/contract.rb"
rm -rf "$build_dir"
```

The contract spawns four fresh processes executing the same Ruby fixture:
unobserved MRI, extension loaded but inactive, extension active, and an inactive
Ruby-frame wrapper that must fail at callback-visible privacy. It compares
every original/application observation, not just exit status or totals.
The active process additionally checks callable identity, notification order,
GC compaction, and isolated ordinary collector failure. Additional fresh
processes exercise actual TERM cancellation, Interrupt, SystemExit and throw.

Decisive summary (each interpreter/YJIT run):

```text
baseline: 56 native assertions; 0 notifications
inactive: 56 native assertions; 0 notifications
active: 56 native assertions; 18 notifications
Ruby-wrapper negative control: detected private callback becoming public
PASS: collector preserves TERM, Interrupt, SystemExit and throw
PASS: original == native-inactive == native-active
```

The full JSON includes private/protected/public visibility **during**
`method_added`, module-function instance and singleton callbacks, explicit
Proc precedence over an ignored block, asymmetric values −2/4/−31, closure
rebinding, aliases and Method/UnboundMethod copies, name coercion exactly once,
keywords/blocks, argument/type/frozen-receiver errors, and exact application
exception identity. A callback that raises still leaves MRI's installation
intact but produces no success notification. A callback that removes the
method returns MRI's original symbol and is not undone.

Literal blocks retain installed-method lambda behavior: required parameters,
strict arity, no single-array destructuring, and local `return`/`break`, including
the module-function singleton path. `rb_block_proc` exposes a non-lambda Proc
for **provenance**; the installed Method's `to_proc.lambda?` remains true. Do
not use that captured Proc's lambda flag or parameter kinds as the installed
method's ABI, and do not execute it to discover application behavior.

## Collector contract

```ruby
require "native_define_method_observer"
events = []
NativeDefineMethodObserver.collector = ->(owner, name, callable) do
  events << [owner, name, callable]
end
# Trusted bounded boot here.
NativeDefineMethodObserver.collector = nil
# Inspect events/failures or raise exporter errors HERE, outside native calls.
NativeDefineMethodObserver.failures.each { |_, _, error| raise error }
```

Only `Module#define_method` is wrapped, using a C method prepended to Module.
With no collector it simply forwards. With a collector it retains the explicit
second argument (even when a block is present), or captures the one-argument
form's block. No Ruby collector, source parsing, coercion or reflection runs
before `rb_call_super_kw`. Original `argc`, `argv`, keyword flags and current
block are forwarded exactly once. Successful native return is followed by
`collector.call(owner, result_symbol, callable)`, then that same result is
returned. Original native/callback exceptions propagate without rescue.
Collector StandardError exceptions are separately retained as
`[owner, name, exception]` in `failures`; the application's prior `$!`, return,
and installed body survive. Other unwind tags propagate unchanged, including
SignalException/Interrupt, SystemExit and nonlocal throw. The earlier broad
`rb_protect` recovery swallowed TERM during the Mastodon experiment; that
reproduction is now part of the contract, not an accepted cancellation limit.

The C frame matters: MRI's caller-CREF lookup skips C frames, but does not skip
a Ruby prepend frame. Nothing repairs visibility after the fact, and the
application's hooks observe native private/protected/module-function semantics.

**Events describe successful calls, not final installed definitions.** In the
reentrant control, the outer body returns 13, `method_added` installs a body
returning −29, notifications arrive inner-first (−29 then 13), and the final
method returns −29. No definition snapshot is attached to either notification.
Associating the outer callable with `owner.instance_method(name)` after return
would be wrong. An integrating exporter needs its own before/after and
installation snapshots, or must reject/mark opaque ambiguous mutations. This
prototype does not solve that association or silently label the replacement.

## Constraints

- Trusted single-threaded boot only; the collector must be inert except for
  recording provenance. No general non-interference claim for arbitrary
  collector effects, threads, Ractors, cancellation, or allocation failure.
- MRI 3.4.8 is the executed boundary, not JRuby/TruffleRuby/other MRI versions.
  Prepending necessarily changes reflection on Module/ancestors/stack traces.
- No observation of `alias_method`, ordinary `def`, `define_singleton_method`,
  or top-level `define_method`'s separate native implementation. Aliases and
  Method copies in the contract keep working; only define-method calls emit
  events. No full Rails-boot/export coverage is implied.
- Only ordinary collector StandardError is deferred; other unwinds are rethrown
  with `rb_jump_tag`. No exceptions from the original native call are rescued.
- Capturing a block allocates a Proc and notifications/failures allocate Ruby
  objects. Unbounded event retention, memory exhaustion and production
  overhead are not solved or benchmarked.
- Requiring the extension installs the prepend for that process; disabling
  collection does not remove it. Use fresh disposable VMs for controls.

The supplied parent source archive was verified against SHA256
`0607454626deed7338d3403684ba92c9294429bd3f80437543bb972f3275bc26`
before inspection. It is not a dependency of these reusable files.

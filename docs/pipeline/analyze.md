# Analyze

The analyzer annotates every expression in the IR with a type and an
effect set. Two walks — types, then effects — orchestrated by a
whole-program fixpoint.

**Source:** `src/analyze/` — orchestration in `mod.rs`
(`Analyzer::analyze`); body-typer in `src/analyze/body/`; effect walk
in `src/analyze/effects.rs`; `diagnose` in
`src/analyze/diagnostics.rs`.

## The two walks

`mod.rs` is orchestration, not the walks themselves.
`Analyzer::analyze(&mut self, app)` runs the typing passes over the
whole app, then a whole-program fixpoint: harvest inferred return
types from method bodies into the dispatch registry, unify parameter
types across call sites, re-type with the refined registry, and
repeat until a signature fingerprint stabilizes (bounded iterations).
The param table is rebuilt each round rather than accumulated: it is
derived from the types the last pass wrote, and carrying a round-1
observation forward fuses an under-informed `Untyped` into the
converged answer. The loop's last act is a typing pass, so one final
harvest runs after it — otherwise the registry is permanently a round
behind the bodies.
A companion fixpoint (`Analyzer::build_constant_registry`) types
app-level constants — see below. After convergence,
`stamp_inferred_library_signatures` writes what inference discovered
into every library-class `MethodDef.signature` — returns from the
dispatch registry, params from the call-site unification — so emitters
and the emitted `.rbs` sidecars carry it. It never clobbers a
hand-written signature, never stamps `initialize`, and stamps nothing
when it learned nothing.

### Type walk — `BodyTyper`

The body-typer (`BodyTyper` in `src/analyze/body/mod.rs`; public
entry `analyze_expr` — the recursive `compute` is private) is entered
per top-level expression (controller action body, model method body,
scope body, view body, seed program). Returns a `Ty` and, as a side
effect, writes the inferred type onto each sub-expression's
`Expr::ty` field.

Dispatch is by receiver + method name against the analyzer's
registries:

- `class_methods` per `ClassId` — seeded from
  `crate::catalog::AR_CATALOG` at `Analyzer::with_adapter` time, one
  signature per entry with a declared `ReturnKind`.
- `instance_methods` per `ClassId` — seeded from the model's
  attribute row (schema columns → instance method returning the
  column's type) plus any declared instance methods.
- `local_bindings` / `ivar_bindings` in the recursion context —
  extended when the walker sees an assign inside a `Seq`, so a later
  use gets the right type.

**What isn't (yet) inferred:**

- Block-return generics — `def f { () -> T } -> T` style. The
  body-typer doesn't thread the block's return type through `yield`
  in the method body; `yield` types as `Ty::Untyped` (the gradual
  escape) instead of `T`. One carve-out: in a view or layout body,
  `yield` renders content and types `Str` (the Yield arm in
  `src/analyze/body/mod.rs`).
- `super(...)` parent-method tracking — typed `Ty::Untyped`.
- Constants are partially covered. Module-level frozen Hash/Array
  constants in framework Ruby are tracked (`parse_module_constants`
  in `src/runtime_src.rs`), and `Analyzer::build_constant_registry`
  builds a whole-app name→type registry from `CONST = …` assignments
  in model/controller class bodies — with its own small fixpoint, so
  a constant defined in terms of another resolves once its dependency
  does. Constant shapes outside those channels still fall through.

Each gap lands when a fixture forces it; the analyzer never fails, it
either leaves a `Ty::Var(n)` placeholder (inference gap, surfaced as
an `UnresolvedType` Warning) or a `Ty::Untyped` (RBS-declared gradual
escape, surfaced as a `GradualUntyped` Warning).

### Type variants worth knowing about

Beyond the obvious primitives (`Int`, `Str`, `Array<T>`, etc.), the
type system has three special variants:

- **`Ty::Var`** — inference gap. The analyzer couldn't determine a
  type at this position. Surfaces as `DiagnosticKind::UnresolvedType`
  with default severity Warning — a coverage-measurement signal, not
  a hard error (see the variant's doc in `src/diagnostic.rs`).
  `roundhouse-check` gates on Errors (parse errors included), not on
  these.
- **`Ty::Untyped`** — gradual escape. RBS-declared `untyped`, or
  unwrapped propagation through gradual dispatch. Author-signed
  opt-out from checking. Counts as a Warning. Per-target rendering:
  TS `any`, Python `Any`, Rust `()` (fallback; strict targets are
  expected to elevate to Error at emit time), Crystal `_`, Go
  `interface{}`.
- **`Ty::Bottom`** — divergent expression (`raise`, `return`,
  `next`). Subtype of every other type; filtered out in
  `union_of` / `union_many` so `if cond then raise else x end`
  types as `typeof(x)` instead of `typeof(x) | Nil`. Per-target
  rendering: Rust `!`, TS `never`, Python `Never`, Crystal
  `NoReturn`, Go fallback to `interface{}`.

### Effect walk — `collect_effects` / `visit_effects`

Lives in `src/analyze/effects.rs`. Runs after the type walk (the
effect of a Send depends on knowing which table its receiver is bound
to). Every expression ends up with an `EffectSet` on `Expr::effects`.

The walk is straightforward: recurse into children, union their
effect sets, add whatever the current node contributes. The only
non-trivial node is `Send`:

1. Does the receiver type have a bound table? (Yes only for AR model
   classes and instances.)
2. If so, hand the method name to `self.adapter.classify_ar_method`.
3. On `Read` → add `Effect::DbRead { table }`. On `Write` →
   `Effect::DbWrite { table }`. On `Unknown` → nothing.

One more recognizer produces `Effect::Io`: `render` / `redirect_to` /
`head` on a controller receiver (any class matching the Rails
`*Controller` naming convention) — Rails dialect, not adapter
territory. The remaining effect classes (`Time`, `Random`, `Net`,
`Log`, `Raises`) are dormant — no recognizer produces them today.

## How Rails conventions draw type edges

The analyzer isn't inferring types in a vacuum; it's threading them
along edges Rails has already drawn in the source. The most
load-bearing:

| Edge | What flows | Example |
|------|------------|---------|
| schema → model | Column type becomes the instance-method return type | `t.string "title"` in schema.rb makes `article.title : Str` |
| `belongs_to :x` | Instance method `article.user : User` | Foreign-key typed via the Association IR |
| `has_many :xs` | `article.comments : Relation<Comment>` | Resolved via `src/lower/associations.rs` |
| `before_action :m` | The action body is entered with `@post = m()`'s binding in ivar scope | See `src/lower/controller/actions.rs::resolve_before_actions` |
| `render :name` / implicit render | Binds the view to the action's ivars at the concrete types | View body is typed with the controller's ivar scope pre-populated |
| `render "partial"` | Collection-partial rendering types the local from the collection's element type | Implicit `local` binding in `_article.html.erb` is `Article` when invoked as `render @article` |

These are the conventions ruby2js and railcar also leaned on; they're
what make zero-annotation typing viable.

## The diagnostic pipeline

The predicate "this app ingests and every expression has a known
type" is the subset of programs roundhouse can transpile. Enforced
by `analyze::diagnose` and gated in tests via the error/warning
severity split:

```rust
let diagnostics = roundhouse::analyze::diagnose(&app);
let errors: Vec<_> = diagnostics.iter()
    .filter(|d| d.severity == Severity::Error)
    .collect();
assert!(errors.is_empty(), "...");
```

**Each diagnostic carries:**
- `kind: DiagnosticKind` — the structured variant (`ivar_unresolved`,
  `send_dispatch_failed`, `incompatible_binop`, `gradual_untyped`, …)
- `severity: Severity` — `Error` (gates emission), `Warning`
  (informational; per-target emitters may elevate to Error), or
  `Info` (see attribution below)
- `span: Span` and `message: String`

The third severity, `Info`, exists for attribution downgrades:
`src/analyze/attribution.rs` reclassifies diagnostics whose root
cause is a survey-mode ingest gap — not a defect in the user's code —
down to Info with the root cause appended, so consumers render them
as coverage rather than accusations. Genuine findings keep their
original severity.

**Diagnostic kinds and their default severities** (examples, not the
full set — see `DiagnosticKind` in `src/diagnostic.rs` for every
variant):

| Kind | Severity | When |
|------|----------|------|
| `IvarUnresolved` | Error | `@ivar` read with no binding in scope |
| `SendDispatchFailed` | Error | `Send` on a typed receiver where the method doesn't resolve |
| `IncompatibleBinop` | Error | `a OP b` where Ruby would raise at runtime (`Int + Str`, `Hash + Hash`, `1 < "x"`) — annotated by the body-typer at the Send |
| `GradualUntyped` | Warning | An expression resolved to `Ty::Untyped` (RBS gradual escape). Strict-target emitters (Rust, Go) are expected to elevate to Error at emit time |

Two more variants worth knowing: `UnresolvedType` (Warning) is the
silent residue — a `Ty::Var` or never-stamped node at a leaf position
where no more specific diagnostic fires; `MissingPreload` (Warning)
is the static N+1 finding produced by `src/analyze/preload.rs`, which
runs as part of `diagnose`.

**What doesn't produce a diagnostic:**

- A Send whose receiver type is itself unknown — the root cause is
  upstream, and reporting both sites duplicates the signal. Fix the
  upstream site and the downstream one usually resolves.
- Anonymous blocks whose bodies never return to a typed context.

**`roundhouse-check` CLI:** runs ingest + analyze + diagnose on a
Rails app path, prints diagnostics to stderr, and exits non-zero if
any *error* fired. Warnings print but don't gate.

## Why effects are their own walk

Effects depend on types (an `.each` on a known-collection receiver
carries its element's effects; an `.each` on an unknown receiver
can't). Running types first, effects second, is the simplest
ordering — typing iterates to its fixpoint, then effects are a single
pure pass over the already-typed tree, with no lattice joins or
iteration of their own.

Adapters plug in at effect-classification time precisely for this
reason: the type walk is adapter-agnostic, so the same analyzer
produces the same types regardless of which DB backend the generated
project will ship to. The effect walk is where backend semantics
appear. This ordering means you can swap the adapter without
re-running type inference.

## Key public surface

```rust
pub struct Analyzer { /* ... */ }

impl Analyzer {
    pub fn new(app: &App) -> Self;                           // SqliteAdapter default
    pub fn with_adapter(app: &App, adapter: Box<dyn DatabaseAdapter>) -> Self;
    pub fn analyze(&mut self, app: &mut App);                // mutates Expr::ty + Expr::effects
    pub fn class_registry(&self) -> &HashMap<ClassId, ClassInfo>;  // post-fixpoint class table
}

pub fn diagnose(app: &App) -> Vec<Diagnostic>;
```

The analyze→lower seam is `src/session.rs::analyze_and_lower`: build
the analyzer, run it, then apply the shared post-analyze lowerings
against `class_registry()` to reach the emit-ready IR. Only the
emit-bound drivers take that second step — LSP, MCP, `emit_preview`,
and `roundhouse-check` deliberately run analyze *without* the
post-analyze lowerings, because they consume source-shaped IR
(previews, type checks, hovers); `src/session.rs`'s module header
explains the split.

## Extending the analyzer

- **New AR method** — add a catalog entry in `src/catalog/mod.rs`.
  The analyzer picks it up without code changes here.
- **New non-AR method shape** — these live in `src/analyze/registry/`,
  split by domain (`ar`, `activemodel`, `controllers`, `library`,
  `routes`, `stdlib`, `view`). Each submodule owns one
  framework/stdlib surface and populates the class registry that
  `Analyzer::with_adapter` orchestrates; add the signature in the
  submodule that owns the surface.
- **New IR variant** — add cases in both the body-typer's `compute`
  (`src/analyze/body/mod.rs`) and `visit_effects`
  (`src/analyze/effects.rs`). Missing a `visit_effects` arm is
  silent; missing a `compute` arm produces a `Ty::Var` placeholder
  that surfaces as a diagnostic.

## Directory map

Beyond the walks, `src/analyze/` carries single-purpose passes:

- `async_color.rs` — async coloring: seeds deployment-adapter methods
  as `is_async` and propagates, driving TS `async`/`await` emission.
- `attribution.rs` — gap attribution: downgrades diagnostics whose
  root cause is a survey-mode ingest gap to Info.
- `preload.rs` — static N+1 detection (`MissingPreload`), run as part
  of `diagnose`.
- `block_refine.rs` — refines a block-forwarding method's `&block`
  signature slot from the callee's known block signature.
- `mutates_self.rs` — annotates `MethodDef` with whether the body
  writes instance state, driving `&mut self` vs `&self` on strict
  targets.
- `render.rs` — render-site/partial resolution: which partials a view
  renders, the locals they receive, controller action → view-name
  mapping (the machinery behind the render rows in the table above).
- `inferred_types.rs` — harvest of every stamped type plus its span
  (the playground hover surface).
- `registry/` — per-domain class-registry population (see "Extending
  the analyzer").
- `body/send.rs` — Send dispatch: receiver `ClassInfo` lookup, the
  primitive method tables, block-parameter seeding.

## Key files

| File | Role |
|------|------|
| `src/analyze/mod.rs` | Orchestration: `Analyzer`, registry seeding, the typing/fixpoint loop |
| `src/analyze/body/` | Body-typer (recursive `analyze_expr`, dispatch tables, narrowing) |
| `src/analyze/effects.rs` | Effect walk (`collect_effects` / `visit_effects`) |
| `src/analyze/diagnostics.rs` | `diagnose` / `diagnose_with_coverage` |
| `src/diagnostic.rs` | `Diagnostic`, `DiagnosticKind`, `Severity` |
| `src/adapter.rs` | Backend seam — `classify_ar_method` |
| `src/catalog/mod.rs` | AR method signatures the walks consume |
| `src/effect.rs` | `Effect` enum + `EffectSet` |
| `src/ty.rs` | `Ty` (with `Untyped` and `Bottom`), `Row`, `Param` |
| `src/runtime_src.rs` | Framework-Ruby ingestion (RBS-paired) + module-level constant tracking |
| `src/rbs.rs` | RBS sidecar parsing — signatures, includes, `%a{abstract}` annotation |

## Related docs

- [`../data/catalog.md`](../data/catalog.md) — where method signatures
  live.
- [`../data/adapter.md`](../data/adapter.md) — the effect-classification
  seam.
- [`lower.md`](lower.md) — what the lowered form consumes the analyzed
  IR for.

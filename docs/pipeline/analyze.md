# Analyze

The analyzer annotates every expression in the IR with a type and an
effect set. Two walks, one result.

**Source:** `src/analyze/` (`Analyzer`, `compute`, `visit_effects`,
`diagnose`); body-typer in `src/analyze/body/`.

## The two walks

### Type walk — `Analyzer::compute`

Entered per top-level expression (controller action body, model
method body, scope body, view body, seed program). Returns a `Ty`
and, as a side effect, writes the inferred type onto each
sub-expression's `Expr::ty` field.

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
  escape) instead of `T`.
- `super(...)` parent-method tracking — typed `Ty::Untyped`.
- Module-level frozen Hash/Array constants are tracked
  (`parse_module_constants` in `src/runtime_src.rs`); other constant
  shapes still fall through.

Each gap lands when a fixture forces it; the analyzer never fails, it
either leaves a `Ty::Var(n)` placeholder (inference gap, surfaced as
an Error diagnostic) or a `Ty::Untyped` (RBS-declared gradual escape,
surfaced as a Warning).

### Type variants worth knowing about

Beyond the obvious primitives (`Int`, `Str`, `Array<T>`, etc.), the
type system has three special variants:

- **`Ty::Var`** — inference gap. The analyzer couldn't determine a
  type at this position. Counts as an Error in the diagnostic
  pipeline; failing to close means `roundhouse-check` fails.
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

### Effect walk — `Analyzer::visit_effects`

Runs after the type walk (the effect of a Send depends on knowing
which table its receiver is bound to). Every expression ends up with
an `EffectSet` on `Expr::effects`.

The walk is straightforward: recurse into children, union their
effect sets, add whatever the current node contributes. The only
non-trivial node is `Send`:

1. Does the receiver type have a bound table? (Yes only for AR model
   classes and instances.)
2. If so, hand the method name to `self.adapter.classify_ar_method`.
3. On `Read` → add `Effect::DbRead { table }`. On `Write` →
   `Effect::DbWrite { table }`. On `Unknown` → nothing.

All other effect classes (`Io`, `Time`, `Random`, `Net`, `Log`,
`Raises`) are dormant — no recognizer produces them today.

## How Rails conventions draw type edges

The analyzer isn't inferring types in a vacuum; it's threading them
along edges Rails has already drawn in the source. The most
load-bearing:

| Edge | What flows | Example |
|------|------------|---------|
| schema → model | Column type becomes the instance-method return type | `t.string "title"` in schema.rb makes `article.title : Str` |
| `belongs_to :x` | Instance method `article.user : User` | Foreign-key typed via the Association IR |
| `has_many :xs` | `article.comments : Relation<Comment>` | Resolved via `src/lower/associations.rs` |
| `before_action :m` | The action body is entered with `@post = m()`'s binding in ivar scope | See `src/lower/controller.rs::resolve_before_actions` |
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
  `send_dispatch_failed`, `incompatible_binop`, `gradual_untyped`)
- `severity: Severity` — `Error` (gates emission) or `Warning`
  (informational; per-target emitters may elevate to Error)
- `span: Span` and `message: String`

**Diagnostic kinds and their default severities:**

| Kind | Severity | When |
|------|----------|------|
| `IvarUnresolved` | Error | `@ivar` read with no binding in scope |
| `SendDispatchFailed` | Error | `Send` on a typed receiver where the method doesn't resolve |
| `IncompatibleBinop` | Error | `a OP b` where Ruby would raise at runtime (`Int + Str`, `Hash + Hash`, `1 < "x"`) — annotated by the body-typer at the Send |
| `GradualUntyped` | Warning | An expression resolved to `Ty::Untyped` (RBS gradual escape). Strict-target emitters (Rust, Go) are expected to elevate to Error at emit time |

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
ordering — no fixed-point iteration, no lattice joins, just a pure
second pass over the already-typed tree.

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
    pub fn analyze(&self, app: &mut App);                    // mutates Expr::ty + Expr::effects
}

pub fn diagnose(app: &App) -> Vec<Diagnostic>;
```

## Extending the analyzer

- **New AR method** — add a catalog entry in `src/catalog/mod.rs`.
  The analyzer picks it up without code changes here.
- **New non-AR method shape** — today each of these is declared
  inline inside `Analyzer::with_adapter` (controller helpers,
  ActiveModel helpers). Migration into the catalog is ongoing.
- **New IR variant** — add cases in both `compute` and
  `visit_effects`. Missing an `visit_effects` arm is silent; missing
  a `compute` arm produces a `Ty::Var` placeholder that surfaces as
  a diagnostic.

## Key files

| File | Role |
|------|------|
| `src/analyze/mod.rs` | Both walks, registry seeding, `diagnose` |
| `src/analyze/body/` | Body-typer (recursive `analyze_expr`, dispatch tables, narrowing) |
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

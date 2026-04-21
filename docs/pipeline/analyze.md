# Analyze

The analyzer annotates every expression in the IR with a type and an
effect set. Two walks, one result.

**Source:** `src/analyze.rs` (`Analyzer`, `compute`, `visit_effects`,
`diagnose`).

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

- Cross-branch unification (`if`/`case` merging types).
- Row-polymorphic parameter types.
- Generic instantiation beyond `Array<Post>`-style.
- Instance method dispatch on ivars/locals whose types aren't
  trivially known.

Each gap lands when a fixture forces it; the analyzer never fails, it
just leaves a `Ty::Var(n)` placeholder where it couldn't resolve.

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

## The zero-diagnostics contract

The predicate "this app ingests and every expression has a known
type" is the subset of programs roundhouse can transpile. Enforced
by `analyze::diagnose` and gated in tests:

```rust
let diagnostics = roundhouse::analyze::diagnose(&app);
assert!(diagnostics.is_empty(), "...");
```

**What produces a diagnostic:**

- `IvarUnresolved` — an `@ivar` read with no binding in scope and no
  type the analyzer could derive.
- `SendDispatchFailed` — a `Send` whose receiver's type is *known*
  but the method doesn't resolve against that type's registries.
- `LValueUnknown` — an assignment target with an unresolvable kind.
- `LiteralUnknown` — a literal the type walk couldn't classify.

**What doesn't produce a diagnostic:**

- A Send whose receiver type is itself unknown — the root cause is
  upstream, and reporting both sites duplicates the signal. Fix the
  upstream site and the downstream one usually resolves.
- Anonymous blocks whose bodies never return to a typed context.

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
| `src/analyze.rs` | Both walks, registry seeding, `diagnose` |
| `src/adapter.rs` | Backend seam — `classify_ar_method` |
| `src/catalog/mod.rs` | AR method signatures the walks consume |
| `src/effect.rs` | `Effect` enum + `EffectSet` |
| `src/ty.rs` | `Ty`, `Row`, `Param` |

## Related docs

- [`../data/catalog.md`](../data/catalog.md) — where method signatures
  live.
- [`../data/adapter.md`](../data/adapter.md) — the effect-classification
  seam.
- [`lower.md`](lower.md) — what the lowered form consumes the analyzed
  IR for.

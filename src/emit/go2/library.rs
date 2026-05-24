//! Generic LibraryClass → Go emit.
//!
//! Mirrors `src/emit/rust2/library.rs` but emits Go. Couples the
//! function-decl shape (`render_params` + `render_return`) with the
//! body walker in `super::expr` to produce real method bodies for
//! variants the walker covers. Unhandled `ExprNode` variants surface
//! as `/* TODO: emit ... */` comments inside the body — visible to
//! `go build` against the v2/ overlay, which is the inventory loop
//! for widening walker coverage one variant at a time.
//!
//! Output shape:
//! - Each LibraryClass becomes `type <Name> struct {}` plus one
//!   `func (*<Name>) <method>(args) ret { <body> }` per method.
//! - Modules (Mode::Module) become a bag of `func <name>(args) ret { <body> }`.
//! - Constants emit as `var <NAME> interface{} = nil` placeholders
//!   (Phase 2+: real const renderer over `Expr`).
//!
//! Param + return types render via `super::ty::go_ty_stub` — a
//! permissive variant that returns `interface{}` for unknown shapes
//! and concrete Go types (`int64`, `string`, ...) for known
//! primitives. Per-param Tys come from the method's `signature:
//! Option<Ty::Fn>` when present.

use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver};
use crate::expr::Expr;
use crate::ty::{ParamKind, Ty};

use crate::emit::go::shared::go_field_name;

use super::expr::{emit_return_body, EmitCtx};
use super::ty::go_ty_stub;

pub fn emit_library_class(class: &LibraryClass) -> Result<String, String> {
    emit_library_class_with_registry(class, &std::collections::HashSet::new())
}

/// Variant that takes a `variadic_ctors` set so synthesized embedded-
/// parent constructor calls match the parent's actual ctor shape.
/// Without this, `NewApplicationController(_opts...)` forwards into
/// `NewActionControllerBase(_opts...)` even though the latter is
/// non-variadic (Ruby `def initialize` with no params → Go no-arg
/// ctor). Build the set in `go2.rs::emit_overlay_files` by walking
/// every LC (runtime + models + controllers) and checking whether
/// its `initialize` MethodDef has a trailing-optional param.
pub fn emit_library_class_with_registry(
    class: &LibraryClass,
    variadic_ctors: &std::collections::HashSet<String>,
) -> Result<String, String> {
    // Module-singleton shape: a Ruby `module X` whose body is just
    // `class << self; attr_accessor :slot; end` (and/or `def self.foo`
    // methods). All methods are Class receivers; no instances exist.
    // Go analog is a unit struct + per-slot package var + bare
    // accessor functions — distinct enough from the per-instance
    // struct shape that it gets its own emit path. Detection matches
    // rust2's `is_module_singleton` predicate.
    let is_module_singleton = class.is_module
        && !class.methods.is_empty()
        && class
            .methods
            .iter()
            .all(|m| matches!(m.receiver, MethodReceiver::Class));
    if is_module_singleton {
        return emit_module_singleton(class);
    }

    let name = sanitize_type_name(class.name.0.as_str());
    let mut out = String::new();

    // Q1 — inheritance via embedding. When the class has a parent
    // (Article < ApplicationRecord, ApplicationRecord < ActiveRecord::Base),
    // emit the parent as an anonymous `*Parent` field so Go's method
    // promotion delivers Base's instance methods (`MarkPersistedBang`,
    // `Save`, `Destroy`, …) on the subclass automatically. The chain
    // composes — ApplicationRecord embeds *ActiveRecordBase, Article
    // embeds *ApplicationRecord, and `article.MarkPersistedBang()`
    // resolves through both promotions.
    let embedded_parent = embedded_parent_type(class);

    // Discover the struct's field layout. `attr_reader` / `attr_writer`
    // methods synthesize MethodDefs whose signature carries the field
    // type — those become the exported struct fields. Initialize-only
    // ivars (assigned but no reader/writer) aren't reflected as Go
    // fields yet; they'd surface as missing-symbol errors at use,
    // which is fine inventory.
    let fields = collect_fields(&class.methods);
    if embedded_parent.is_none() && fields.is_empty() {
        out.push_str(&format!("type {name} struct{{}}\n\n"));
    } else {
        out.push_str(&format!("type {name} struct {{\n"));
        if let Some(ref parent_ty) = embedded_parent {
            out.push_str(&format!("\t*{parent_ty}\n"));
        }
        for f in &fields {
            out.push_str(&format!("\t{} {}\n", f.pascal_name, f.go_ty));
        }
        out.push_str("}\n\n");
    }

    // Constructor synthesis. When an `initialize` method is present,
    // emit `New<Name>(...)` returning a pointer to the struct,
    // populated via field-by-field assignment. The original
    // `initialize` method is NOT emitted as a method on the type;
    // its body becomes the constructor body. When the class is a
    // parent-embedding shape with NO `initialize` of its own
    // (ApplicationRecord), synthesize a pass-through constructor so
    // subclasses can build the embedded slot via `NewParent(_opts...)`.
    let parent_is_variadic = embedded_parent
        .as_deref()
        .map(|p| variadic_ctors.contains(p))
        .unwrap_or(false);
    if let Some(init) = class.methods.iter().find(|m| {
        matches!(m.receiver, MethodReceiver::Instance) && m.name.as_str() == "initialize"
    }) {
        out.push_str(&emit_constructor(
            &name,
            init,
            embedded_parent.as_deref(),
            parent_is_variadic,
        ));
        out.push('\n');
    } else if let Some(ref parent_ty) = embedded_parent {
        out.push_str(&emit_default_embedded_constructor(
            &name,
            parent_ty,
            parent_is_variadic,
        ));
        out.push('\n');
    }

    // Build the self-method registry: Ruby names of real (non-attr)
    // instance methods on this class. Consumed by `emit_send` (via
    // `EmitCtx.self_methods`) to decide whether `self.foo` emits as
    // a method call (`self.Foo()`) or a field read (`self.Foo`).
    // attr_reader/writer-backed slots are NOT in the set — those
    // are struct fields and the parenless read is the right shape.
    // Class methods aren't included either; implicit-self calls to
    // them inside other class methods route through the existing
    // SelfRef-in-class-method bare-fn path (`ClassName_method()`).
    let self_methods = collect_self_methods(&class.methods, &fields);

    for m in &class.methods {
        // Skip attr_reader / attr_writer (now fields) and the
        // initialize method (now NewClass).
        if matches!(
            m.kind,
            AccessorKind::AttributeReader | AccessorKind::AttributeWriter
        ) {
            continue;
        }
        if matches!(m.receiver, MethodReceiver::Instance) && m.name.as_str() == "initialize" {
            continue;
        }
        // Skip trivial-reader instance methods (`def errors; @errors;
        // end`) when their name collides with a struct field. The
        // field already serves as the Go reader — defining a same-
        // named method would produce a `field and method with the
        // same name` vet error. Non-trivial bodies (anything other
        // than a single matching Ivar read) keep emitting so behavior
        // isn't silently dropped.
        if matches!(m.receiver, MethodReceiver::Instance) && is_trivial_ivar_reader(m) {
            let stem = m.name.as_str().trim_end_matches(['?', '!', '=']);
            if fields.iter().any(|f| f.ruby_name == stem) {
                continue;
            }
        }
        out.push_str(&emit_method(&name, m, &self_methods));
        out.push('\n');
    }

    // Per-model class-method wrappers for the public AR::Base class
    // API (`<Model>.find`, `<Model>.exists?`, `<Model>.all`, etc.).
    // Go embedding promotes INSTANCE methods only — bare-fn class
    // methods on Base (`ActiveRecordBase_find` etc.) don't appear
    // under the subclass's `<Model>_<method>` prefix. The lowerer
    // already emits the per-model `_adapter_*` primitives; these
    // wrappers delegate to them so call sites like
    // `Article.exists?(id)` → `Article_exists_p(id)` resolve.
    // Mirrors rust2's per-model `pub fn exists/find/count/all/...`
    // shim block (see fixtures/real-blog transpiled rust output).
    // Only fires when the class inherits from AR::Base (transitively)
    // AND has the lowerer-emitted per-model adapter primitives
    // (`_adapter_find_by_id`, etc.). Concrete models (Article, Comment)
    // have them; abstract intermediates (ApplicationRecord) don't, and
    // emitting wrappers there would reference undefined symbols.
    if embedded_parent.is_some() && has_adapter_primitives(class) {
        out.push_str(&emit_ar_class_method_wrappers(&name));
    }
    Ok(out)
}

/// True when the lowerer synthesized per-model `_adapter_*` class
/// methods (the marker for concrete schema-backed models). Detection
/// keys on `_adapter_find_by_id` — every concrete model gets it via
/// `adapter_emit::push_adapter_methods`; abstract intermediates and
/// non-AR LibraryClasses don't.
fn has_adapter_primitives(class: &LibraryClass) -> bool {
    class.methods.iter().any(|m| {
        matches!(m.receiver, MethodReceiver::Class)
            && m.name.as_str() == "_adapter_find_by_id"
    })
}

/// Emit per-model wrappers for the public AR::Base class API. Each
/// wrapper delegates to the lowerer-synthesized `<Model>__adapter_*`
/// primitive that already exists on the model. Naming matches
/// `sanitize_method_name` so call sites that target
/// `<Model>.<ruby_method_name>` route here cleanly:
///
/// - `Model.exists?(id)` → `Model_exists_p(id)`
/// - `Model.find(id)` → `Model_find(id)` (panic on nil)
/// - `Model.all` → `Model_all()`
/// - `Model.count` → `Model_count()`
/// - `Model.last` → `Model_last()`
/// - `Model.destroy_all` → `Model_destroy_all()`
/// - `Model.create(attrs)` / `Model.create!(attrs)` → `New<Model>(attrs).save()`
///
/// `Model.where(conditions)` is intentionally omitted — uses the
/// global `ActiveRecord.adapter.Where` path which isn't per-model
/// adapter-emit-friendly. Add when a forcing call site appears.
fn emit_ar_class_method_wrappers(name: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("// AR::Base class API delegates — Go embedding doesn't promote\n"));
    out.push_str(&format!("// bare-fn class methods, so wrap the per-model _adapter_*\n"));
    out.push_str(&format!("// primitives the lowerer already emitted.\n"));
    out.push_str(&format!(
        "func {name}_exists_p(id int64) bool {{ return {name}__adapter_exists_by_id_p(id) }}\n"
    ));
    out.push_str(&format!(
        "func {name}_find(id int64) *{name} {{\n\tresult := {name}__adapter_find_by_id(id)\n\tif result == nil {{ panic(\"Couldn't find {name} with id=\") }}\n\treturn result\n}}\n"
    ));
    out.push_str(&format!(
        "func {name}_all() []*{name} {{ return {name}__adapter_all() }}\n"
    ));
    out.push_str(&format!(
        "func {name}_count() int64 {{ return {name}__adapter_count() }}\n"
    ));
    out.push_str(&format!(
        "func {name}_last() *{name} {{\n\trecords := {name}__adapter_all()\n\tif len(records) == 0 {{ return nil }}\n\treturn records[len(records)-1]\n}}\n"
    ));
    out.push_str(&format!(
        "func {name}_destroy_all() []*{name} {{\n\trecords := {name}__adapter_all()\n\tfor _, r := range records {{ r.Destroy() }}\n\treturn records\n}}\n"
    ));
    out.push_str(&format!(
        "func {name}_create(attrs map[string]interface{{}}) *{name} {{\n\tinstance := New{name}(attrs)\n\tinstance.Save()\n\treturn instance\n}}\n"
    ));
    out.push_str(&format!(
        "func {name}_create_bang(attrs map[string]interface{{}}) *{name} {{\n\tinstance := New{name}(attrs)\n\tif !instance.Save() {{ panic(\"RecordInvalid\") }}\n\treturn instance\n}}\n"
    ));
    out
}

/// Collect the names of real (non-attr) instance methods on a
/// class. Class methods are excluded because implicit-self calls to
/// them inside another method body route through the bare-fn path
/// (`ClassName_method()`), not through receiver-shaped dispatch.
fn collect_self_methods(
    methods: &[crate::dialect::MethodDef],
    fields: &[Field],
) -> std::rc::Rc<std::collections::HashSet<String>> {
    let mut set: std::collections::HashSet<String> = std::collections::HashSet::new();
    for m in methods {
        if !matches!(m.receiver, MethodReceiver::Instance) {
            continue;
        }
        if matches!(
            m.kind,
            AccessorKind::AttributeReader | AccessorKind::AttributeWriter
        ) {
            continue;
        }
        // Trivial-reader instance methods whose name matches a struct
        // field aren't emitted (the field IS the reader). Exclude
        // them from self_methods too so `self.foo` call sites emit as
        // a field read (`self.Foo`), not a method call (`self.Foo()`).
        // The predicate-suffix strip mirrors `is_trivial_ivar_reader`.
        if is_trivial_ivar_reader(m) {
            let stem = m.name.as_str().trim_end_matches(['?', '!', '=']);
            if fields.iter().any(|f| f.ruby_name == stem) {
                continue;
            }
        }
        set.insert(m.name.as_str().to_string());
    }
    std::rc::Rc::new(set)
}

/// One Go struct field derived from a Ruby `attr_reader` / `attr_writer`.
struct Field {
    /// PascalCase, Go-style field name (`Verb`, `PathParams`, `ID`).
    pascal_name: String,
    /// Original Ruby ivar name without `@` (`verb`, `path_params`) —
    /// used to look up Ivar references at emit time.
    ruby_name: String,
    go_ty: String,
}

/// Walk methods and gather one Field per attr_reader / attr_writer,
/// plus any ivars that `initialize` writes but no accessor exposes
/// (state fields like AR::Base's `@persisted`, `@destroyed`,
/// `@errors`). The signature's return type (reader) or single param
/// type (writer) provides the Go type for accessor-backed fields;
/// initialize-only ivars infer from the assigned value's Ty.
fn collect_fields(methods: &[MethodDef]) -> Vec<Field> {
    let mut out: Vec<Field> = Vec::new();
    for m in methods {
        // Class-receiver attrs are module-singleton slots, not
        // per-instance fields. The module-singleton emit path
        // handles them separately; here we keep struct fields
        // strictly per-instance so a class with mixed receivers
        // doesn't accidentally lift a class-level slot into the
        // struct shape.
        if matches!(m.receiver, MethodReceiver::Class) {
            continue;
        }
        let name = m.name.as_str().trim_end_matches('=').to_string();
        let go_ty = match m.kind {
            AccessorKind::AttributeReader => {
                if let Some(Ty::Fn { ret, .. }) = m.signature.as_ref() {
                    go_ty_stub(Some(ret))
                } else {
                    "interface{}".to_string()
                }
            }
            AccessorKind::AttributeWriter => {
                if let Some(Ty::Fn { params, .. }) = m.signature.as_ref() {
                    params
                        .first()
                        .map(|p| go_ty_stub(Some(&p.ty)))
                        .unwrap_or_else(|| "interface{}".to_string())
                } else {
                    "interface{}".to_string()
                }
            }
            _ => continue,
        };
        if out.iter().any(|f| f.ruby_name == name) {
            continue;
        }
        out.push(Field {
            pascal_name: go_field_name(&name),
            ruby_name: name,
            go_ty,
        });
    }
    // Second pass: walk EVERY instance method's body for `@ivar =
    // expr` assignments to fields we haven't seen yet. Type comes
    // from the assigned value's `Expr.ty` when present; falls back
    // to `interface{}` so we never block on unknown. Controllers
    // synthesize ivars across action methods (`@articles = Article.all`
    // in Index, `@article = Article.find(...)` in Show) — without
    // walking each body, these references vet-fail as undefined
    // struct fields. The first-write-wins semantics mean later
    // ivar writes with a wider Ty don't override (consistent with
    // initialize-only ivars taking their first-assigned value).
    for m in methods {
        if !matches!(m.receiver, MethodReceiver::Instance) {
            continue;
        }
        collect_ivar_writes(&m.body, &mut out);
    }
    out
}

/// `def foo; @foo; end` — body is exactly a single Ivar read whose
/// name matches the method. The field synthesized from initialize's
/// `@foo = ...` write already serves as the Go reader, so emitting
/// the method on top of it would collide. Detect this de-facto
/// attr_reader shape and let the field stand alone.
fn is_trivial_ivar_reader(m: &MethodDef) -> bool {
    use crate::expr::ExprNode;
    // Predicate-suffixed reader: `def persisted?; @persisted; end`.
    // Ivar names never carry `?`/`!`/`=` — strip suffixes from the
    // method name before comparing.
    let raw = m.name.as_str();
    let name = raw.trim_end_matches(['?', '!', '=']);
    let body = &m.body;
    let single = match &*body.node {
        ExprNode::Seq { exprs } if exprs.len() == 1 => &exprs[0],
        ExprNode::Seq { .. } => return false,
        _ => body,
    };
    match &*single.node {
        ExprNode::Ivar { name: ivar_name } => ivar_name.as_str() == name,
        _ => false,
    }
}

/// Walk an Expr tree collecting top-level `@ivar = value` writes and
/// adding them to `fields` if not already present.
fn collect_ivar_writes(body: &Expr, fields: &mut Vec<Field>) {
    use crate::expr::{ExprNode, LValue};
    match &*body.node {
        ExprNode::Seq { exprs } => {
            for e in exprs {
                collect_ivar_writes(e, fields);
            }
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            let ruby_name = name.as_str().to_string();
            if fields.iter().any(|f| f.ruby_name == ruby_name) {
                return;
            }
            let go_ty = go_ty_stub(value.ty.as_ref());
            fields.push(Field {
                pascal_name: go_field_name(&ruby_name),
                ruby_name,
                go_ty,
            });
        }
        _ => {}
    }
}

/// Emit `func New<Name>(params...) *<Name> { return &<Name>{...} }`
/// from an `initialize` MethodDef. Handles the simple shape where
/// each body Assign is `@<name> = <var>` — fields populate directly
/// from the matching positional param. Falls back to a build-then-
/// assign form when the body shape is more complex.
fn emit_constructor(
    class_name: &str,
    init: &MethodDef,
    embedded_parent: Option<&str>,
    parent_is_variadic: bool,
) -> String {
    let (params, optional_unpack) = render_constructor_params(init);
    let mut out = format!("func New{class_name}({params}) *{class_name} {{\n");

    // Inject the embedded-parent constructor call when present —
    // `&Article{ApplicationRecord: NewApplicationRecord(_opts...)}`
    // initializes the embedded slot so promoted methods see a non-nil
    // receiver. Only forward `_opts...` when BOTH the current ctor is
    // variadic AND the parent ctor is variadic. Forwarding into a
    // non-variadic parent is a vet error; calling a variadic parent
    // with no args is fine (zero variadic slots).
    let parent_slot = embedded_parent.map(|p| {
        let forward = if params.contains("_opts ...") && parent_is_variadic {
            "_opts..."
        } else {
            ""
        };
        format!("{p}: New{p}({forward})")
    });

    // Try the simple-shape detection: every body expr is
    // `Assign { target: Ivar(name), value: Var(name) }`, and the Var
    // name matches the Ivar name. If so, emit `return &Class{Name: name, ...}`.
    if let Some(literal) = try_field_init_literal(class_name, &init.body, parent_slot.as_deref()) {
        out.push_str(&optional_unpack);
        out.push_str(&format!("\treturn {literal}\n"));
    } else {
        // Fallback: declare a fresh receiver, walk the body as
        // statements (NOT return-wrapped — `self.X = Y` is a Go
        // statement, not an expression), then return `self`.
        // `void_method=true` on the ctx makes the body walker emit
        // each tail as a statement instead of `return X`.
        let parent_init = parent_slot
            .as_deref()
            .map(|s| s.to_string())
            .unwrap_or_default();
        out.push_str(&format!("\tself := &{class_name}{{{parent_init}}}\n"));
        out.push_str(&optional_unpack);
        let mut ctx = EmitCtx::none();
        ctx.void_method = true;
        for p in &init.params {
            ctx.declare_param(p.name.as_str());
        }
        // Ruby `initialize` is void, so `return if other.nil?` (the
        // skip-the-rest idiom) is normal. Lowered to ExprNode::Return
        // with Nil value, naive emit produces `return nil` — but in the
        // outer ctor (`func New<X>() *<X>`) that returns a nil
        // pointer, not "skip the rest then return self". Detect early
        // returns and wrap the body in an IIFE: `return` exits the
        // closure but the outer fn still runs `return self`. `self`
        // and the param-unpack locals are captured by reference. The
        // closure adds no runtime cost the Go inliner can't elide.
        let wrap_in_iife = body_has_return(&init.body);
        if wrap_in_iife {
            // Inside the IIFE the return type is void, so the Return
            // emit produces bare `return` (truly_void path) instead of
            // `return nil`.
            ctx.return_ty = Some(crate::ty::Ty::Nil);
        }
        // Constructor body executes against `self` directly, just
        // like an instance method would. Keep `in_class_method=false`
        // so `@ivar = …` writes resolve to `self.Field`.
        let body = emit_return_body(&ctx, &init.body);
        if wrap_in_iife {
            out.push_str("\tfunc() {\n");
            out.push_str(&body);
            out.push_str("\t}()\n");
        } else {
            out.push_str(&body);
        }
        out.push_str("\treturn self\n");
    }
    out.push_str("}\n");
    out
}

/// Render a constructor's param list. Trailing params with default
/// values fold into a single variadic catch-all (`opts ...T`); the
/// caller (`emit_constructor`) injects per-param unpack at the top
/// of the body. Returns `(param_list, unpack_block)`.
///
/// Ruby `def initialize(other = nil)` → Go `func NewX(opts ...map)`
/// + body `var other map; if len(opts) > 0 { other = opts[0] }`.
/// This is the Go idiom for optional positional args; the caller's
/// `NewX()` (no args) and `NewX(theOther)` both resolve.
///
/// Constraints: Go only allows ONE variadic, and it must be the
/// last param. So only TRAILING defaulted params fold. A
/// required-then-optional-then-required shape can't be expressed in
/// Go and falls back to non-variadic emit (caller must always pass).
fn render_constructor_params(m: &MethodDef) -> (String, String) {
    let sig_tys = signature_param_tys(m);
    let split = trailing_optional_split(&m.params);
    let mut params_out: Vec<String> = Vec::new();
    let mut unpack = String::new();
    for (i, p) in m.params.iter().enumerate() {
        let ty = sig_tys.as_ref().and_then(|tys| tys.get(i));
        if i < split {
            params_out.push(format!("{} {}", sanitize(p.name.as_str()), go_ty_stub(ty)));
        } else if i == split {
            let go_ty = go_ty_stub(ty);
            params_out.push(format!("_opts ...{go_ty}"));
            // Per-param unpack — first variadic slot = the original
            // param's name, subsequent params (also defaulted) pull
            // from successive slots.
            for (j, opt) in m.params[split..].iter().enumerate() {
                // Ruby's `_X` convention signals an intentionally
                // unused parameter. Go allows unused PARAMS (not
                // unused LOCALS), and our variadic emit lowers
                // optional params to locals. Skip the unpack for
                // `_`-prefixed names so vet doesn't error on the
                // unused declaration — the body never references
                // them anyway.
                if opt.name.as_str().starts_with('_') {
                    continue;
                }
                let opt_ty = sig_tys.as_ref().and_then(|tys| tys.get(split + j));
                let opt_go_ty = go_ty_stub(opt_ty);
                let opt_name = sanitize(opt.name.as_str());
                unpack.push_str(&format!("\tvar {opt_name} {opt_go_ty}\n"));
                unpack.push_str(&format!("\tif len(_opts) > {j} {{ {opt_name} = _opts[{j}] }}\n"));
            }
            break;
        }
    }
    (params_out.join(", "), unpack)
}

/// Index of the first trailing-defaulted param. Returns
/// `m.params.len()` when there are no trailing-defaulted params.
fn trailing_optional_split(params: &[crate::dialect::Param]) -> usize {
    let mut split = params.len();
    for (i, p) in params.iter().enumerate().rev() {
        if p.default.is_some() {
            split = i;
        } else {
            break;
        }
    }
    split
}

/// Pattern-match the simple-shape constructor body (`Seq` of
/// Recursive walk: does `e` contain an `ExprNode::Return` anywhere?
/// Used by `emit_constructor` to decide whether to wrap the ctor body
/// in an IIFE so the Ruby `return if cond` skip-the-rest idiom doesn't
/// short-circuit the outer `New<X>` and yield a nil pointer.
fn body_has_return(e: &Expr) -> bool {
    use crate::expr::{ExprNode, LValue};
    match &*e.node {
        ExprNode::Return { .. } => true,
        ExprNode::Seq { exprs } => exprs.iter().any(body_has_return),
        ExprNode::If { cond, then_branch, else_branch } => {
            body_has_return(cond)
                || body_has_return(then_branch)
                || body_has_return(else_branch)
        }
        ExprNode::Case { scrutinee, arms } => {
            body_has_return(scrutinee)
                || arms.iter().any(|a| {
                    a.guard.as_ref().map(body_has_return).unwrap_or(false)
                        || body_has_return(&a.body)
                })
        }
        ExprNode::Assign { target, value } => {
            let target_has = match target {
                LValue::Attr { recv, .. } | LValue::Index { recv, .. } => body_has_return(recv),
                _ => false,
            };
            target_has || body_has_return(value)
        }
        ExprNode::Send { recv, args, block, .. } => {
            recv.as_ref().map(|r| body_has_return(r)).unwrap_or(false)
                || args.iter().any(body_has_return)
                || block.as_ref().map(|b| body_has_return(b)).unwrap_or(false)
        }
        ExprNode::Cast { value, .. } => body_has_return(value),
        // Loops + Lambda bodies have their own return semantics —
        // a `return` inside a `Lambda` exits the lambda, not the
        // enclosing ctor, so we don't need IIFE wrapping for those.
        ExprNode::Lambda { .. } => false,
        _ => false,
    }
}

/// `@ivar = var` assigns where `var` matches the ivar's Pascal field).
/// Returns the struct literal text on success. `parent_slot`, when
/// provided, prepends the embedded-parent initializer
/// (`ApplicationRecord: NewApplicationRecord(_opts...)`) ahead of the
/// per-ivar bindings.
fn try_field_init_literal(
    class_name: &str,
    body: &Expr,
    parent_slot: Option<&str>,
) -> Option<String> {
    use crate::expr::{ExprNode, LValue};
    let exprs = match &*body.node {
        ExprNode::Seq { exprs } => exprs.as_slice(),
        _ => std::slice::from_ref(body),
    };
    let mut bindings: Vec<(String, String)> = Vec::new();
    for e in exprs {
        let ExprNode::Assign { target, value } = &*e.node else {
            return None;
        };
        let LValue::Ivar { name } = target else {
            return None;
        };
        let ExprNode::Var { name: var_name, .. } = &*value.node else {
            return None;
        };
        bindings.push((go_field_name(name.as_str()), var_name.as_str().to_string()));
    }
    if bindings.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(slot) = parent_slot {
        parts.push(slot.to_string());
    }
    parts.extend(bindings.iter().map(|(f, v)| format!("{f}: {v}")));
    Some(format!("&{class_name}{{{}}}", parts.join(", ")))
}

/// `ActionController::Base` → `ActionControllerBase`. Go identifiers
/// can't contain `::` (Ruby's namespace separator); strip it so the
/// emitted type at least file-parses.
fn sanitize_type_name(name: &str) -> String {
    name.replace("::", "")
}

/// Resolve the parent class as the type name to embed (`*Parent`) on
/// the subclass struct. Returns `None` when the class has no parent
/// or the parent is a Ruby/system root we don't emit a Go struct for
/// (e.g. `Object`, `BasicObject`). The chain composes: ApplicationRecord
/// → `*ActiveRecordBase`, Article → `*ApplicationRecord`. Method
/// promotion delivers Base's instance methods through both hops.
fn embedded_parent_type(class: &LibraryClass) -> Option<String> {
    let parent = class.parent.as_ref()?;
    let raw = parent.0.as_str();
    if matches!(raw, "Object" | "BasicObject") {
        return None;
    }
    Some(sanitize_type_name(raw))
}

/// Synthesize a no-body constructor for classes that embed a parent
/// but have no Ruby `initialize` of their own (ApplicationRecord,
/// ApplicationController, …). Match the parent's ctor shape — variadic
/// for AR::Base-rooted chains (Article ← ApplicationRecord ←
/// ActiveRecordBase, all take optional `_attrs = {}`), non-variadic
/// for AC::Base-rooted chains (ArticlesController ← ApplicationController
/// ← ActionControllerBase, plain `def initialize`). `parent_is_variadic`
/// comes from a pre-pass over every LC's `initialize` MethodDef in
/// `go2.rs::emit_overlay_files`.
fn emit_default_embedded_constructor(
    class_name: &str,
    parent_ty: &str,
    parent_is_variadic: bool,
) -> String {
    if parent_is_variadic {
        format!(
            "func New{class_name}(_opts ...map[string]interface{{}}) *{class_name} {{\n\treturn &{class_name}{{{parent_ty}: New{parent_ty}(_opts...)}}\n}}\n"
        )
    } else {
        format!(
            "func New{class_name}() *{class_name} {{\n\treturn &{class_name}{{{parent_ty}: New{parent_ty}()}}\n}}\n"
        )
    }
}

/// Ruby method names allow `?`, `!`, `=` suffixes; Go identifiers
/// don't. Map to Go-friendly suffixes so emitted shapes file-parse.
/// Used for BARE-FN names (e.g. `Inflector_pluralize`) — does NOT
/// pascalize. Method-call sites that need PascalCase form use
/// `go2_method_ident` in expr.rs instead.
pub(super) fn sanitize_method_name(name: &str) -> String {
    // Operator-shape method names (`[]`, `[]=`, `<=>`, `==`, `+`,
    // `-`, ...) need to map to Go identifiers. Handle the common
    // ones explicitly; fall back to a `op_<hex>` form for anything
    // else so we never emit an unparseable identifier.
    match name {
        "[]" => return "op_get".to_string(),
        "[]=" => return "op_set".to_string(),
        "<=>" => return "op_cmp".to_string(),
        "==" => return "op_eq".to_string(),
        "!=" => return "op_ne".to_string(),
        "<" => return "op_lt".to_string(),
        "<=" => return "op_le".to_string(),
        ">" => return "op_gt".to_string(),
        ">=" => return "op_ge".to_string(),
        "+" => return "op_add".to_string(),
        "-" => return "op_sub".to_string(),
        "*" => return "op_mul".to_string(),
        "/" => return "op_div".to_string(),
        "%" => return "op_mod".to_string(),
        "<<" => return "op_lshift".to_string(),
        ">>" => return "op_rshift".to_string(),
        "&" => return "op_and".to_string(),
        "|" => return "op_or".to_string(),
        "^" => return "op_xor".to_string(),
        "~" => return "op_inv".to_string(),
        _ => {}
    }
    let mapped = name
        .replace("=", "_eq")
        .replace("?", "_p")
        .replace("!", "_bang");
    if mapped.is_empty() {
        "method".to_string()
    } else {
        mapped
    }
}

/// PascalCase variant for instance-method DEFINITIONS, mirroring
/// `go2_method_ident` in expr.rs so call sites and method defs line
/// up: `destroyed?` → `Destroyed`, `save!` → `SaveBang`, plain
/// `destroy` → `Destroy`. Operator-shape names route through
/// `sanitize_method_name` (`[]` → `op_get`, lowercase) — the index/
/// operator peepholes in expr.rs invoke these via Go index syntax,
/// not method-call, so the casing doesn't have to match.
fn pascalize_instance_method_name(name: &str) -> String {
    // Operator-shape names route through sanitize_method_name (`[]`
    // → `op_get`, `[]=` → `op_set`, ...) then pascalize via
    // `go_method_name` (`op_get` → `OpGet`). The call-site emit for
    // `recv[k]` / `recv[k]=v` against a Class-typed receiver emits
    // `recv.OpGet(...)` / `recv.OpSet(...)` (PascalCase, matches
    // here).
    if !name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '?' || c == '!' || c == '=') {
        return crate::emit::go::shared::go_method_name(&sanitize_method_name(name));
    }
    // Mirror go2_method_ident: strip `?`, `!` → `_bang`, then
    // pascalize via go_method_name (`_`-split, per-segment Pascal,
    // `id` → `ID`).
    let stripped = name.strip_suffix('?').unwrap_or(name);
    let normalized = stripped.replace('!', "_bang");
    crate::emit::go::shared::go_method_name(&normalized)
}

pub fn emit_module(methods: &[MethodDef]) -> Result<String, String> {
    // Mode::Module — no enclosing class; module-level methods emit
    // as bare functions. `SelfRef` inside them has no class context,
    // so the walker surfaces a TODO marker if it appears.
    let mut out = String::new();
    for m in methods {
        // Fresh ctx per method so `declared` doesn't leak between
        // methods. Seed with param names so param re-assignment
        // emits as `=`. void_method mirrors what render_return uses
        // for the func decl's return type.
        let returns_void = matches!(
            m.signature.as_ref(),
            Some(Ty::Fn { ret, .. }) if matches!(ret.as_ref(), Ty::Nil)
        );
        let mut ctx = EmitCtx::none();
        ctx.void_method = returns_void;
        for p in &m.params {
            ctx.declare_param(p.name.as_str());
        }
        let params = render_params(m);
        let ret = render_return(m);
        let name = sanitize_method_name(m.name.as_str());
        let body = render_body(&ctx, m);
        out.push_str(&format!("func {name}({params}){ret} {{\n{body}}}\n\n"));
    }
    Ok(out)
}

pub fn format_constant(name: &str, value: &Expr) -> String {
    // Module-level constants in Go are `var NAME = expr` (not
    // `const`) because the values are typically composite literals
    // (Hash → map literal, Regex → regexp.MustCompile) — neither
    // is a Go compile-time constant.
    //
    // The walker's body emit already handles every shape we need
    // (Hash → map literal, StringInterp → fmt.Sprintf, Regex →
    // regexp.MustCompile via emit_literal). `freeze` peeled by
    // emit_send.
    let ctx = super::expr::EmitCtx::none();
    let rendered = super::expr::emit_expr(&ctx, value);
    format!("var {name} = {rendered}")
}

/// Format a module-level `@ivar = value` as a Go package-level
/// `var <Owner>_<ivar>_slot = <rendered>`. The qualified `owner`
/// (e.g. `"ActionView::ViewHelpers"`) is collapsed by stripping
/// `::` and that string is woven into the var name so reads (which
/// also synthesize `<Owner>_<ivar>_slot` — see
/// `ExprNode::Ivar` in expr.rs) resolve to the same identifier.
/// Empty owner (program-root ivar) skips the prefix and falls back
/// to a bare `<ivar>_slot` so the var still exists at package scope.
pub fn format_module_ivar(owner: &str, name: &str, value: &Expr) -> String {
    let ctx = super::expr::EmitCtx::none();
    let rendered = super::expr::emit_expr(&ctx, value);
    let var_name = if owner.is_empty() {
        format!("{name}_slot")
    } else {
        format!("{}_{name}_slot", sanitize_type_name(owner))
    };
    format!("var {var_name} = {rendered}")
}

fn emit_method(
    class_name: &str,
    m: &MethodDef,
    self_methods: &std::rc::Rc<std::collections::HashSet<String>>,
) -> String {
    let params = render_params(m);
    let ret = render_return(m);
    let receiver = match m.receiver {
        MethodReceiver::Instance => format!("(self *{class_name}) "),
        MethodReceiver::Class => String::new(),
    };
    let class_method_name = match m.receiver {
        // Instance methods emit PascalCase to match the
        // `go2_method_ident` call-site shape (expr.rs). `destroyed?`
        // → `Destroyed`, `save!` → `SaveBang`, `[]` → `op_get`
        // (operators stay lowercase — they're invoked via the
        // index-syntax peepholes in expr.rs, not by Go method-call).
        MethodReceiver::Instance => pascalize_instance_method_name(m.name.as_str()),
        // Class methods emit as bare functions prefixed with the
        // class name (Go has no class-method dispatch). Concrete
        // call sites reference `Foo_bar(...)` via the `self.class.X`
        // and `SelfRef-in-class-method` peepholes — both use the
        // bare-fn (lowercase) name from `sanitize_method_name`.
        MethodReceiver::Class => format!("{class_name}_{}", sanitize_method_name(m.name.as_str())),
    };
    // Build per-method context so the body walker can resolve
    // `SelfRef` against the right enclosing class + method receiver.
    // Seed `declared` with the method's parameter names so any
    // assignment to a param emits as `=`, not `:=`. `void_method`
    // mirrors the Ty::Nil-return detection in render_return so the
    // body walker can suppress the implicit `return X` wrap.
    let returns_void = matches!(
        m.signature.as_ref(),
        Some(Ty::Fn { ret, .. }) if matches!(ret.as_ref(), Ty::Nil)
    );
    let return_ty = match m.signature.as_ref() {
        Some(Ty::Fn { ret, .. }) => Some((**ret).clone()),
        _ => None,
    };
    let ctx = EmitCtx {
        class_name: Some(class_name.to_string()),
        in_class_method: matches!(m.receiver, MethodReceiver::Class),
        var_renames: std::collections::HashMap::new(),
        declared: std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashSet::new())),
        void_method: returns_void,
        in_module_singleton: false,
        self_methods: Some(std::rc::Rc::clone(self_methods)),
        return_ty,
    };
    for p in &m.params {
        ctx.declare_param(p.name.as_str());
    }
    let body = render_body(&ctx, m);
    format!("func {receiver}{class_method_name}({params}){ret} {{\n{body}}}\n")
}

fn render_params(m: &MethodDef) -> String {
    // Take per-param Tys from `signature: Option<Ty::Fn { params }>`
    // when present; fall back to `interface{}` if absent (no RBS or
    // not decomposable).
    let sig_tys = signature_param_tys(m);
    m.params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let ty = sig_tys.as_ref().and_then(|tys| tys.get(i));
            format!("{} {}", sanitize(p.name.as_str()), go_ty_stub(ty))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn signature_param_tys(m: &MethodDef) -> Option<Vec<Ty>> {
    let Some(Ty::Fn { params, .. }) = m.signature.as_ref() else {
        return None;
    };
    Some(
        params
            .iter()
            .filter(|p| !matches!(p.kind, ParamKind::Block | ParamKind::KeywordRest))
            .map(|p| p.ty.clone())
            .collect(),
    )
}

/// Emit the walked body. Unhandled `ExprNode` variants surface as
/// `/* TODO: emit ... */` comments inside the body — that's
/// intentional, since it lets the v2/ overlay's `go build` surface
/// exactly what walker coverage is missing (rather than hiding the
/// gap behind a `panic("stub")`). Per-method panic fallbacks come
/// back if we ever need them, but for the strangler-fig widening
/// the loud failure is the inventory.
///
/// Special case: when the body is effectively nil (a bare `Lit::Nil`
/// OR an empty `Seq` OR a `Seq` whose only expr is `Lit::Nil` — Ruby
/// `def foo; end` ingests as the empty-Seq form, used by AR::Base's
/// `_adapter_insert` etc. as overridable stubs) AND the method's
/// return type is non-Nil, emit `return <zero value>`. Without this,
/// Go rejects `return nil` against e.g. `int64` returns or "missing
/// return" for the bare empty-body shape.
fn render_body(ctx: &EmitCtx, m: &MethodDef) -> String {
    use crate::expr::{ExprNode, Literal};
    let body_is_effectively_nil = match &*m.body.node {
        ExprNode::Lit { value: Literal::Nil } => true,
        ExprNode::Seq { exprs } => {
            exprs.is_empty()
                || (exprs.len() == 1
                    && matches!(&*exprs[0].node, ExprNode::Lit { value: Literal::Nil }))
        }
        _ => false,
    };
    if !ctx.void_method && body_is_effectively_nil {
        if let Some(Ty::Fn { ret, .. }) = m.signature.as_ref() {
            return format!("\treturn {}\n", go_zero_value(ret));
        }
    }
    // Return-Ty back-prop: when the method's declared return is
    // `Hash[K, V]` or `Array[E]` and the body's tail is a bare empty
    // literal (analyzer didn't pin e.ty — typically Var/Var), rewrite
    // the body with the literal's e.ty set to the declared return.
    // Without this, AR::Base's `def attributes; {}; end` (declared
    // `Hash[Symbol, untyped]`) would emit `map[string]string{}` via
    // the all-empty heuristic and clash with the declared return.
    let body = if let Some(Ty::Fn { ret, .. }) = m.signature.as_ref() {
        backprop_return_ty_to_tail(&m.body, ret)
    } else {
        m.body.clone()
    };
    emit_return_body(ctx, &body)
}

/// Pin the tail expression's `e.ty` to `target_ty` when it's an empty
/// `{}` / `[]` literal whose analyzer-set Ty resolves to `interface{}`
/// (i.e., Var/Var or Untyped). Returns a new Expr; non-matching tails
/// pass through unmodified.
fn backprop_return_ty_to_tail(body: &Expr, target_ty: &Ty) -> Expr {
    use crate::expr::{ExprNode, LValue};
    if !matches!(target_ty, Ty::Hash { .. } | Ty::Array { .. }) {
        return body.clone();
    }
    match &*body.node {
        ExprNode::Hash { entries, kwargs }
            if literal_ty_uninformative(body)
                || (!entries.is_empty() && target_value_is_widening(target_ty)) =>
        {
            let mut tail = (*body).clone();
            tail.ty = Some(target_ty.clone());
            tail.node = Box::new(ExprNode::Hash {
                entries: entries.clone(),
                kwargs: *kwargs,
            });
            tail
        }
        ExprNode::Array { elements, style } if elements.is_empty() && literal_ty_uninformative(body) => {
            let mut tail = (*body).clone();
            tail.ty = Some(target_ty.clone());
            tail.node = Box::new(ExprNode::Array {
                elements: elements.clone(),
                style: style.clone(),
            });
            tail
        }
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            // Recurse into the literal tail first.
            let mut new_exprs: Vec<Expr> = exprs[..exprs.len() - 1].to_vec();
            let last = exprs.last().unwrap();
            new_exprs.push(backprop_return_ty_to_tail(last, target_ty));
            // Additional case: tail is `Var(name)`. Walk backwards
            // to find the first `Assign { Var(name), <empty literal> }`
            // and back-prop the target_ty there. Catches Ruby idiom:
            //     result = []
            //     result.push(x)
            //     ...
            //     result          # tail
            // With `result: Array[String]` return, the `result = []`
            // assignment needs to land as `[]string{}` (not the
            // default `[]interface{}{}`).
            if let ExprNode::Var { name, .. } = &*last.node {
                let var_name = name.as_str().to_string();
                for e in new_exprs.iter_mut() {
                    if let ExprNode::Assign {
                        target: LValue::Var { name: assign_name, .. },
                        value,
                    } = &*e.node
                    {
                        if assign_name.as_str() == var_name {
                            let new_value = backprop_return_ty_to_tail(value, target_ty);
                            let mut new_assign = e.clone();
                            new_assign.node = Box::new(ExprNode::Assign {
                                target: LValue::Var {
                                    id: match &e.node.as_ref() {
                                        ExprNode::Assign { target: LValue::Var { id, .. }, .. } => *id,
                                        _ => unreachable!(),
                                    },
                                    name: assign_name.clone(),
                                },
                                value: new_value,
                            });
                            *e = new_assign;
                            // Only the FIRST assignment carries the
                            // declaration; subsequent reassigns don't
                            // affect the var's Go type. Stop here.
                            break;
                        }
                    }
                }
            }
            let mut new_body = (*body).clone();
            new_body.node = Box::new(ExprNode::Seq { exprs: new_exprs });
            new_body
        }
        _ => body.clone(),
    }
}

/// True when the declared return type's value side widens to
/// `interface{}` — `Hash[K, Untyped]` (and the `Hash[K, Var]` shape
/// analyzer leaves unresolved). Force a non-empty Hash tail literal
/// to adopt this declared type so the emit produces the widened
/// `map[K]interface{}{...}` (boxing each typed value), rather than
/// the analyzer-inferred narrow `map[string]string` that vet rejects
/// against the declared return. Mirrors how Rust's call-site Cast
/// handles heterogeneous-Hash widening — Go just does it at the
/// literal directly since map element types are by-construction.
fn target_value_is_widening(target_ty: &Ty) -> bool {
    matches!(
        target_ty,
        Ty::Hash { value, .. } if matches!(value.as_ref(), Ty::Untyped | Ty::Var { .. })
    )
}

/// True when the analyzer's Ty on an empty literal carries no signal
/// for the emit (Var/Var or Untyped). Concrete-Ty literals already
/// flow through the back-prop branch in emit_expr.
fn literal_ty_uninformative(e: &Expr) -> bool {
    match e.ty.as_ref() {
        None => true,
        Some(Ty::Untyped) => true,
        Some(Ty::Var { .. }) => true,
        Some(Ty::Hash { key, value }) => {
            matches!(key.as_ref(), Ty::Var { .. } | Ty::Untyped)
                && matches!(value.as_ref(), Ty::Var { .. } | Ty::Untyped)
        }
        Some(Ty::Array { elem }) => {
            matches!(elem.as_ref(), Ty::Var { .. } | Ty::Untyped)
        }
        _ => false,
    }
}

/// Go zero-value literal for a Ty. Used as the default body when an
/// overridable method has an empty source body but a non-void return.
fn go_zero_value(ty: &Ty) -> String {
    match ty {
        Ty::Str | Ty::Sym => "\"\"".to_string(),
        Ty::Int => "0".to_string(),
        Ty::Float => "0.0".to_string(),
        Ty::Bool => "false".to_string(),
        Ty::Nil => "nil".to_string(),
        Ty::Hash { .. } | Ty::Array { .. } | Ty::Class { .. } => "nil".to_string(),
        Ty::Union { variants } => {
            let non_nil: Vec<&Ty> = variants
                .iter()
                .filter(|t| !matches!(t, Ty::Nil))
                .collect();
            // Union with Nil collapses to nil-zero of the typed
            // variant when reference-shaped; otherwise interface{}'s
            // zero is nil too.
            if non_nil.len() == 1 {
                match non_nil[0] {
                    Ty::Hash { .. } | Ty::Array { .. } | Ty::Class { .. } => {
                        "nil".to_string()
                    }
                    _ => "nil".to_string(),
                }
            } else {
                "nil".to_string()
            }
        }
        _ => "nil".to_string(),
    }
}

/// Avoid emitting Go reserved words as parameter names. Adds a `_`
/// suffix to any clash; preserves all others unchanged.
pub(super) fn sanitize(name: &str) -> String {
    const RESERVED: &[&str] = &[
        "break", "case", "chan", "const", "continue", "default",
        "defer", "else", "fallthrough", "for", "func", "go", "goto",
        "if", "import", "interface", "map", "package", "range",
        "return", "select", "struct", "switch", "type", "var",
    ];
    if RESERVED.contains(&name) {
        format!("{name}_")
    } else {
        name.to_string()
    }
}

fn render_return(m: &MethodDef) -> String {
    if let Some(Ty::Fn { ret, .. }) = m.signature.as_ref() {
        // Ty::Nil → Go void (no return type).
        if matches!(ret.as_ref(), Ty::Nil) {
            return String::new();
        }
        return format!(" {}", go_ty_stub(Some(ret)));
    }
    " interface{}".to_string()
}

/// Emit a Ruby module-singleton class — `module X; class << self;
/// attr_accessor :slot; end; end` — as a unit struct + per-slot
/// package var + bare accessor/method functions.
///
/// Output shape:
///
/// ```text
/// type ActiveRecord struct{}
///
/// var ActiveRecord_adapter_slot *AdapterInterface
///
/// func ActiveRecord_adapter() *AdapterInterface { return ActiveRecord_adapter_slot }
/// func ActiveRecord_adapter_eq(value *AdapterInterface) { ActiveRecord_adapter_slot = value }
/// ```
///
/// Callers reach the slot via `ActiveRecord.adapter` /
/// `ActiveRecord.adapter = v` on the Ruby side; the expr walker's
/// `Const(X).method(args)` → `X_method(args)` rewrite + `adapter=`'s
/// sanitize-to-`adapter_eq` mapping route those calls to the bare-fn
/// pair this emits.
fn emit_module_singleton(class: &LibraryClass) -> Result<String, String> {
    let name = sanitize_type_name(class.name.0.as_str());
    let mut out = String::new();

    // Unit struct keeps `<Name>` a valid type so e.g. `var x *<Name>`
    // parses if anyone references the module-as-type. Methods live
    // as bare `<Name>_<method>` funcs alongside.
    out.push_str(&format!("type {name} struct{{}}\n\n"));

    // Per-slot package vars. Each unique ivar across the
    // attr_reader/attr_writer pairs becomes one `var`. The `_slot`
    // suffix avoids collision with the accessor func of the same
    // base name (`ActiveRecord_adapter` is the reader fn;
    // `ActiveRecord_adapter_slot` is the backing storage).
    let slots = collect_module_singleton_slots(&class.methods);
    for (slot, ty) in &slots {
        out.push_str(&format!(
            "var {name}_{slot}_slot {}\n",
            go_ty_stub(Some(ty)),
        ));
    }
    if !slots.is_empty() {
        out.push('\n');
    }

    // Methods — every entry is Class-receiver by detection
    // (`all-class-receivers` predicate). Same shape as
    // `emit_method`'s Class-receiver branch, but EmitCtx flips
    // `in_module_singleton=true` so `@ivar` reads/writes resolve
    // to the namespaced slot rather than `self.Field`.
    for m in &class.methods {
        out.push_str(&emit_module_singleton_method(&name, m));
        out.push('\n');
    }
    Ok(out)
}

/// Pair `attr_reader`/`attr_writer` MethodDefs by ivar name and
/// return one (name, Ty) per unique slot. Signature carries the
/// type — reader's return Ty, writer's first-param Ty. Falls back
/// to `Ty::Untyped` (which `go_ty_stub` maps to `interface{}`)
/// when no signature is set.
fn collect_module_singleton_slots(methods: &[MethodDef]) -> Vec<(String, Ty)> {
    let mut out: Vec<(String, Ty)> = Vec::new();
    for m in methods {
        let raw_name = m.name.as_str().trim_end_matches('=').to_string();
        let ty = match m.kind {
            AccessorKind::AttributeReader => match m.signature.as_ref() {
                Some(Ty::Fn { ret, .. }) => (**ret).clone(),
                _ => Ty::Untyped,
            },
            AccessorKind::AttributeWriter => match m.signature.as_ref() {
                Some(Ty::Fn { params, .. }) => params
                    .first()
                    .map(|p| p.ty.clone())
                    .unwrap_or(Ty::Untyped),
                _ => Ty::Untyped,
            },
            _ => continue,
        };
        if out.iter().any(|(n, _)| *n == raw_name) {
            continue;
        }
        out.push((raw_name, ty));
    }
    out
}

fn emit_module_singleton_method(class_name: &str, m: &MethodDef) -> String {
    let params = render_params(m);
    let ret = render_return(m);
    let method = sanitize_method_name(m.name.as_str());
    let returns_void = matches!(
        m.signature.as_ref(),
        Some(Ty::Fn { ret, .. }) if matches!(ret.as_ref(), Ty::Nil)
    );
    let return_ty = match m.signature.as_ref() {
        Some(Ty::Fn { ret, .. }) => Some((**ret).clone()),
        _ => None,
    };
    let ctx = EmitCtx {
        class_name: Some(class_name.to_string()),
        in_class_method: true,
        var_renames: std::collections::HashMap::new(),
        declared: std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashSet::new())),
        void_method: returns_void,
        in_module_singleton: true,
        // Module-singleton has no instance methods; nothing to put
        // in the self-method registry.
        self_methods: None,
        return_ty,
    };
    for p in &m.params {
        ctx.declare_param(p.name.as_str());
    }
    let body = render_body(&ctx, m);
    format!("func {class_name}_{method}({params}){ret} {{\n{body}}}\n")
}

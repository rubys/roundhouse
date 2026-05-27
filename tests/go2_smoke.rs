//! go2 overlay regression tests.
//!
//! Locks in the contract for what's landed in `src/emit/go2/`:
//!
//! - **Shape test** (unconditional): emit real-blog, assert the v2/
//!   inflector.go file is present and contains the expected function
//!   declaration. Catches accidental walker regressions, output-path
//!   reshuffles, or signature-decomposition breakage.
//!
//! - **Toolchain test** (`#[ignore]`): emit + `go vet ./app/v2` +
//!   `go test` against a smoke test that exercises
//!   `Inflector_pluralize`. Requires the Go toolchain on PATH;
//!   matches `tests/go_toolchain.rs`'s posture for legacy go.
//!
//! Run the toolchain test with:
//!
//!     cargo test --test go2_smoke -- --ignored --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::dialect::{
    AccessorKind, LibraryClass, MethodDef, MethodReceiver, Param as DialectParam,
};
use roundhouse::effect::EffectSet;
use roundhouse::emit::{go, go2};
use roundhouse::expr::{Expr, ExprNode, LValue, Literal};
use roundhouse::ident::{ClassId, Symbol, VarId};
use roundhouse::ingest::ingest_app;
use roundhouse::span::Span;
use roundhouse::ty::{Param as TyParam, ParamKind, Ty};

const FIXTURE: &str = "fixtures/real-blog";

fn ingest_with_analyzer() -> roundhouse::App {
    let mut app = ingest_app(Path::new(FIXTURE)).expect("ingest real-blog");
    Analyzer::new(&app).analyze(&mut app);
    app
}

fn find_file<'a>(
    files: &'a [roundhouse::emit::EmittedFile],
    needle: &str,
) -> Option<&'a roundhouse::emit::EmittedFile> {
    files.iter().find(|f| f.path.to_string_lossy() == needle)
}

/// Synthesize the module-singleton LibraryClass shape — `module
/// ActiveRecord; class << self; attr_accessor :adapter; end; end`
/// — and assert the emitted Go matches the module-slot architecture
/// contract: unit struct + per-slot package var + reader/writer
/// accessor functions, with `@adapter` reads/writes routing to the
/// namespaced slot (not `self.Adapter`).
///
/// Built from a synthesized `LibraryClass` rather than driven through
/// `GO_RUNTIME` because `active_record/base.rb` has many remaining
/// emit gaps (each-blocks, `.class` reflection, `Time` chain, etc.);
/// dropping it in whole would break `go vet` on the v2/ overlay. The
/// synthetic approach lets the module-singleton contract land
/// independently of the broader AR::Base widening.
#[test]
fn module_singleton_shape() {
    // `def self.adapter; @adapter; end` — synthesized from
    // `attr_accessor :adapter` inside `class << self`. Body is a
    // bare Ivar read; signature carries the slot's Ty so the
    // emitted `var ActiveRecord_adapter_slot <Ty>` declares the
    // right type. AdapterInterface stands in for what the RBS
    // gives in real ingest.
    let adapter_ty = Ty::Class {
        id: ClassId(Symbol::from("AdapterInterface")),
        args: vec![],
    };
    let reader = MethodDef {
        name: Symbol::from("adapter"),
        receiver: MethodReceiver::Class,
        params: vec![],
        body: Expr::new(
            Span::synthetic(),
            ExprNode::Ivar { name: Symbol::from("adapter") },
        ),
        signature: Some(Ty::Fn {
            params: vec![],
            block: None,
            ret: Box::new(adapter_ty.clone()),
            effects: EffectSet::default(),
        }),
        effects: EffectSet::default(),
        enclosing_class: Some(Symbol::from("ActiveRecord")),
        kind: AccessorKind::AttributeReader,
        is_async: false,
        mutates_self: false,
        block_param: None,
    };
    // `def self.adapter=(value); @adapter = value; end`.
    let writer = MethodDef {
        name: Symbol::from("adapter="),
        receiver: MethodReceiver::Class,
        params: vec![DialectParam::positional(Symbol::from("value"))],
        body: Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Ivar { name: Symbol::from("adapter") },
                value: Expr::new(
                    Span::synthetic(),
                    ExprNode::Var {
                        id: VarId(0),
                        name: Symbol::from("value"),
                    },
                ),
            },
        ),
        signature: Some(Ty::Fn {
            params: vec![TyParam {
                name: Symbol::from("value"),
                ty: adapter_ty.clone(),
                kind: ParamKind::Required,
            }],
            block: None,
            ret: Box::new(adapter_ty.clone()),
            effects: EffectSet::default(),
        }),
        effects: EffectSet::default(),
        enclosing_class: Some(Symbol::from("ActiveRecord")),
        kind: AccessorKind::AttributeWriter,
        is_async: false,
        mutates_self: false,
        block_param: None,
    };
    let class = LibraryClass {
        name: ClassId(Symbol::from("ActiveRecord")),
        is_module: true,
        parent: None,
        includes: vec![],
        methods: vec![reader, writer],
        origin: None,
    };

    let emitted = go2::emit_library_class(&class).expect("emit module singleton");

    // Unit struct — the type name is preserved so `var x
    // *ActiveRecord` parses if anyone references it.
    assert!(
        emitted.contains("type ActiveRecord struct{}"),
        "missing unit-struct decl:\n{emitted}",
    );
    // Per-slot package var, namespaced by class name to avoid
    // cross-module collision.
    assert!(
        emitted.contains("var ActiveRecord_adapter_slot *AdapterInterface"),
        "missing slot var:\n{emitted}",
    );
    // Reader function — return slot, no receiver param.
    assert!(
        emitted.contains("func ActiveRecord_adapter() *AdapterInterface {"),
        "missing reader fn signature:\n{emitted}",
    );
    assert!(
        emitted.contains("return ActiveRecord_adapter_slot"),
        "reader body missing slot read:\n{emitted}",
    );
    // Writer function — sanitize maps `adapter=` to `adapter_eq`.
    // Writes target the slot; the value param is the single
    // positional arg.
    assert!(
        emitted.contains(
            "func ActiveRecord_adapter_eq(value *AdapterInterface)"
        ),
        "missing writer fn signature:\n{emitted}",
    );
    assert!(
        emitted.contains("ActiveRecord_adapter_slot = value"),
        "writer body missing slot assign:\n{emitted}",
    );
    // Setter body must not emit `return slot = value` (assign-as-
    // expression is illegal in Go). Emit the assign as a statement
    // and then `return` by reading the slot back.
    assert!(
        !emitted.contains("return ActiveRecord_adapter_slot = value"),
        "writer body emits assign-in-return (invalid Go):\n{emitted}",
    );
    assert!(
        emitted.contains("ActiveRecord_adapter_slot = value\n\treturn ActiveRecord_adapter_slot"),
        "writer body missing tail read-back return:\n{emitted}",
    );
}

/// Hand-written runtime — `app/v2/adapter_interface.go` must ship in
/// the overlay so the transpiled `ActiveRecord` module-singleton's
/// slot type (`*AdapterInterface`) resolves at `go vet` / `go build`
/// time. Catches accidental renames or removals from the `RT_V2_*`
/// table.
#[test]
fn hand_written_runtime_present() {
    let app = ingest_with_analyzer();
    let files = go2::emit_overlay_files(&app);

    let adapter = find_file(&files, "app/v2/adapter_interface.go")
        .expect("v2/adapter_interface.go missing");
    assert!(
        adapter.content.contains("type ActiveRecordAdapterInterface interface {"),
        "adapter interface decl missing:\n{}",
        adapter.content,
    );
    assert!(
        adapter.content.contains("Find(tableName string, id int64) Row"),
        "Find sig missing or shape-shifted:\n{}",
        adapter.content,
    );
    assert!(
        adapter.content.contains("type Row = map[string]any"),
        "Row alias missing:\n{}",
        adapter.content,
    );
}

/// Pair to `module_singleton_shape` — assert that a plain class
/// (`is_module=false`) with the same attr_accessor still emits as a
/// struct field, NOT a module-singleton slot. Regression guard: the
/// module-singleton detection predicate must not fire on regular
/// classes; otherwise per-instance state would silently lift to
/// package vars.
#[test]
fn module_singleton_does_not_fire_on_plain_class() {
    let attr_ty = Ty::Class {
        id: ClassId(Symbol::from("AdapterInterface")),
        args: vec![],
    };
    let reader = MethodDef {
        name: Symbol::from("adapter"),
        receiver: MethodReceiver::Instance,
        params: vec![],
        body: Expr::new(
            Span::synthetic(),
            ExprNode::Ivar { name: Symbol::from("adapter") },
        ),
        signature: Some(Ty::Fn {
            params: vec![],
            block: None,
            ret: Box::new(attr_ty.clone()),
            effects: EffectSet::default(),
        }),
        effects: EffectSet::default(),
        enclosing_class: Some(Symbol::from("Configurable")),
        kind: AccessorKind::AttributeReader,
        is_async: false,
        mutates_self: false,
        block_param: None,
    };
    let class = LibraryClass {
        name: ClassId(Symbol::from("Configurable")),
        is_module: false,
        parent: None,
        includes: vec![],
        methods: vec![reader],
        origin: None,
    };

    let emitted = go2::emit_library_class(&class).expect("emit plain class");

    // Plain class → struct with a field; NOT a module-singleton
    // slot. Adapter shows up as a struct field rendered from the
    // attr_reader's signature.
    assert!(
        emitted.contains("type Configurable struct {")
            && emitted.contains("Adapter *AdapterInterface"),
        "plain class should emit struct field, not slot:\n{emitted}",
    );
    assert!(
        !emitted.contains("_slot"),
        "plain class accidentally hit module-singleton path:\n{emitted}",
    );
}

/// `raise X, "msg"` → `panic("msg")` peephole. The class arg is
/// dropped (Go has no class-typed panic; the message usually
/// carries enough context for callers). 1-arg `raise "msg"` or
/// `raise X` panics with the lone arg.
///
/// Critical: tail-position `raise` must NOT be wrapped in `return`
/// (Go rejects `return panic(...)` — panic returns nothing). The
/// emit_return_at Send-raise arm handles that.
#[test]
fn raise_panic_peephole() {
    // `def fail!(msg); raise NotImplementedError, msg; end` — the
    // 2-arg form. Body: Send {recv:None, method:"raise",
    // args:[Const(NotImplementedError), Var(msg)]}.
    let raise_2arg = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: None,
            method: Symbol::from("raise"),
            args: vec![
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Const { path: vec![Symbol::from("NotImplementedError")] },
                ),
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Var { id: VarId(0), name: Symbol::from("msg") },
                ),
            ],
            block: None,
            parenthesized: true,
        },
    );
    let fail_method = MethodDef {
        name: Symbol::from("fail!"),
        receiver: MethodReceiver::Instance,
        params: vec![DialectParam::positional(Symbol::from("msg"))],
        body: raise_2arg,
        signature: Some(Ty::Fn {
            params: vec![TyParam {
                name: Symbol::from("msg"),
                ty: Ty::Str,
                kind: ParamKind::Required,
            }],
            block: None,
            ret: Box::new(Ty::Nil),
            effects: EffectSet::default(),
        }),
        effects: EffectSet::default(),
        enclosing_class: Some(Symbol::from("Crasher")),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    };
    // `def abort_with(msg); raise msg; end` — 1-arg form. Body
    // is a Send with one arg (the message itself).
    let raise_1arg = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: None,
            method: Symbol::from("raise"),
            args: vec![Expr::new(
                Span::synthetic(),
                ExprNode::Var { id: VarId(0), name: Symbol::from("msg") },
            )],
            block: None,
            parenthesized: true,
        },
    );
    let abort_method = MethodDef {
        name: Symbol::from("abort_with"),
        receiver: MethodReceiver::Instance,
        params: vec![DialectParam::positional(Symbol::from("msg"))],
        body: raise_1arg,
        signature: Some(Ty::Fn {
            params: vec![TyParam {
                name: Symbol::from("msg"),
                ty: Ty::Str,
                kind: ParamKind::Required,
            }],
            block: None,
            ret: Box::new(Ty::Nil),
            effects: EffectSet::default(),
        }),
        effects: EffectSet::default(),
        enclosing_class: Some(Symbol::from("Crasher")),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    };
    let class = LibraryClass {
        name: ClassId(Symbol::from("Crasher")),
        is_module: false,
        parent: None,
        includes: vec![],
        methods: vec![fail_method, abort_method],
        origin: None,
    };

    let emitted = go2::emit_library_class(&class).expect("emit crasher class");

    // 2-arg form with a known framework error class wraps the message
    // in a typed sentinel struct so the HTTP router_glue's defer-
    // recover can type-switch (RecordNotFound → 404 etc.). Unknown
    // classes still produce a bare `panic(msg)`.
    assert!(
        emitted.contains("panic(&NotImplementedErrorValue{Message: msg})"),
        "raise NotImplementedError, msg should wrap with typed sentinel:\n{emitted}",
    );
    // 1-arg form still panics with the lone arg (no class to inspect).
    assert!(
        emitted.contains("panic(msg)"),
        "1-arg raise should produce panic(msg):\n{emitted}",
    );
    // Tail-position raise must NOT be wrapped in `return` — Go
    // rejects `return panic(...)` (panic returns nothing).
    assert!(
        !emitted.contains("return panic("),
        "tail-position raise must not be return-wrapped:\n{emitted}",
    );
    // And the legacy broken shape (`Raise(...)`) must be gone.
    assert!(
        !emitted.contains("Raise("),
        "raise must not pascalize to undefined `Raise(...)`:\n{emitted}",
    );
}

/// `Time.now.utc.iso8601` chain — used by `ActiveRecord::Base#fill_timestamps`
/// to stamp `created_at` / `updated_at` with a UTC ISO-8601 string. Ruby
/// chains three Sends; Go's stdlib provides no element-wise analog
/// (`time.Time` has no `.utc` method, formatting goes through
/// `Format(layout)`), so the outermost-Send peephole rewrites the whole
/// chain to `time.Now().UTC().Format(time.RFC3339)` in one shot. The
/// `time.RFC3339` literal triggers `time` import injection via
/// `needed_imports`.
#[test]
fn time_now_utc_iso8601_peephole() {
    // Body: `Time.now.utc.iso8601` — outermost Send is `.iso8601`
    // on `.utc` on `.now` on `Const(Time)`.
    let chain = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(Expr::new(
                        Span::synthetic(),
                        ExprNode::Send {
                            recv: Some(Expr::new(
                                Span::synthetic(),
                                ExprNode::Const { path: vec![Symbol::from("Time")] },
                            )),
                            method: Symbol::from("now"),
                            args: vec![],
                            block: None,
                            parenthesized: false,
                        },
                    )),
                    method: Symbol::from("utc"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                },
            )),
            method: Symbol::from("iso8601"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let stamp = MethodDef {
        name: Symbol::from("stamp"),
        receiver: MethodReceiver::Instance,
        params: vec![],
        body: chain,
        signature: Some(Ty::Fn {
            params: vec![],
            block: None,
            ret: Box::new(Ty::Str),
            effects: EffectSet::default(),
        }),
        effects: EffectSet::default(),
        enclosing_class: Some(Symbol::from("Clock")),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    };
    let class = LibraryClass {
        name: ClassId(Symbol::from("Clock")),
        is_module: false,
        parent: None,
        includes: vec![],
        methods: vec![stamp],
        origin: None,
    };

    let emitted = go2::emit_library_class(&class).expect("emit clock class");

    // Full Go chain lands in one shot; the substring check pins the
    // exact rewrite so a partial-chain change in the future is loud.
    assert!(
        emitted.contains("time.Now().UTC().Format(time.RFC3339)"),
        "Time.now.utc.iso8601 chain missing Go rewrite:\n{emitted}",
    );
    // Regression guard: the generic Const-recv class-method fallback
    // would otherwise emit `Time_now()` (an undefined bare function).
    assert!(
        !emitted.contains("Time_now("),
        "iso8601 chain leaked through to Const-class-method dispatch:\n{emitted}",
    );
}

/// `arr.include?(x)` with an Array-typed receiver must route to
/// `slices.Contains(arr, x)`, NOT the default `strings.Contains` path
/// that the receiver-Ty-agnostic str_method fallback would take. Used
/// by `ActiveRecord::Base#fill_timestamps` (`cols.include?(:updated_at)`
/// where `cols` is `Array[Symbol]` from `self.class.schema_columns`).
/// Sym literal arg lowers to a Go string literal — `slices.Contains`
/// type-checks with `[]string` recv + `string` arg.
#[test]
fn include_array_recv_routes_to_slices_contains() {
    // Body: `cols.include?(:updated_at)` with `cols: Array[Symbol]`.
    // The Var carries the Ty explicitly so the receiver-Ty branch
    // fires without needing the analyzer to propagate from a real
    // class shape.
    let mut cols_var = Expr::new(
        Span::synthetic(),
        ExprNode::Var { id: VarId(0), name: Symbol::from("cols") },
    );
    cols_var.ty = Some(Ty::Array { elem: Box::new(Ty::Sym) });
    let body = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(cols_var),
            method: Symbol::from("include?"),
            args: vec![Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: roundhouse::Literal::Sym { value: Symbol::from("updated_at") } },
            )],
            block: None,
            parenthesized: true,
        },
    );
    let probe = MethodDef {
        name: Symbol::from("has_col?"),
        receiver: MethodReceiver::Instance,
        params: vec![DialectParam::positional(Symbol::from("cols"))],
        body,
        signature: Some(Ty::Fn {
            params: vec![TyParam {
                name: Symbol::from("cols"),
                ty: Ty::Array { elem: Box::new(Ty::Sym) },
                kind: ParamKind::Required,
            }],
            block: None,
            ret: Box::new(Ty::Bool),
            effects: EffectSet::default(),
        }),
        effects: EffectSet::default(),
        enclosing_class: Some(Symbol::from("ColCheck")),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    };
    let class = LibraryClass {
        name: ClassId(Symbol::from("ColCheck")),
        is_module: false,
        parent: None,
        includes: vec![],
        methods: vec![probe],
        origin: None,
    };

    let emitted = go2::emit_library_class(&class).expect("emit colcheck class");

    // Array receiver → slices.Contains.
    assert!(
        emitted.contains("slices.Contains(cols, \"updated_at\")"),
        "Array recv include? missing slices.Contains rewrite:\n{emitted}",
    );
    // Regression guard: the str_method fallback would route to
    // strings.Contains, which fails to compile against `[]string`.
    assert!(
        !emitted.contains("strings.Contains(cols"),
        "Array recv include? leaked through to strings.Contains:\n{emitted}",
    );
}

/// `recv[-1]` / `recv[-2]` — Ruby's negative-index Array/String access
/// has no Go analog; rewrite to `recv[len(recv)-N]`. Used by
/// `ActiveRecord::Base.last` (`records.empty? ? nil : records[-1]`).
/// Gated on a literal negative `Int` arg so dynamically-negative
/// runtime values still emit the bare form and panic at index time
/// — matching Go's convention.
#[test]
fn negative_index_rewrites_to_len_minus_n() {
    // Body: `records[-1]` — Send {recv: Var(records), method: "[]",
    // args: [Lit::Int(-1)]}.
    let neg_one = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Var { id: VarId(0), name: Symbol::from("records") },
            )),
            method: Symbol::from("[]"),
            args: vec![Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: roundhouse::Literal::Int { value: -1 } },
            )],
            block: None,
            parenthesized: false,
        },
    );
    let last_method = MethodDef {
        name: Symbol::from("tail"),
        receiver: MethodReceiver::Instance,
        params: vec![DialectParam::positional(Symbol::from("records"))],
        body: neg_one,
        signature: Some(Ty::Fn {
            params: vec![TyParam {
                name: Symbol::from("records"),
                ty: Ty::Array { elem: Box::new(Ty::Str) },
                kind: ParamKind::Required,
            }],
            block: None,
            ret: Box::new(Ty::Str),
            effects: EffectSet::default(),
        }),
        effects: EffectSet::default(),
        enclosing_class: Some(Symbol::from("Tailer")),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    };
    let class = LibraryClass {
        name: ClassId(Symbol::from("Tailer")),
        is_module: false,
        parent: None,
        includes: vec![],
        methods: vec![last_method],
        origin: None,
    };

    let emitted = go2::emit_library_class(&class).expect("emit tailer class");

    // `records[-1]` → `records[len(records)-1]`.
    assert!(
        emitted.contains("records[len(records)-1]"),
        "negative-index missing len-minus-N rewrite:\n{emitted}",
    );
    // Regression guard: the bare `records[-1]` form must NOT survive
    // — Go rejects negative slice indices at compile time for
    // constants and at runtime for non-constants.
    assert!(
        !emitted.contains("records[-1]"),
        "raw negative index leaked into emit:\n{emitted}",
    );
}

/// `.class.X` reflection — Ruby idioms that have no Go analog:
///
/// 1. `self.class.X(args)` (instance-method chain) → enclosing-class
///    class-method bare-fn call (`<ClassName>_X(args)`). Mirrors
///    rust2's `Self::X(args)` strategy. Subclass overrides don't
///    reroute through this rewrite — they need an interface dispatch
///    later — but emitting a syntactically valid call to the
///    enclosing-class slot is enough to make the walker pass.
///
/// 2. `self.class.name` (instance-method chain) → string literal of
///    the enclosing class name. Resolves to a string at emit time so
///    interpolation into raise messages renders sensibly.
///
/// 3. Bare `name` in class-method context → same string-literal of
///    the enclosing class. Covers the `def self.X; raise ...,
///    "#{name}.X must be overridden"; end` shape that surfaces in
///    `ActiveRecord::Base#schema_columns`.
#[test]
fn class_reflection_rewrites() {
    // ---- Case 1: self.class.schema_columns (instance method)
    //
    // Body: `self.class.schema_columns`. The Send chain is
    //   Send { recv: Send { recv: SelfRef, method: "class", args:[] },
    //          method: "schema_columns", args:[] }.
    let class_chain = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(Expr::new(Span::synthetic(), ExprNode::SelfRef)),
                    method: Symbol::from("class"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                },
            )),
            method: Symbol::from("schema_columns"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let lookup_cols = MethodDef {
        name: Symbol::from("lookup_cols"),
        receiver: MethodReceiver::Instance,
        params: vec![],
        body: class_chain,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(Symbol::from("Reflect")),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    };

    // ---- Case 2: self.class.name (instance method)
    let class_name_chain = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(Expr::new(Span::synthetic(), ExprNode::SelfRef)),
                    method: Symbol::from("class"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                },
            )),
            method: Symbol::from("name"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let lookup_name = MethodDef {
        name: Symbol::from("lookup_name"),
        receiver: MethodReceiver::Instance,
        params: vec![],
        body: class_name_chain,
        signature: Some(Ty::Fn {
            params: vec![],
            block: None,
            ret: Box::new(Ty::Str),
            effects: EffectSet::default(),
        }),
        effects: EffectSet::default(),
        enclosing_class: Some(Symbol::from("Reflect")),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    };

    // ---- Case 3: bare `name` in class method context.
    //
    // Body: `name` (a 0-arg implicit-self Send). The enclosing
    // class method is `def self.diag` so EmitCtx.in_class_method = true.
    let bare_name = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: None,
            method: Symbol::from("name"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let diag = MethodDef {
        name: Symbol::from("diag"),
        receiver: MethodReceiver::Class,
        params: vec![],
        body: bare_name,
        signature: Some(Ty::Fn {
            params: vec![],
            block: None,
            ret: Box::new(Ty::Str),
            effects: EffectSet::default(),
        }),
        effects: EffectSet::default(),
        enclosing_class: Some(Symbol::from("Reflect")),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    };
    let class = LibraryClass {
        name: ClassId(Symbol::from("Reflect")),
        is_module: false,
        parent: None,
        includes: vec![],
        methods: vec![lookup_cols, lookup_name, diag],
        origin: None,
    };

    let emitted = go2::emit_library_class(&class).expect("emit reflect class");

    // Case 1 — `self.class.schema_columns` → `Reflect_schema_columns()`.
    assert!(
        emitted.contains("Reflect_schema_columns()"),
        "self.class.schema_columns missing class-method bare-fn rewrite:\n{emitted}",
    );
    // Case 2 — `self.class.name` → string literal.
    assert!(
        emitted.contains("\"Reflect\""),
        "self.class.name / bare name missing class-name string literal:\n{emitted}",
    );
    // Case 3 — bare `name` in class method context. Same `"Reflect"`
    // literal lands. The substring count covers both Case 2 and Case 3.
    assert!(
        emitted.matches("\"Reflect\"").count() >= 2,
        "bare-name-in-class-method missing class-name literal:\n{emitted}",
    );
    // Regression guards — the broken legacy emits.
    assert!(
        !emitted.contains("self.Class"),
        "self.class chain leaked into emit as self.Class field:\n{emitted}",
    );
    // Bare `name` would previously emit as the undefined identifier
    // `name` at statement position. The diag class method body must
    // not bottom out at that shape.
    assert!(
        !emitted.contains("return name\n") && !emitted.contains("return name }"),
        "bare name leaked as undefined identifier:\n{emitted}",
    );
}

/// Bare `new(args)` inside a class method (`def self.create;
/// new(attrs); end`) — Ruby's implicit receiver resolves to the
/// enclosing class, so emit must route through the synthesized
/// `New<ClassName>` constructor, not the undefined bare identifier
/// `New`. Mirrors the `Const(X).new(args)` rewrite that fires when
/// the receiver is explicit.
#[test]
fn bare_new_in_class_method_resolves_to_constructor() {
    // `def self.create(attrs); new(attrs); end`
    let create = MethodDef {
        name: Symbol::from("create"),
        receiver: MethodReceiver::Class,
        params: vec![DialectParam::positional(Symbol::from("attrs"))],
        body: Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from("new"),
                args: vec![Expr::new(
                    Span::synthetic(),
                    ExprNode::Var { id: VarId(0), name: Symbol::from("attrs") },
                )],
                block: None,
                parenthesized: true,
            },
        ),
        signature: Some(Ty::Fn {
            params: vec![TyParam {
                name: Symbol::from("attrs"),
                ty: Ty::Untyped,
                kind: ParamKind::Required,
            }],
            block: None,
            ret: Box::new(Ty::Class { id: ClassId(Symbol::from("Widget")), args: vec![] }),
            effects: EffectSet::default(),
        }),
        effects: EffectSet::default(),
        enclosing_class: Some(Symbol::from("Widget")),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    };
    let class = LibraryClass {
        name: ClassId(Symbol::from("Widget")),
        is_module: false,
        parent: None,
        includes: vec![],
        methods: vec![create],
        origin: None,
    };

    let emitted = go2::emit_library_class(&class).expect("emit widget class");

    // The bare `new(attrs)` must rewrite to the typed constructor.
    assert!(
        emitted.contains("NewWidget(attrs)"),
        "bare new(attrs) in class method missing constructor rewrite:\n{emitted}",
    );
    // Must not leave the bare identifier — Go would fail with
    // "undefined: New" at build time.
    assert!(
        !emitted.contains("return New(attrs)") && !emitted.contains(" New(attrs)\n"),
        "bare new leaked as undefined identifier `New`:\n{emitted}",
    );
}

/// Implicit-self method-call resolution: a 0-arg implicit-self
/// Send to a method DEFINED on the enclosing class must emit as
/// `self.Method()` (call). A 0-arg implicit-self Send to an
/// attr_reader-backed field must STAY `self.Field` (read). The
/// class method registry (built by `library::collect_self_methods`)
/// is what distinguishes the two; without it both would emit the
/// same (the old bug).
///
/// Synthesizes a class with one attr_accessor (`status`) and two
/// real methods (`tick` — the caller; `notify` — a no-op real
/// method called via implicit self). Tick's body reads `self.status`
/// (must stay parenless) AND calls `notify` (must gain parens).
#[test]
fn implicit_self_method_call_resolution() {
    let status_reader = MethodDef {
        name: Symbol::from("status"),
        receiver: MethodReceiver::Instance,
        params: vec![],
        body: Expr::new(
            Span::synthetic(),
            ExprNode::Ivar { name: Symbol::from("status") },
        ),
        signature: Some(Ty::Fn {
            params: vec![],
            block: None,
            ret: Box::new(Ty::Str),
            effects: EffectSet::default(),
        }),
        effects: EffectSet::default(),
        enclosing_class: Some(Symbol::from("Worker")),
        kind: AccessorKind::AttributeReader,
        is_async: false,
        mutates_self: false,
        block_param: None,
    };
    // `def notify; end` — no-op real method. Becomes
    // `func (self *Worker) notify() {}` in emit.
    let notify = MethodDef {
        name: Symbol::from("notify"),
        receiver: MethodReceiver::Instance,
        params: vec![],
        body: Expr::new(Span::synthetic(), ExprNode::Lit { value: roundhouse::Literal::Nil }),
        signature: Some(Ty::Fn {
            params: vec![],
            block: None,
            ret: Box::new(Ty::Nil),
            effects: EffectSet::default(),
        }),
        effects: EffectSet::default(),
        enclosing_class: Some(Symbol::from("Worker")),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    };
    // `def tick; s = self.status; self.notify; s; end` — Seq of:
    //   Assign(s, self.status)       — must stay self.Status (field)
    //   Send(self, notify)           — must emit self.Notify() (call)
    //   Var(s)                       — tail return
    let s_assign = Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: Symbol::from("s") },
            value: Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(Expr::new(Span::synthetic(), ExprNode::SelfRef)),
                    method: Symbol::from("status"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                },
            ),
        },
    );
    let notify_call = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(Span::synthetic(), ExprNode::SelfRef)),
            method: Symbol::from("notify"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let s_return = Expr::new(
        Span::synthetic(),
        ExprNode::Var { id: VarId(0), name: Symbol::from("s") },
    );
    let tick = MethodDef {
        name: Symbol::from("tick"),
        receiver: MethodReceiver::Instance,
        params: vec![],
        body: Expr::new(
            Span::synthetic(),
            ExprNode::Seq { exprs: vec![s_assign, notify_call, s_return] },
        ),
        signature: Some(Ty::Fn {
            params: vec![],
            block: None,
            ret: Box::new(Ty::Str),
            effects: EffectSet::default(),
        }),
        effects: EffectSet::default(),
        enclosing_class: Some(Symbol::from("Worker")),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    };
    let class = LibraryClass {
        name: ClassId(Symbol::from("Worker")),
        is_module: false,
        parent: None,
        includes: vec![],
        methods: vec![status_reader, notify, tick],
        origin: None,
    };

    let emitted = go2::emit_library_class(&class).expect("emit worker class");

    // Attr-reader stays a struct field — `self.Status` (no parens).
    // The Assign body is `s := self.Status` (PascalCase via
    // go_field_name on the ivar name).
    assert!(
        emitted.contains("s := self.Status"),
        "attr_reader-backed field accidentally promoted to method call:\n{emitted}",
    );
    // Real method gets parens — `self.Notify()` (call).
    assert!(
        emitted.contains("self.Notify()"),
        "implicit-self method call missing parens:\n{emitted}",
    );
    // Regression guard: `self.Notify` (without parens) would be a
    // field-read of a method, which Go doesn't allow.
    assert!(
        !emitted.contains("self.Notify\n"),
        "implicit-self method emitted as bare field-read:\n{emitted}",
    );
}

/// `arr.each { |x| body }` lowers to a Go `for _, x := range arr`
/// loop wrapped in an `func() interface{} { ...; return arr }()`
/// IIFE — the wrap makes the statement-shaped loop fit anywhere an
/// expression goes (assignment value, Seq middle, method tail), and
/// the receiver-returning shape matches Ruby `each` semantics so
/// callers in non-void tail position get a typed return value.
///
/// Synthesizes a one-method class so the per-method param-name
/// declaration path runs (the block param `x` must be visible in
/// the body's emit ctx so any inner assignment to `x` emits as `=`
/// — but a bare-Var body suffices for this shape check).
#[test]
fn each_array_block_shape() {
    let arr_param = Symbol::from("arr");
    // Body: `arr.each { |x| x }` — one-param block iterating
    // a Var receiver. Single-Var body is enough for the shape
    // assertion; the param-name declaration plumbing surfaces
    // in a richer body (covered by the toolchain test once a
    // real call site lands in GO_RUNTIME).
    let block = Expr::new(
        Span::synthetic(),
        ExprNode::Lambda {
            params: vec![Symbol::from("x")],
            block_param: None,
            body: Expr::new(
                Span::synthetic(),
                ExprNode::Var { id: VarId(0), name: Symbol::from("x") },
            ),
            block_style: Default::default(),
        },
    );
    let body = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Var { id: VarId(0), name: arr_param.clone() },
            )),
            method: Symbol::from("each"),
            args: vec![],
            block: Some(block),
            parenthesized: false,
        },
    );
    let method = MethodDef {
        name: Symbol::from("traverse"),
        receiver: MethodReceiver::Instance,
        params: vec![DialectParam::positional(arr_param)],
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(Symbol::from("Loop")),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    };
    let class = LibraryClass {
        name: ClassId(Symbol::from("Loop")),
        is_module: false,
        parent: None,
        includes: vec![],
        methods: vec![method],
        origin: None,
    };

    let emitted = go2::emit_library_class(&class).expect("emit each-block class");

    // Range over the receiver with the block param bound; underscore
    // discards the index since 1-param blocks ignore the position.
    // arr is param-typed `interface{}` (no RBS), so the each peephole
    // injects a `.([]any)` type assertion — Go can't range over
    // interface{} directly.
    assert!(
        emitted.contains("for _, x := range arr.([]any) {"),
        "each block missing for-range shape:\n{emitted}",
    );
    // IIFE wrap with interface{} return — keeps the each-Send a
    // total expression that's valid in every position.
    assert!(
        emitted.contains("func() interface{} {"),
        "IIFE wrap missing:\n{emitted}",
    );
    assert!(
        emitted.contains("return arr"),
        "IIFE missing receiver return for Ruby each semantics:\n{emitted}",
    );
}

/// Hash variant: `h.each { |k, v| body }` → `for k, v := range h`
/// — the 2-param shape that drives Hash iteration. Same IIFE
/// wrap as the array case.
#[test]
fn each_hash_block_shape() {
    let h_param = Symbol::from("h");
    let block = Expr::new(
        Span::synthetic(),
        ExprNode::Lambda {
            params: vec![Symbol::from("k"), Symbol::from("v")],
            block_param: None,
            body: Expr::new(
                Span::synthetic(),
                ExprNode::Var { id: VarId(0), name: Symbol::from("v") },
            ),
            block_style: Default::default(),
        },
    );
    let body = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Var { id: VarId(0), name: h_param.clone() },
            )),
            method: Symbol::from("each"),
            args: vec![],
            block: Some(block),
            parenthesized: false,
        },
    );
    let method = MethodDef {
        name: Symbol::from("traverse"),
        receiver: MethodReceiver::Instance,
        params: vec![DialectParam::positional(h_param)],
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(Symbol::from("HashLoop")),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    };
    let class = LibraryClass {
        name: ClassId(Symbol::from("HashLoop")),
        is_module: false,
        parent: None,
        includes: vec![],
        methods: vec![method],
        origin: None,
    };

    let emitted = go2::emit_library_class(&class).expect("emit each hash-block class");

    // h is param-typed `interface{}` (no RBS), 2-param block ⇒ Hash;
    // each peephole injects a `.(map[string]any)` type assertion.
    assert!(
        emitted.contains("for k, v := range h.(map[string]any) {"),
        "hash each missing for-range shape:\n{emitted}",
    );
}

/// `recv.map { |x| body }` — accumulates each block-body value into a
/// new slice. Mirrors `each` shape but with `append` instead of side-
/// effect-only iteration. Result-elem Ty isn't inferred yet, so the
/// fallback emit is `[]interface{}`.
#[test]
fn map_array_block_shape() {
    let arr_param = Symbol::from("arr");
    // Body: `arr.map { |x| x }` — identity map, enough to assert the
    // IIFE shape, range vars, and the append-tail wiring.
    let block = Expr::new(
        Span::synthetic(),
        ExprNode::Lambda {
            params: vec![Symbol::from("x")],
            block_param: None,
            body: Expr::new(
                Span::synthetic(),
                ExprNode::Var { id: VarId(0), name: Symbol::from("x") },
            ),
            block_style: Default::default(),
        },
    );
    let body = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Var { id: VarId(0), name: arr_param.clone() },
            )),
            method: Symbol::from("map"),
            args: vec![],
            block: Some(block),
            parenthesized: false,
        },
    );
    let method = MethodDef {
        name: Symbol::from("doubled"),
        receiver: MethodReceiver::Instance,
        params: vec![DialectParam::positional(arr_param)],
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(Symbol::from("Mapper")),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    };
    let class = LibraryClass {
        name: ClassId(Symbol::from("Mapper")),
        is_module: false,
        parent: None,
        includes: vec![],
        methods: vec![method],
        origin: None,
    };

    let emitted = go2::emit_library_class(&class).expect("emit map-block class");

    // IIFE wrap with concrete `[]interface{}` return — distinguishes
    // map from each (which uses `interface{}`).
    assert!(
        emitted.contains("func() []interface{} {"),
        "map IIFE missing []interface{{}} return:\n{emitted}",
    );
    // The accumulator slot.
    assert!(
        emitted.contains("out := []interface{}{}"),
        "map IIFE missing accumulator init:\n{emitted}",
    );
    // Range over the receiver, append tail expr.
    assert!(
        emitted.contains("for _, x := range arr {"),
        "map block missing for-range shape:\n{emitted}",
    );
    assert!(
        emitted.contains("out = append(out, x)"),
        "map block missing append-of-body-tail:\n{emitted}",
    );
    // Tail returns the accumulated slice.
    assert!(
        emitted.contains("return out"),
        "map IIFE missing slice return:\n{emitted}",
    );
    // Regression guard: the broken legacy shape was a bare field-read
    // `.Map` on the receiver.
    assert!(
        !emitted.contains("arr.Map"),
        "map leaked as bare field-read on receiver:\n{emitted}",
    );
}

/// `def _adapter_insert; end` — Ruby's empty body returns nil. If the
/// method's return type is non-void (e.g. `int64` for the overridable
/// adapter stub), the Go emit needs a synthesized zero-value return
/// or `go vet` rejects the function as "missing return".
///
/// This covers two shapes:
/// 1. Body is a bare `Lit::Nil` (the Symbol body that `def m; end`
///    parses to today).
/// 2. Body is `Seq { exprs: [Lit::Nil] }` — single-statement Seq with
///    just nil. Belt-and-suspenders since the lowerer wraps some
///    bodies and the gap is the resulting Go is the same shape.
#[test]
fn empty_body_with_nonvoid_return_synthesizes_zero_value() {
    let make_method = |name: &str, body: Expr, ret: Ty| MethodDef {
        name: Symbol::from(name),
        receiver: MethodReceiver::Instance,
        params: vec![],
        body,
        signature: Some(Ty::Fn {
            params: vec![],
            block: None,
            ret: Box::new(ret),
            effects: EffectSet::default(),
        }),
        effects: EffectSet::default(),
        enclosing_class: Some(Symbol::from("Stub")),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    };

    // Bare Lit::Nil body + int64 return.
    let m1 = make_method(
        "insert_id",
        Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
        Ty::Int,
    );
    // Seq-wrapped Lit::Nil body + string return.
    let m2 = make_method(
        "table_name",
        Expr::new(
            Span::synthetic(),
            ExprNode::Seq {
                exprs: vec![Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit { value: Literal::Nil },
                )],
            },
        ),
        Ty::Str,
    );

    let class = LibraryClass {
        name: ClassId(Symbol::from("Stub")),
        is_module: false,
        parent: None,
        includes: vec![],
        methods: vec![m1, m2],
        origin: None,
    };

    let emitted = go2::emit_library_class(&class).expect("emit empty-body class");

    // m1: int64 return → `return 0`. Instance methods emit PascalCase
    // to match the call-site shape (`r.InsertID()` from
    // `go2_method_ident`).
    assert!(
        emitted.contains("InsertID() int64 {\n\treturn 0\n}"),
        "Lit::Nil + int64 return missing zero-value synth:\n{emitted}",
    );
    // m2: string return → `return ""` (NOT `return nil` — empty Seq
    // bodies must collapse to the typed zero value).
    assert!(
        emitted.contains("TableName() string {\n\treturn \"\"\n}"),
        "Seq[Nil] + string return missing zero-value synth:\n{emitted}",
    );
    // Regression guard: must not emit `return nil` against a typed
    // return — Go rejects it.
    assert!(
        !emitted.contains("TableName() string {\n\treturn nil\n}"),
        "Seq[Nil] body emitted `return nil` instead of zero value:\n{emitted}",
    );
}

/// Empty Array literal whose Expr Ty is set by the analyzer must
/// emit with the concrete element type, not the `[]interface{}`
/// fallback. The base case: `@errors = []` against an `Array[String]`
/// field needs `[]string{}` so it matches the destination slot.
///
/// Same family for Hash: a Hash literal in return position against a
/// `map[string]string` return type needs the typed map literal.
#[test]
fn typed_empty_literals_back_propagate() {
    // Helper: build a typed-empty Array Expr.
    let typed_array_lit = |elem: Ty| {
        let mut e = Expr::new(
            Span::synthetic(),
            ExprNode::Array { elements: vec![], style: Default::default() },
        );
        e.ty = Some(Ty::Array { elem: Box::new(elem) });
        e
    };
    // Helper: build a typed-empty Hash Expr.
    let typed_hash_lit = |k: Ty, v: Ty| {
        let mut e = Expr::new(
            Span::synthetic(),
            ExprNode::Hash { entries: vec![], kwargs: false },
        );
        e.ty = Some(Ty::Hash { key: Box::new(k), value: Box::new(v) });
        e
    };

    // `def errors; []; end` — body is a typed-empty Array literal,
    // signature returns `Array[String]`. The literal's `.ty` carries
    // the elem; emit must produce `[]string{}` (not `[]interface{}{}`).
    let errors = MethodDef {
        name: Symbol::from("errors"),
        receiver: MethodReceiver::Instance,
        params: vec![],
        body: typed_array_lit(Ty::Str),
        signature: Some(Ty::Fn {
            params: vec![],
            block: None,
            ret: Box::new(Ty::Array { elem: Box::new(Ty::Str) }),
            effects: EffectSet::default(),
        }),
        effects: EffectSet::default(),
        enclosing_class: Some(Symbol::from("Schema")),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    };
    // `def lookup; {}; end` — same shape for Hash, against return
    // `Hash[String, String]`. Emit must produce `map[string]string{}`.
    let lookup = MethodDef {
        name: Symbol::from("lookup"),
        receiver: MethodReceiver::Instance,
        params: vec![],
        body: typed_hash_lit(Ty::Str, Ty::Str),
        signature: Some(Ty::Fn {
            params: vec![],
            block: None,
            ret: Box::new(Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Str) }),
            effects: EffectSet::default(),
        }),
        effects: EffectSet::default(),
        enclosing_class: Some(Symbol::from("Schema")),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    };

    let class = LibraryClass {
        name: ClassId(Symbol::from("Schema")),
        is_module: false,
        parent: None,
        includes: vec![],
        methods: vec![errors, lookup],
        origin: None,
    };

    let emitted = go2::emit_library_class(&class).expect("emit typed-literal class");

    assert!(
        emitted.contains("return []string{}"),
        "typed empty Array literal missing typed elem:\n{emitted}",
    );
    assert!(
        emitted.contains("return map[string]string{}"),
        "typed empty Hash literal missing typed kv:\n{emitted}",
    );
    // Regression guards: the untyped fallbacks must NOT fire when
    // `.ty` is set.
    assert!(
        !emitted.contains("return []interface{}{}"),
        "typed Array literal fell through to []interface{{}} fallback:\n{emitted}",
    );
    assert!(
        !emitted.contains("return map[string]interface{}{}"),
        "typed Hash literal fell through to []interface{{}} fallback:\n{emitted}",
    );
}

#[test]
fn json_builder_v2_shape() {
    let app = ingest_with_analyzer();
    let files = go2::emit_overlay_files(&app);
    let json_builder = find_file(&files, "app/v2/json_builder.go")
        .expect("v2/json_builder.go missing from overlay output");
    let text = &json_builder.content;

    // Module-level const initializers — Hash literal and regex —
    // emit as real values, not `var X interface{} = nil` placeholders.
    assert!(
        text.contains("var ESCAPES = map[string]string{"),
        "ESCAPES missing typed-map initializer:\n{text}",
    );
    assert!(
        text.contains("var ESCAPE_PATTERN = regexp.MustCompile("),
        "ESCAPE_PATTERN missing regexp.MustCompile initializer:\n{text}",
    );

    // Regex inside-class escape rewrite — `\b`/`\f` translate to
    // `\x08`/`\x0c` since Go's regexp rejects them inside `[]`.
    assert!(
        text.contains("\\\\x08") && text.contains("\\\\x0c"),
        "ESCAPE_PATTERN missing \\b/\\f → \\x08/\\x0c rewrite:\n{text}",
    );

    // gsub peephole — `s.gsub(REGEX, HASH)` → `REGEX.ReplaceAllStringFunc(s, func ...)`.
    assert!(
        text.contains("ESCAPE_PATTERN.ReplaceAllStringFunc(s, func(m string) string"),
        "encode_string missing gsub → ReplaceAllStringFunc translation:\n{text}",
    );

    // is_a? branches — singletons collapse to equality, mapped Tys
    // use type-assert if-init with branch-scoped ident substitution.
    assert!(text.contains("if v == true"), "TrueClass branch missing:\n{text}");
    assert!(
        text.contains("if i, ok := v.(int64); ok"),
        "Integer branch missing typed init:\n{text}",
    );
    assert!(
        text.contains("if s, ok := v.(string); ok"),
        "String branch missing typed init:\n{text}",
    );

    // Union[Str, Nil] now renders as Go `string` directly (empty-
    // as-nil convention in go_ty_stub), so the nil-narrow no
    // longer needs a `s.(string)` assertion — `s` is ALREADY a
    // string. The early-return shape collapses to `if s == ""`.
    // (The encode_value `is_a?` branches still emit `.(string)`
    // because their `v` is typed Untyped, not Union[Str, Nil].)
    assert!(
        text.contains("if s == \"\""),
        "encode_datetime missing empty-as-nil early return:\n{text}",
    );
}

#[test]
fn router_v2_shape() {
    let app = ingest_with_analyzer();
    let files = go2::emit_overlay_files(&app);
    let router = find_file(&files, "app/v2/router.go")
        .expect("v2/router.go missing from overlay output");
    let text = &router.content;

    // Class shape — attr_reader → struct fields, constructors.
    assert!(
        text.contains("type ActionDispatchRouterRoute struct {")
            && text.contains("Verb string")
            && text.contains("Pattern string"),
        "Route struct missing typed fields:\n{text}",
    );
    assert!(
        text.contains("func NewActionDispatchRouterRoute("),
        "Route constructor missing:\n{text}",
    );
    assert!(
        text.contains("PathParams map[string]string"),
        "MatchResult missing map[string]string field for path_params:\n{text}",
    );

    // Class methods — receive table as `[]*Route`, return `*MatchResult`.
    assert!(
        text.contains("table []*ActionDispatchRouterRoute"),
        "match() param missing typed slice:\n{text}",
    );
    assert!(
        text.contains(") *ActionDispatchRouterMatchResult {"),
        "match() return type not collapsed from nilable T:\n{text}",
    );

    // String method translations.
    assert!(
        text.contains("strings.ToUpper("),
        "method.to_s.upcase missing strings.ToUpper:\n{text}",
    );
    assert!(
        text.contains("strings.Split("),
        "split missing strings.Split:\n{text}",
    );
    assert!(
        text.contains("strings.HasPrefix("),
        "start_with? missing strings.HasPrefix:\n{text}",
    );

    // While loop + i++ + []= index assign.
    assert!(text.contains("for i < int64(len("), "while loop missing for-emit:\n{text}");
    assert!(text.contains("i = i + 1"), "i += 1 missing reassign emit:\n{text}");
    assert!(
        text.contains("params[pp[1:]] = ap"),
        "[]= missing index-assign emit:\n{text}",
    );

    // `unless` → inverted if (no bare-nil then-branch).
    assert!(
        text.contains("if !(params == nil)"),
        "unless missing inverted-if emit:\n{text}",
    );
}

#[test]
fn inflector_v2_shape() {
    let app = ingest_with_analyzer();
    let files = go2::emit_overlay_files(&app);
    let inflector = find_file(&files, "app/v2/inflector.go")
        .expect("v2/inflector.go missing from overlay output");

    let text = &inflector.content;
    // Package + import — `fmt.Sprintf` is referenced by the
    // Sprintf-emitted body so the file must `import "fmt"`.
    assert!(
        text.contains("package v2"),
        "v2/inflector.go missing `package v2` declaration:\n{text}",
    );
    assert!(
        text.contains("import \"fmt\""),
        "v2/inflector.go missing `import \"fmt\"`:\n{text}",
    );

    // Type declaration — Inflector is a Mode::Library entry so it
    // emits as an empty struct alongside its methods.
    assert!(
        text.contains("type Inflector struct{}"),
        "v2/inflector.go missing `type Inflector struct{{}}`:\n{text}",
    );

    // Function signature — class-method receiver flattens to a bare
    // `Inflector_pluralize`, with sig-derived `count int64, word string`
    // and return type `string`.
    assert!(
        text.contains("func Inflector_pluralize(count int64, word string) string"),
        "v2/inflector.go missing typed pluralize signature:\n{text}",
    );

    // Body — Ruby `count == 1 ? ... : ...` ternary lowered to Go
    // `if count == 1 { return ... } else { return ... }`. Both
    // branches return a `fmt.Sprintf(...)` call.
    assert!(
        text.contains("if count == 1 {"),
        "v2/inflector.go missing `if count == 1` branch:\n{text}",
    );
    assert!(
        text.contains("return fmt.Sprintf("),
        "v2/inflector.go body missing `return fmt.Sprintf(...)`:\n{text}",
    );
}

/// The `nil_check_to_comma_ok` lowerer rewrites the
/// `v = m[k]; if !v.nil? { body }` shape into a synthesized Send
/// with method `_go_try_fetch`. None of the current GO_RUNTIME
/// entries (inflector, json_builder, router, active_record/base)
/// hit this shape, so the end-to-end smoke tests exercise only the
/// identity path. This synthesizes the pattern directly to verify
/// the rewrite fires and produces the expected IR shape.
#[test]
fn nil_check_to_comma_ok_rewrites_pair() {
    use roundhouse::emit::go2::lower::{lower_for_go, nil_check_to_comma_ok};

    // Build IR for:
    //   def f(other)
    //     v = other["k"]
    //     @field = v if !v.nil?
    //   end
    // with `other` typed `Hash[String, String]` so the receiver-Ty
    // gate fires.
    let span = Span::synthetic();
    let other_ty = Ty::Hash {
        key: Box::new(Ty::Str),
        value: Box::new(Ty::Str),
    };
    let mut other_var = Expr::new(span, ExprNode::Var {
        id: VarId(0),
        name: Symbol::from("other"),
    });
    other_var.ty = Some(other_ty.clone());

    let index_send = Expr::new(span, ExprNode::Send {
        recv: Some(other_var),
        method: Symbol::from("[]"),
        args: vec![Expr::new(span, ExprNode::Lit { value: Literal::Str { value: "k".into() } })],
        block: None,
        parenthesized: true,
    });
    let assign = Expr::new(span, ExprNode::Assign {
        target: LValue::Var { id: VarId(1), name: Symbol::from("v") },
        value: index_send,
    });

    let v_var = Expr::new(span, ExprNode::Var { id: VarId(1), name: Symbol::from("v") });
    let nil_check = Expr::new(span, ExprNode::Send {
        recv: Some(v_var),
        method: Symbol::from("nil?"),
        args: vec![],
        block: None,
        parenthesized: false,
    });
    let not_nil = Expr::new(span, ExprNode::Send {
        recv: Some(nil_check),
        method: Symbol::from("!"),
        args: vec![],
        block: None,
        parenthesized: false,
    });
    let body = Expr::new(span, ExprNode::Assign {
        target: LValue::Ivar { name: Symbol::from("field") },
        value: Expr::new(span, ExprNode::Var { id: VarId(1), name: Symbol::from("v") }),
    });
    let if_expr = Expr::new(span, ExprNode::If {
        cond: not_nil,
        then_branch: body,
        else_branch: Expr::new(span, ExprNode::Lit { value: Literal::Nil }),
    });

    let body_seq = Expr::new(span, ExprNode::Seq { exprs: vec![assign, if_expr] });

    let method = MethodDef {
        name: Symbol::from("f"),
        receiver: MethodReceiver::Instance,
        params: vec![DialectParam::positional(Symbol::from("other"))],
        body: body_seq,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(Symbol::from("StubFlash")),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    };
    let class = LibraryClass {
        name: ClassId(Symbol::from("StubFlash")),
        is_module: false,
        parent: None,
        includes: vec![],
        methods: vec![method],
        origin: None,
    };

    let out = lower_for_go(vec![class]);
    let method = &out[0].methods[0];
    let exprs = match &*method.body.node {
        ExprNode::Seq { exprs } => exprs,
        _ => panic!("expected Seq body, got {:?}", method.body.node),
    };
    assert_eq!(exprs.len(), 1, "pair should collapse to one synthesized Send");

    let synth = &exprs[0];
    let (method_name, has_block) = match &*synth.node {
        ExprNode::Send { method, block, .. } => (method.as_str(), block.is_some()),
        _ => panic!("expected Send, got {:?}", synth.node),
    };
    assert_eq!(method_name, nil_check_to_comma_ok::SENTINEL_METHOD);
    assert!(has_block, "synthesized Send should carry a Lambda block");
}

/// Receiver-Ty gate: when the receiver isn't a Hash (e.g., a class
/// instance), the lowerer must NOT rewrite — `self[k]` on a struct
/// is a method call, not a map index, and the comma-ok emit would
/// produce invalid Go.
#[test]
fn nil_check_to_comma_ok_skips_non_hash_receiver() {
    use roundhouse::emit::go2::lower::lower_for_go;

    let span = Span::synthetic();
    // Receiver typed as a Class — should NOT match.
    let mut self_ref = Expr::new(span, ExprNode::SelfRef);
    self_ref.ty = Some(Ty::Class { id: ClassId(Symbol::from("StubFlash")), args: vec![] });

    let index_send = Expr::new(span, ExprNode::Send {
        recv: Some(self_ref),
        method: Symbol::from("[]"),
        args: vec![Expr::new(span, ExprNode::Lit { value: Literal::Str { value: "k".into() } })],
        block: None,
        parenthesized: true,
    });
    let assign = Expr::new(span, ExprNode::Assign {
        target: LValue::Var { id: VarId(0), name: Symbol::from("v") },
        value: index_send,
    });

    let v_var = Expr::new(span, ExprNode::Var { id: VarId(0), name: Symbol::from("v") });
    let not_nil = Expr::new(span, ExprNode::Send {
        recv: Some(Expr::new(span, ExprNode::Send {
            recv: Some(v_var),
            method: Symbol::from("nil?"),
            args: vec![],
            block: None,
            parenthesized: false,
        })),
        method: Symbol::from("!"),
        args: vec![],
        block: None,
        parenthesized: false,
    });
    let if_expr = Expr::new(span, ExprNode::If {
        cond: not_nil,
        then_branch: Expr::new(span, ExprNode::Lit { value: Literal::Nil }),
        else_branch: Expr::new(span, ExprNode::Lit { value: Literal::Nil }),
    });
    let body_seq = Expr::new(span, ExprNode::Seq { exprs: vec![assign, if_expr] });

    let method = MethodDef {
        name: Symbol::from("f"),
        receiver: MethodReceiver::Instance,
        params: vec![],
        body: body_seq,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(Symbol::from("StubFlash")),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    };
    let class = LibraryClass {
        name: ClassId(Symbol::from("StubFlash")),
        is_module: false,
        parent: None,
        includes: vec![],
        methods: vec![method],
        origin: None,
    };

    let out = lower_for_go(vec![class]);
    let exprs = match &*out[0].methods[0].body.node {
        ExprNode::Seq { exprs } => exprs,
        _ => panic!("expected Seq body"),
    };
    // Must stay TWO statements — no rewrite.
    assert_eq!(exprs.len(), 2, "non-Hash receiver must not trigger rewrite");
}

fn emit_to_scratch() -> PathBuf {
    let scratch = std::env::temp_dir().join("roundhouse-go2-smoke");
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).expect("clean scratch");
    }
    std::fs::create_dir_all(&scratch).expect("mkdir scratch");

    let app = ingest_with_analyzer();
    let mut files = go::emit(&app);
    files.extend(go2::emit_overlay_files(&app));

    for f in &files {
        let path = scratch.join(&f.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir parent");
        }
        std::fs::write(&path, &f.content).expect("write file");
    }
    scratch
}

#[test]
#[ignore]
fn inflector_v2_compiles_and_runs() {
    let scratch = emit_to_scratch();

    // Pin the behavioral contract — a Go test alongside the emitted
    // v2/inflector.go that exercises Inflector_pluralize.
    let smoke = "package v2\n\
                 \n\
                 import \"testing\"\n\
                 \n\
                 func TestInflectorPluralize_Smoke(t *testing.T) {\n\
                 \tcases := []struct{ count int64; word, want string }{\n\
                 \t\t{1, \"post\", \"1 post\"},\n\
                 \t\t{0, \"post\", \"0 posts\"},\n\
                 \t\t{5, \"post\", \"5 posts\"},\n\
                 \t\t{2, \"comment\", \"2 comments\"},\n\
                 \t}\n\
                 \tfor _, c := range cases {\n\
                 \t\tgot := Inflector_pluralize(c.count, c.word)\n\
                 \t\tif got != c.want {\n\
                 \t\t\tt.Errorf(\"Inflector_pluralize(%d,%q) = %q, want %q\", c.count, c.word, got, c.want)\n\
                 \t\t}\n\
                 \t}\n\
                 }\n";
    std::fs::write(scratch.join("app/v2/inflector_smoke_test.go"), smoke)
        .expect("write smoke test");

    // `go mod tidy` to populate go.sum from go.mod. Mirrors
    // tests/go_toolchain.rs.
    let tidy = Command::new("go")
        .arg("mod")
        .arg("tidy")
        .current_dir(&scratch)
        .output()
        .expect("run go mod tidy");
    assert!(
        tidy.status.success(),
        "go mod tidy failed:\n=== stderr ===\n{}",
        String::from_utf8_lossy(&tidy.stderr),
    );

    // `go vet ./app/v2` — parses + type-checks just the overlay
    // package. Scoped narrow so a legacy app/ regression doesn't
    // mask a v2 success or vice-versa.
    let vet = Command::new("go")
        .arg("vet")
        .arg("./app/v2")
        .current_dir(&scratch)
        .output()
        .expect("run go vet ./app/v2");
    assert!(
        vet.status.success(),
        "go vet ./app/v2 failed at {}:\n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&vet.stderr),
    );

    // JsonBuilder smoke — encode_string, encode_value, encode_datetime
    // behavior pinned against the emitted bodies.
    let json_smoke = "package v2\n\
                      \n\
                      import \"testing\"\n\
                      \n\
                      func TestJsonBuilder_EncodeValue_Smoke(t *testing.T) {\n\
                      \tcases := []struct{ in interface{}; want string }{\n\
                      \t\t{nil, \"null\"},\n\
                      \t\t{true, \"true\"},\n\
                      \t\t{false, \"false\"},\n\
                      \t\t{int64(42), \"42\"},\n\
                      \t\t{\"hi\", `\"hi\"`},\n\
                      \t}\n\
                      \tfor _, c := range cases {\n\
                      \t\tif got := JsonBuilder_encode_value(c.in); got != c.want {\n\
                      \t\t\tt.Errorf(\"encode_value(%v) = %q, want %q\", c.in, got, c.want)\n\
                      \t\t}\n\
                      \t}\n\
                      }\n\
                      \n\
                      func TestJsonBuilder_EncodeString_Smoke(t *testing.T) {\n\
                      \tif got := JsonBuilder_encode_string(`a\"b`); got != `a\\\"b` {\n\
                      \t\tt.Errorf(`encode_string(a\"b) = %q, want a\\\"b`, got)\n\
                      \t}\n\
                      \tif got := JsonBuilder_encode_string(\"a\\nb\"); got != `a\\nb` {\n\
                      \t\tt.Errorf(\"encode_string(a\\\\nb) = %q, want a\\\\nb\", got)\n\
                      \t}\n\
                      }\n";
    std::fs::write(
        scratch.join("app/v2/json_builder_smoke_test.go"),
        json_smoke,
    )
    .expect("write json_builder smoke");

    // Router smoke — pattern matching + table dispatch.
    let router_smoke = "package v2\n\
                        \n\
                        import \"testing\"\n\
                        \n\
                        func TestRouter_MatchPattern_Smoke(t *testing.T) {\n\
                        \tgot := ActionDispatchRouter_match_pattern(\"/articles/:id\", \"/articles/42\")\n\
                        \tif got == nil || got[\"id\"] != \"42\" {\n\
                        \t\tt.Fatalf(\"match_pattern result wrong: %#v\", got)\n\
                        \t}\n\
                        }\n\
                        \n\
                        func TestRouter_Match_Smoke(t *testing.T) {\n\
                        \ttable := []*ActionDispatchRouterRoute{\n\
                        \t\tNewActionDispatchRouterRoute(\"GET\", \"/articles\", \"articles\", \"index\"),\n\
                        \t\tNewActionDispatchRouterRoute(\"GET\", \"/articles/:id\", \"articles\", \"show\"),\n\
                        \t}\n\
                        \tres := ActionDispatchRouter_match(\"GET\", \"/articles/7\", table)\n\
                        \tif res == nil || res.Action != \"show\" || res.PathParams[\"id\"] != \"7\" {\n\
                        \t\tt.Fatalf(\"match result wrong: %#v\", res)\n\
                        \t}\n\
                        \tif ActionDispatchRouter_match(\"POST\", \"/articles/7\", table) != nil {\n\
                        \t\tt.Error(\"expected nil for unmatched method\")\n\
                        \t}\n\
                        }\n";
    std::fs::write(scratch.join("app/v2/router_smoke_test.go"), router_smoke)
        .expect("write router smoke");

    // Slots smoke — per-goroutine isolation of the content_for / yield
    // store. Pins the regression-test acceptance criterion from issue
    // #7 section 1: if a future change reverts to a shared map (or the
    // goroutine-ID scheme breaks), this fails fast on every commit
    // that runs --ignored. Each goroutine sets its own title, yields
    // to let others interleave, then re-reads — under a shared map
    // the read would surface a peer goroutine's value.
    let slots_smoke = "package v2\n\
                       \n\
                       import (\n\
                       \t\"fmt\"\n\
                       \t\"runtime\"\n\
                       \t\"sync\"\n\
                       \t\"testing\"\n\
                       )\n\
                       \n\
                       func TestSlots_PerGoroutineIsolation_Smoke(t *testing.T) {\n\
                       \tconst N = 200\n\
                       \tvar wg sync.WaitGroup\n\
                       \tfor i := 0; i < N; i++ {\n\
                       \t\twg.Add(1)\n\
                       \t\tgo func(i int) {\n\
                       \t\t\tdefer wg.Done()\n\
                       \t\t\ttitle := fmt.Sprintf(\"title-%d\", i)\n\
                       \t\t\tActionViewViewHelpers_reset_slots_bang()\n\
                       \t\t\tActionViewViewHelpers_content_for_set(\"title\", title)\n\
                       \t\t\t// Yield so other goroutines get scheduled\n\
                       \t\t\t// between set and read — under a shared map\n\
                       \t\t\t// the read would observe a peer's title.\n\
                       \t\t\tfor j := 0; j < 5; j++ {\n\
                       \t\t\t\truntime.Gosched()\n\
                       \t\t\t}\n\
                       \t\t\tif got := ActionViewViewHelpers_content_for_get(\"title\"); got != title {\n\
                       \t\t\t\tt.Errorf(\"goroutine %d: get(title) = %q, want %q\", i, got, title)\n\
                       \t\t\t}\n\
                       \t\t\tif got := ActionViewViewHelpers_get_slot(\"title\"); got != title {\n\
                       \t\t\t\tt.Errorf(\"goroutine %d: get_slot(title) = %q, want %q\", i, got, title)\n\
                       \t\t\t}\n\
                       \t\t}(i)\n\
                       \t}\n\
                       \twg.Wait()\n\
                       }\n\
                       \n\
                       func TestSlots_YieldBodyIsolation_Smoke(t *testing.T) {\n\
                       \tconst N = 200\n\
                       \tvar wg sync.WaitGroup\n\
                       \tfor i := 0; i < N; i++ {\n\
                       \t\twg.Add(1)\n\
                       \t\tgo func(i int) {\n\
                       \t\t\tdefer wg.Done()\n\
                       \t\t\tbody := fmt.Sprintf(\"<body>%d</body>\", i)\n\
                       \t\t\tActionViewViewHelpers_reset_slots_bang()\n\
                       \t\t\tActionViewViewHelpers_set_yield(body)\n\
                       \t\t\tfor j := 0; j < 5; j++ {\n\
                       \t\t\t\truntime.Gosched()\n\
                       \t\t\t}\n\
                       \t\t\tif got := ActionViewViewHelpers_get_yield(); got != body {\n\
                       \t\t\t\tt.Errorf(\"goroutine %d: get_yield = %q, want %q\", i, got, body)\n\
                       \t\t\t}\n\
                       \t\t}(i)\n\
                       \t}\n\
                       \twg.Wait()\n\
                       }\n";
    std::fs::write(scratch.join("app/v2/slots_smoke_test.go"), slots_smoke)
        .expect("write slots smoke");

    // FrameworkTestAdapter CRUD smoke removed in tandem with the
    // per-target adapter cleanup. A follow-on session will restore an
    // adapter smoke once base_test is wired against each target's
    // real sqlite stack.

    // `go test ./app/v2` — runs the smoke tests against the emitted
    // Inflector_pluralize, JsonBuilder_*, and Router.
    let test = Command::new("go")
        .arg("test")
        .arg("./app/v2")
        .current_dir(&scratch)
        .output()
        .expect("run go test ./app/v2");
    assert!(
        test.status.success(),
        "go test ./app/v2 failed at {}:\n=== stdout ===\n{}\n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&test.stdout),
        String::from_utf8_lossy(&test.stderr),
    );
}

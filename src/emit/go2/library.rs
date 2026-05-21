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
    let name = sanitize_type_name(class.name.0.as_str());
    let mut out = String::new();

    // Discover the struct's field layout. `attr_reader` / `attr_writer`
    // methods synthesize MethodDefs whose signature carries the field
    // type — those become the exported struct fields. Initialize-only
    // ivars (assigned but no reader/writer) aren't reflected as Go
    // fields yet; they'd surface as missing-symbol errors at use,
    // which is fine inventory.
    let fields = collect_fields(&class.methods);
    if fields.is_empty() {
        out.push_str(&format!("type {name} struct{{}}\n\n"));
    } else {
        out.push_str(&format!("type {name} struct {{\n"));
        for f in &fields {
            out.push_str(&format!("\t{} {}\n", f.pascal_name, f.go_ty));
        }
        out.push_str("}\n\n");
    }

    // Constructor synthesis. When an `initialize` method is present,
    // emit `New<Name>(...)` returning a pointer to the struct,
    // populated via field-by-field assignment. The original
    // `initialize` method is NOT emitted as a method on the type;
    // its body becomes the constructor body.
    if let Some(init) = class.methods.iter().find(|m| {
        matches!(m.receiver, MethodReceiver::Instance) && m.name.as_str() == "initialize"
    }) {
        out.push_str(&emit_constructor(&name, init));
        out.push('\n');
    }

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
        out.push_str(&emit_method(&name, m));
        out.push('\n');
    }
    Ok(out)
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

/// Walk methods and gather one Field per attr_reader / attr_writer.
/// The signature's return type (reader) or single param type (writer)
/// provides the Go type. If both forms exist for the same name, the
/// reader wins; deduplication is by Ruby name.
fn collect_fields(methods: &[MethodDef]) -> Vec<Field> {
    let mut out: Vec<Field> = Vec::new();
    for m in methods {
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
    out
}

/// Emit `func New<Name>(params...) *<Name> { return &<Name>{...} }`
/// from an `initialize` MethodDef. Handles the simple shape where
/// each body Assign is `@<name> = <var>` — fields populate directly
/// from the matching positional param. Falls back to a build-then-
/// assign form when the body shape is more complex.
fn emit_constructor(class_name: &str, init: &MethodDef) -> String {
    let params = render_params(init);
    let mut out = format!("func New{class_name}({params}) *{class_name} {{\n");

    // Try the simple-shape detection: every body expr is
    // `Assign { target: Ivar(name), value: Var(name) }`, and the Var
    // name matches the Ivar name. If so, emit `return &Class{Name: name, ...}`.
    if let Some(literal) = try_field_init_literal(class_name, &init.body) {
        out.push_str(&format!("\treturn {literal}\n"));
    } else {
        // Fallback: declare a fresh receiver, walk the body with
        // ctx.in_class_method=false so `self.X = …` writes resolve,
        // then return it. Currently runtime/ruby/ initializers are
        // all simple shape; this path is dead weight today but
        // keeps the emit shape total.
        out.push_str(&format!("\tself := &{class_name}{{}}\n"));
        let ctx = EmitCtx::none();
        for p in &init.params {
            ctx.declare_param(p.name.as_str());
        }
        let body = emit_return_body(&ctx, &init.body);
        out.push_str(&body);
        out.push_str("\treturn self\n");
    }
    out.push_str("}\n");
    out
}

/// Pattern-match the simple-shape constructor body (`Seq` of
/// `@ivar = var` assigns where `var` matches the ivar's Pascal field).
/// Returns the struct literal text on success.
fn try_field_init_literal(class_name: &str, body: &Expr) -> Option<String> {
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
    let parts = bindings
        .iter()
        .map(|(f, v)| format!("{f}: {v}"))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!("&{class_name}{{{parts}}}"))
}

/// `ActionController::Base` → `ActionControllerBase`. Go identifiers
/// can't contain `::` (Ruby's namespace separator); strip it so the
/// emitted type at least file-parses.
fn sanitize_type_name(name: &str) -> String {
    name.replace("::", "")
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

pub fn emit_module(methods: &[MethodDef]) -> Result<String, String> {
    // Mode::Module — no enclosing class; module-level methods emit
    // as bare functions. `SelfRef` inside them has no class context,
    // so the walker surfaces a TODO marker if it appears.
    let mut out = String::new();
    for m in methods {
        // Fresh ctx per method so `declared` doesn't leak between
        // methods. Seed with param names so param re-assignment
        // emits as `=`.
        let ctx = EmitCtx::none();
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

fn emit_method(class_name: &str, m: &MethodDef) -> String {
    let params = render_params(m);
    let ret = render_return(m);
    let receiver = match m.receiver {
        MethodReceiver::Instance => format!("(self *{class_name}) "),
        MethodReceiver::Class => String::new(),
    };
    let method = sanitize_method_name(m.name.as_str());
    let class_method_name = match m.receiver {
        MethodReceiver::Instance => method.clone(),
        // Class methods emit as bare functions prefixed with the
        // class name (Go has no class-method dispatch). Concrete
        // call sites would reference `Foo_bar(...)`.
        MethodReceiver::Class => format!("{class_name}_{method}"),
    };
    // Build per-method context so the body walker can resolve
    // `SelfRef` against the right enclosing class + method receiver.
    // Seed `declared` with the method's parameter names so any
    // assignment to a param emits as `=`, not `:=`.
    let ctx = EmitCtx {
        class_name: Some(class_name.to_string()),
        in_class_method: matches!(m.receiver, MethodReceiver::Class),
        var_renames: std::collections::HashMap::new(),
        declared: std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashSet::new())),
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
fn render_body(ctx: &EmitCtx, m: &MethodDef) -> String {
    emit_return_body(ctx, &m.body)
}

/// Avoid emitting Go reserved words as parameter names. Adds a `_`
/// suffix to any clash; preserves all others unchanged.
fn sanitize(name: &str) -> String {
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

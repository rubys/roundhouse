//! Generic `LibraryClass` → Elixir emit.
//!
//! Mirrors `src/emit/go2/library.rs` in shape, mapped to Elixir's
//! functional/immutable model:
//!
//! - A Ruby `class`/`module` becomes a `defmodule`.
//! - A module-singleton (a `module` whose methods are all
//!   `self.`-receivers — e.g. `Inflector`, `JsonBuilder`) becomes a
//!   module of functions; Ruby `def self.foo` → Elixir `def foo`.
//! - A normal class becomes a module with a `defstruct` payload;
//!   instance methods thread the record as the first param
//!   (`def foo(record, …)`), class methods stay bare.
//! - Inheritance (`parent`) is ignored — the lowerer linearizes method
//!   overrides onto each class (same as rust2).
//!
//! Each class emits its OWN `defmodule V2.<DottedName> do … end`, named
//! from its (fully-qualified) `ClassId` with the `V2.` overlay prefix —
//! so a multi-class file (`action_dispatch/router.rb` →
//! `V2.ActionDispatch.Router.Route` / `.MatchResult` / `.Router`) emits
//! three sibling modules. Module-level constants don't appear in
//! `emit_library_class` (they're parsed separately); they're injected
//! INTO their owning module by `runtime_loader::elixir_wrap_namespace`,
//! because Elixir module attributes don't cross module boundaries and
//! Elixir has no file-level constants.

use std::fmt::Write;

use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver};
use crate::expr::{Expr, ExprNode, LValue, Literal};

use super::expr;

/// Map a `ClassId` to its emitted Elixir module name (with the `V2.`
/// overlay prefix). `ActiveRecord::Base` → `V2.ActiveRecord.Base`.
pub(super) fn v2_module_name(class: &str) -> String {
    format!("V2.{}", class.replace("::", "."))
}

/// Emit a `LibraryClass` as a full Elixir `defmodule V2.<DottedName> do
/// … end` (trailing newline included).
pub fn emit_library_class(class: &LibraryClass) -> Result<String, String> {
    let v2_name = v2_module_name(class.name.0.as_str());
    let body = emit_class_body(class, &v2_name)?;
    Ok(format!("defmodule {v2_name} do\n{body}end\n"))
}

/// The body (def/defstruct lines, indented one level) that goes inside
/// the `defmodule`.
fn emit_class_body(class: &LibraryClass, v2_name: &str) -> Result<String, String> {
    let is_module_singleton = class.is_module
        && !class.methods.is_empty()
        && class
            .methods
            .iter()
            .all(|m| matches!(m.receiver, MethodReceiver::Class));

    let mut out = String::new();

    if !is_module_singleton {
        // Struct payload: attr-declared fields plus any field the
        // mutation-threaded bodies touch (via the `__field__` /
        // `__struct_put__` bridges — session's `@data` has no attr).
        let fields = struct_fields(class);
        if !fields.is_empty() {
            let decls = fields
                .iter()
                .map(|f| format!(":{f}"))
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(out, "  defstruct [{decls}]").unwrap();
            out.push('\n');
        }
    }

    for m in &class.methods {
        if matches!(
            m.kind,
            AccessorKind::AttributeReader | AccessorKind::AttributeWriter
        ) {
            // Represented by struct fields; no accessor function.
            continue;
        }
        // `initialize` → a `new/n` constructor returning a struct literal
        // built from the `@field = value` assignments in its body.
        if m.name.as_str() == "initialize" {
            emit_constructor(&mut out, m, v2_name);
            continue;
        }
        let thread_record =
            !is_module_singleton && matches!(m.receiver, MethodReceiver::Instance);
        emit_fn(&mut out, m, thread_record);
    }

    Ok(out)
}

/// Emit `initialize` as a `new/n` constructor. A flat body (only
/// `@field = value` assigns) emits a clean struct literal `%V2.Name{f:
/// v, …}`. A richer body (conditionals, early returns, locals — e.g.
/// flash's cross-request population) seeds `record = %V2.Name{}` and
/// runs the body threaded through `record` (via
/// `mutation_to_struct_return::thread_constructor_body`).
fn emit_constructor(out: &mut String, m: &MethodDef, v2_name: &str) {
    let params = m.params.iter().map(param_decl).collect::<Vec<_>>().join(", ");

    writeln!(out, "  def new({params}) do").unwrap();
    match flat_field_assigns(&m.body) {
        Some(pairs) if pairs.is_empty() => {
            writeln!(out, "    %{v2_name}{{}}").unwrap();
        }
        Some(pairs) => {
            writeln!(out, "    %{v2_name}{{{}}}", pairs.join(", ")).unwrap();
        }
        None => {
            // Non-flat: seed the struct, then run the threaded body.
            let threaded = crate::lower::functionalize::mutation_to_struct_return::thread_constructor_body(
                &m.body,
            );
            writeln!(out, "    record = %{v2_name}{{}}").unwrap();
            out.push_str(&expr::indent(&expr::emit_method_body(&threaded), 2));
            out.push('\n');
        }
    }
    out.push_str("  end\n");
}

/// `Some(pairs)` when every top-level statement is a `@field = value`
/// assign (a flat constructor → struct literal); `None` otherwise.
fn flat_field_assigns(body: &Expr) -> Option<Vec<String>> {
    let stmts: &[Expr] = match &*body.node {
        ExprNode::Seq { exprs } => exprs,
        _ => std::slice::from_ref(body),
    };
    let mut pairs = Vec::new();
    for s in stmts {
        match &*s.node {
            ExprNode::Assign { target: LValue::Ivar { name }, value } => {
                pairs.push(format!("{name}: {}", expr::emit_expr(value)))
            }
            _ => return None,
        }
    }
    Some(pairs)
}

/// Render one param, applying Elixir default-arg syntax (`name \\ default`).
fn param_decl(p: &crate::dialect::Param) -> String {
    match &p.default {
        Some(d) => format!("{} \\\\ {}", p.as_str(), expr::emit_expr(d)),
        None => p.as_str().to_string(),
    }
}

/// Emit a flat list of `MethodDef`s (Ruby `Mode::Module`) as Elixir
/// functions. Required by the `TargetEmit` contract; not exercised by
/// the current runtime slice.
pub fn emit_module(methods: &[MethodDef]) -> Result<String, String> {
    let mut out = String::new();
    for m in methods {
        emit_fn(&mut out, m, false);
    }
    Ok(out)
}

/// Render a module-level constant as an Elixir module attribute, e.g.
/// `ESCAPES = {…}.freeze` → `  @escapes %{…}`. Indented one level to
/// sit inside the `defmodule` the namespace wrapper supplies.
pub fn format_constant(name: &str, value: &Expr) -> String {
    format!("  @{} {}", name.to_lowercase(), expr::emit_const_value(value))
}

/// The struct's `defstruct` fields: attr-declared names plus every
/// `@ivar` the method bodies reference (read or written). Covers structs
/// whose state is a bare ivar with no accessor (session's `@data`).
fn struct_fields(class: &LibraryClass) -> Vec<String> {
    let mut out = collect_struct_fields(&class.methods);
    for m in &class.methods {
        collect_ivar_names(&m.body, &mut out);
    }
    out
}

/// Collect struct field names from the mutation-threading bridges in a
/// (post-functionalize) body: `record.__field__(:x)` reads and
/// `record.__struct_put__(:x, …)` writes carry the field as a Sym arg.
fn collect_ivar_names(e: &Expr, out: &mut Vec<String>) {
    match &*e.node {
        ExprNode::Send { method, args, recv, block, .. } => {
            let m = method.as_str();
            if (m == "__field__" || m == "__struct_put__") && !args.is_empty() {
                if let ExprNode::Lit { value: Literal::Sym { value } } = &*args[0].node {
                    let n = value.to_string();
                    if !out.contains(&n) {
                        out.push(n);
                    }
                }
            }
            if let Some(r) = recv {
                collect_ivar_names(r, out);
            }
            args.iter().for_each(|a| collect_ivar_names(a, out));
            if let Some(b) = block {
                collect_ivar_names(b, out);
            }
        }
        ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
            exprs.iter().for_each(|x| collect_ivar_names(x, out))
        }
        ExprNode::Assign { value, .. }
        | ExprNode::OpAssign { value, .. }
        | ExprNode::Return { value }
        | ExprNode::Raise { value }
        | ExprNode::Cast { value, .. } => collect_ivar_names(value, out),
        ExprNode::If { cond, then_branch, else_branch } => {
            collect_ivar_names(cond, out);
            collect_ivar_names(then_branch, out);
            collect_ivar_names(else_branch, out);
        }
        ExprNode::While { cond, body, .. } => {
            collect_ivar_names(cond, out);
            collect_ivar_names(body, out);
        }
        ExprNode::BoolOp { left, right, .. } => {
            collect_ivar_names(left, out);
            collect_ivar_names(right, out);
        }
        ExprNode::Lambda { body, .. } => collect_ivar_names(body, out),
        ExprNode::Yield { args } => args.iter().for_each(|a| collect_ivar_names(a, out)),
        ExprNode::Hash { entries, .. } => entries.iter().for_each(|(k, v)| {
            collect_ivar_names(k, out);
            collect_ivar_names(v, out);
        }),
        _ => {}
    }
}

/// Collect unique `defstruct` field names from attr accessor methods.
fn collect_struct_fields(methods: &[MethodDef]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in methods {
        if !matches!(
            m.kind,
            AccessorKind::AttributeReader | AccessorKind::AttributeWriter
        ) {
            continue;
        }
        let field = m.name.as_str().trim_end_matches('=').to_string();
        if !out.contains(&field) {
            out.push(field);
        }
    }
    out
}

/// Emit one method as an Elixir `def` (indented one level for inside a
/// `defmodule`; the body is indented a further level). An instance
/// method threads a leading `record` param — but only when its
/// (mutation-lowered) body actually references `record`; pure instance
/// methods (no `@ivar` use) take no record param so bareword self-calls
/// stay arity-correct.
fn emit_fn(out: &mut String, m: &MethodDef, instance_method: bool) {
    let body = expr::emit_method_body(&m.body);

    let mut params: Vec<String> = Vec::new();
    if instance_method && references_token(&body, "record") {
        params.push("record".to_string());
    }
    params.extend(m.params.iter().map(param_decl));
    // A body that `yield`s calls the block through a trailing `block_fn`.
    if references_token(&body, "block_fn") {
        params.push("block_fn".to_string());
    }

    writeln!(
        out,
        "  def {}({}) do",
        elixir_fn_name(m.name.as_str()),
        params.join(", ")
    )
    .unwrap();
    out.push_str(&expr::indent(&body, 2));
    out.push('\n');
    out.push_str("  end\n");
}

/// Whether a rendered body references `tok` as an identifier token
/// (used to detect emitter-introduced params: `record` from mutation-
/// threading, `block_fn` from `yield`).
pub(super) fn references_token(body: &str, tok: &str) -> bool {
    body.split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|t| t == tok)
}

/// Map a Ruby method name to a legal Elixir function name. `?`/`!`
/// suffixes are valid in Elixir and pass through. The indexing
/// operators `[]`/`[]=` (illegal as Elixir function names) become
/// `get`/`put`; a writer `foo=` becomes `set_foo`.
pub(super) fn elixir_fn_name(name: &str) -> String {
    match name {
        "[]" => return "get".to_string(),
        "[]=" => return "put".to_string(),
        _ => {}
    }
    if let Some(base) = name.strip_suffix('=') {
        format!("set_{base}")
    } else {
        name.to_string()
    }
}

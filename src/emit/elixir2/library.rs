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
use crate::expr::{Expr, ExprNode, LValue};

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
        // Struct payload for normal classes (dormant until a struct
        // class enters ELIXIR_RUNTIME; module-singletons skip it).
        let fields = collect_struct_fields(&class.methods);
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

/// Emit `def new(params) do %V2.Name{field: value, …} end` from an
/// `initialize` whose body is a sequence of `@field = value` assigns.
/// (Constructors with richer bodies — e.g. loops — aren't covered yet;
/// only the `@ivar =` assignments contribute fields.)
fn emit_constructor(out: &mut String, m: &MethodDef, v2_name: &str) {
    let params: Vec<String> = m.params.iter().map(|p| p.as_str().to_string()).collect();
    let stmts: Vec<&Expr> = match &*m.body.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![&m.body],
    };
    let pairs: Vec<String> = stmts
        .iter()
        .filter_map(|s| match &*s.node {
            ExprNode::Assign { target: LValue::Ivar { name }, value } => {
                Some(format!("{name}: {}", expr::emit_expr(value)))
            }
            _ => None,
        })
        .collect();

    writeln!(out, "  def new({}) do", params.join(", ")).unwrap();
    if pairs.is_empty() {
        writeln!(out, "    %{v2_name}{{}}").unwrap();
    } else {
        writeln!(out, "    %{v2_name}{{{}}}", pairs.join(", ")).unwrap();
    }
    out.push_str("  end\n");
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
    params.extend(m.params.iter().map(|p| p.as_str().to_string()));
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

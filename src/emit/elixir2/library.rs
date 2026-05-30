//! Generic `LibraryClass` → Elixir emit (Phase 1 scaffold).
//!
//! Mirrors `src/emit/go2/library.rs` in shape, mapped to Elixir's
//! functional/immutable model:
//!
//! - A Ruby `class`/`module` becomes a `defmodule`.
//! - A module-singleton (a `module` whose methods are all
//!   `self.`-receivers — e.g. `Inflector`) becomes a plain module of
//!   functions; Ruby `def self.foo` → Elixir `def foo`.
//! - A normal class becomes a `defmodule` with a `defstruct` payload;
//!   instance methods thread the record as the first param
//!   (`def foo(record, …)`), class methods stay bare (`def foo(…)`).
//! - Inheritance (`parent`) is ignored — the lowerer linearizes method
//!   overrides onto each class (same as rust2).
//! - `mutates_self` is ignored at Phase 1; the immutable mutation-
//!   threading work lands later, driven by the error inventory.
//!
//! Phase 1 emits stub bodies (see `expr::emit_body`); `ELIXIR_RUNTIME`
//! currently exercises only the module-singleton path (`Inflector`).

use std::fmt::Write;

use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver};

use super::expr;

/// Emit one `LibraryClass` as an Elixir `defmodule` (trailing newline
/// included). The enclosing `V2` namespace wrapper is added by
/// `runtime_loader::elixir_wrap_namespace`, not here.
pub fn emit_library_class(class: &LibraryClass) -> Result<String, String> {
    let is_module_singleton = class.is_module
        && !class.methods.is_empty()
        && class
            .methods
            .iter()
            .all(|m| matches!(m.receiver, MethodReceiver::Class));
    if is_module_singleton {
        emit_module_singleton(class)
    } else {
        emit_struct_class(class)
    }
}

/// Emit a flat list of `MethodDef`s (Ruby `Mode::Module`) as Elixir
/// functions. Elixir has no free functions, so these only make sense
/// once wrapped in a `defmodule` (the namespace wrapper does that).
/// Not exercised by the Phase 1 runtime slice, but required by the
/// `TargetEmit` contract.
pub fn emit_module(methods: &[MethodDef]) -> Result<String, String> {
    let mut out = String::new();
    for m in methods {
        emit_fn(&mut out, m, false);
    }
    Ok(out)
}

/// `ClassId` → Elixir module name. `ActiveRecord::Base` →
/// `ActiveRecord.Base` (Elixir's dotted nested-module form); a bare
/// `Inflector` passes through. Each `::`-segment is already CamelCase,
/// which is exactly what Elixir requires of an alias.
fn module_name(class: &LibraryClass) -> String {
    class.name.0.as_str().replace("::", ".")
}

fn emit_module_singleton(class: &LibraryClass) -> Result<String, String> {
    let name = module_name(class);
    let mut out = String::new();
    writeln!(out, "defmodule {name} do").unwrap();
    for m in &class.methods {
        // Module-singleton methods are all class-receivers; in Elixir a
        // module's functions ARE its "class methods", so no receiver
        // threading.
        emit_fn(&mut out, m, false);
    }
    out.push_str("end\n");
    Ok(out)
}

fn emit_struct_class(class: &LibraryClass) -> Result<String, String> {
    let name = module_name(class);
    let mut out = String::new();
    writeln!(out, "defmodule {name} do").unwrap();

    // `defstruct` payload — one field per unique attribute name
    // synthesized from attr_reader/attr_writer MethodDefs. Initialize-
    // only ivars (assigned with no reader/writer) aren't reflected yet;
    // they'd surface at use sites, which is acceptable inventory.
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

    for m in &class.methods {
        // attr_reader/attr_writer are represented by the struct fields;
        // don't also emit accessor functions.
        if matches!(
            m.kind,
            AccessorKind::AttributeReader | AccessorKind::AttributeWriter
        ) {
            continue;
        }
        let thread_record = matches!(m.receiver, MethodReceiver::Instance);
        emit_fn(&mut out, m, thread_record);
    }

    out.push_str("end\n");
    Ok(out)
}

/// Collect unique `defstruct` field names from attr accessor methods,
/// in first-seen order. A writer `foo=` contributes `foo`.
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

/// Emit one method as an Elixir `def` (2-space indented, for inside a
/// `defmodule`). When `thread_record` is set, an instance receiver is
/// threaded as a leading `record` param. Phase 1 stub bodies don't use
/// their params, so every param is `_`-prefixed to stay clean under
/// `mix compile --warnings-as-errors`.
fn emit_fn(out: &mut String, m: &MethodDef, thread_record: bool) {
    let mut params: Vec<String> = Vec::new();
    if thread_record {
        params.push("_record".to_string());
    }
    params.extend(m.params.iter().map(|p| stub_param(p.as_str())));

    writeln!(
        out,
        "  def {}({}) do",
        elixir_fn_name(m.name.as_str()),
        params.join(", ")
    )
    .unwrap();
    writeln!(out, "    {}", expr::emit_body(m)).unwrap();
    out.push_str("  end\n");
}

/// Phase 1 param rendering: `_`-prefix so unused stub params don't trip
/// `--warnings-as-errors`. Phase 2's real walker emits bare names.
fn stub_param(name: &str) -> String {
    format!("_{name}")
}

/// Map a Ruby method name to a legal Elixir function name. `?`/`!`
/// suffixes are valid in Elixir and pass through; a writer `foo=`
/// (illegal — `=` can't appear in a function name) becomes `set_foo`.
fn elixir_fn_name(name: &str) -> String {
    if let Some(base) = name.strip_suffix('=') {
        format!("set_{base}")
    } else {
        name.to_string()
    }
}

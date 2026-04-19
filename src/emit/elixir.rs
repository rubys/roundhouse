//! Elixir emitter.
//!
//! Second Phase 2 scaffold. Elixir is the target that most aggressively
//! stress-tests IR target-neutrality because its paradigm is
//! fundamentally different from every other target on the list:
//!
//! - No classes — models are modules with a `defstruct` payload.
//! - No method dispatch — what Ruby calls `article.title` becomes
//!   module-function-on-record: `Article.title(article)` (or struct
//!   field access, `article.title`, for direct attributes).
//! - No mutation — ivar rebinds become variable rebinds (no `self.foo =`
//!   state threading at scaffold depth; real Elixir code returns the
//!   updated struct).
//! - No inheritance — the `parent` field on Model/Controller is noted
//!   but not emitted. Rails' `ApplicationRecord` / `ApplicationController`
//!   become `use Railcar.Record` / `use Railcar.Controller`-style
//!   conventions in a real runtime; the scaffold doesn't commit yet.
//! - Pattern matching as control flow — `if expr.save, do: …, else: …`
//!   is idiomatic as a `case` on `{:ok, _} / {:error, _}`. Scaffold
//!   emits `if/else` for now; Phase 3 runtime work converts.
//!
//! Non-goals:
//! - `@spec` type annotations (Elixir is dynamically typed; `Ty` info
//!   is useful for the Rust/Go/TS targets, not here).
//! - Phoenix / Plug integration.
//! - Live View / template emission.
//! - Controllers that return `{:cont, conn}` tuples (real Plug shape).

use std::fmt::Write;
use std::path::PathBuf;

use super::EmittedFile;
use crate::App;
use crate::ident::Symbol;
use crate::dialect::{
    Action, Controller, MethodDef, Model, RouteSpec, Test, TestModule,
};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::naming::snake_case;

const RUNTIME_SOURCE: &str = include_str!("../../runtime/elixir/runtime.ex");
const DB_SOURCE: &str = include_str!("../../runtime/elixir/db.ex");
/// Elixir HTTP runtime — Phase 4d pass-2 shape. Copied verbatim
/// into generated projects as `lib/roundhouse/http.ex` when any
/// controller emits. Exposes ActionResponse/ActionContext structs
/// + Router.match table; the emitter's action templates return
/// ActionResponse directly (no class-based dispatch).
const HTTP_SOURCE: &str = include_str!("../../runtime/elixir/http.ex");
/// Pass-2 test-support runtime. TestClient + TestResponse with
/// Rails-shaped assertions. Ships as
/// `lib/roundhouse/test_support.ex`.
const TEST_SUPPORT_SOURCE: &str =
    include_str!("../../runtime/elixir/test_support.ex");
/// View helpers — link_to, button_to, FormBuilder, etc. Ships as
/// `lib/roundhouse/view_helpers.ex` when views emit.
const VIEW_HELPERS_SOURCE: &str =
    include_str!("../../runtime/elixir/view_helpers.ex");

pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();
    files.push(emit_mix_exs());
    if !app.models.is_empty() {
        files.push(EmittedFile {
            path: PathBuf::from("lib/roundhouse.ex"),
            content: RUNTIME_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("lib/roundhouse/db.ex"),
            content: DB_SOURCE.to_string(),
        });
        files.push(emit_schema_sql_ex(app));
    }
    for model in &app.models {
        files.push(emit_model_file(model, app));
    }
    if !app.controllers.is_empty() {
        // HTTP runtime (ActionResponse/ActionContext + Router) —
        // copied verbatim.
        files.push(EmittedFile {
            path: PathBuf::from("lib/roundhouse/http.ex"),
            content: HTTP_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("lib/roundhouse/test_support.ex"),
            content: TEST_SUPPORT_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("lib/roundhouse/view_helpers.ex"),
            content: VIEW_HELPERS_SOURCE.to_string(),
        });
        let known_models: Vec<Symbol> =
            app.models.iter().map(|m| m.name.0.clone()).collect();
        for controller in &app.controllers {
            files.push(emit_controller_file_pass2(controller, &known_models, app));
        }
        files.push(emit_ex_route_helpers(app));
        files.push(emit_ex_views(app));
    }
    if !app.routes.entries.is_empty() {
        files.push(emit_router_file(app));
        files.push(emit_ex_routes_register(app));
    }
    if !app.fixtures.is_empty() {
        let lowered = crate::lower::lower_fixtures(app);
        files.push(emit_ex_fixtures_helper(&lowered));
        for f in &lowered.fixtures {
            files.push(emit_ex_fixture(f));
        }
    }
    if !app.test_modules.is_empty() {
        files.push(EmittedFile {
            path: PathBuf::from("test/test_helper.exs"),
            content: "ExUnit.start()\n".to_string(),
        });
        for tm in &app.test_modules {
            files.push(emit_ex_test(tm, app));
        }
    }
    files
}

/// Minimal mix.exs. `elixirc_paths` uses a wildcard filter that
/// excludes controllers and the router — their bodies reference
/// runtime that doesn't exist yet (redirect_to, Post.all, etc.), so
/// including them blocks `mix compile`. When Phase 3 wires the
/// runtime, the filter relaxes.
fn emit_mix_exs() -> EmittedFile {
    let content = "\
defmodule App.MixProject do
  use Mix.Project

  def project do
    [
      app: :app,
      version: \"0.1.0\",
      elixir: \"~> 1.18\",
      elixirc_paths: elixirc_paths(Mix.env()),
      start_permanent: Mix.env() == :prod,
      deps: deps()
    ]
  end

  def application do
    [extra_applications: [:logger]]
  end

  defp deps do
    [
      {:exqlite, \"~> 0.30\"}
    ]
  end

  # Phase 4c: controllers now lower through Roundhouse.Http stubs and
  # compile. Test env additionally includes test/support/ so fixtures
  # are compiled alongside the app.
  defp elixirc_paths(:test) do
    Path.wildcard(\"lib/**/*.ex\") ++ Path.wildcard(\"test/support/**/*.ex\")
  end

  defp elixirc_paths(_) do
    Path.wildcard(\"lib/**/*.ex\")
  end
end
";
    EmittedFile {
        path: PathBuf::from("mix.exs"),
        content: content.to_string(),
    }
}

/// `lib/roundhouse/schema_sql.ex` — Elixir module exposing the
/// target-neutral DDL produced by `lower::lower_schema` as a
/// `create_tables/0` function (Elixir module attributes can't hold
/// arbitrary strings at compile time in all versions; a function is
/// uniform).
fn emit_schema_sql_ex(app: &App) -> EmittedFile {
    let ddl = crate::lower::lower_schema(&app.schema);
    // Escape the DDL for a single-quoted Elixir string: backslashes,
    // quotes, newlines. Heredoc would be cleaner but needs matching
    // indentation, which collides with the DDL's column-0 layout.
    let escaped = ddl
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "defmodule Roundhouse.SchemaSQL do").unwrap();
    writeln!(s, "  @moduledoc false").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "  def create_tables do").unwrap();
    writeln!(s, "    \"{escaped}\"").unwrap();
    writeln!(s, "  end").unwrap();
    writeln!(s, "end").unwrap();
    EmittedFile {
        path: PathBuf::from("lib/roundhouse/schema_sql.ex"),
        content: s,
    }
}

// Models ---------------------------------------------------------------

fn emit_model_file(model: &Model, app: &App) -> EmittedFile {
    let module = model.name.0.as_str();
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "defmodule {module} do").unwrap();

    // Struct declaration with typed defaults so NOT NULL columns
    // get concrete values (empty strings for text, 0 for ints)
    // rather than nil. SQLite rejects NULL → NOT NULL at INSERT
    // time, and the fixture harness calls save before every
    // non-id field is explicitly set.
    let fields: Vec<String> = model
        .attributes
        .fields
        .iter()
        .map(|(k, ty)| format!("{}: {}", k.as_str(), ex_default_for(ty)))
        .collect();
    if !fields.is_empty() {
        writeln!(s, "  defstruct [{}]", fields.join(", ")).unwrap();
    } else {
        writeln!(s, "  defstruct []").unwrap();
    }

    let attrs: Vec<Symbol> = model.attributes.fields.keys().cloned().collect();
    for method in model.methods() {
        writeln!(s).unwrap();
        emit_model_method_with_attrs(&mut s, module, method, &attrs);
    }

    let lowered = crate::lower::lower_validations(model);
    if !lowered.is_empty() {
        writeln!(s).unwrap();
        emit_validate_method_ex(&mut s, &lowered);
    }
    // Skip persistence for abstract base classes (no columns beyond
    // `id`) — ApplicationRecord etc.
    let has_table = model
        .attributes
        .fields
        .keys()
        .any(|k| k.as_str() != "id");
    if has_table {
        writeln!(s).unwrap();
        emit_persistence_methods_ex(&mut s, module, model, !lowered.is_empty(), app);
    }

    writeln!(s, "end").unwrap();
    let fname = format!("lib/{}.ex", snake_case(module));
    EmittedFile { path: PathBuf::from(fname), content: s }
}

/// Render save/destroy/count/find for a module against the test DB.
/// Naming: `save/1` and `destroy/1` take the record; `count/0` and
/// `find/1` are module functions. Matches Elixir's functional shape
/// (no implicit receiver) and what our spec emit expects.
fn emit_persistence_methods_ex(
    out: &mut String,
    module: &str,
    model: &Model,
    has_validate: bool,
    app: &App,
) {
    let lp = crate::lower::lower_persistence(model, app);
    let recv = module_receiver_name(module);

    let insert_sql = positional_placeholders_ex(&lp.insert_sql);
    let update_sql = positional_placeholders_ex(&lp.update_sql);
    let delete_sql = positional_placeholders_ex(&lp.delete_sql);
    let select_by_id_sql = positional_placeholders_ex(&lp.select_by_id_sql);

    let non_id_args: Vec<String> = lp
        .non_id_columns
        .iter()
        .map(|s| format!("{recv}.{}", s.as_str()))
        .collect();

    // ----- save/1 -----
    writeln!(out, "  def save({recv}) do").unwrap();
    if has_validate {
        writeln!(out, "    cond do").unwrap();
        writeln!(out, "      validate({recv}) != [] ->").unwrap();
        writeln!(out, "        false").unwrap();
        // belongs_to existence checks, chained into the cond.
        for check in &lp.belongs_to_checks {
            let fk = check.foreign_key.as_str();
            let target = check.target_class.0.as_str();
            writeln!(
                out,
                "      {recv}.{fk} == nil or {recv}.{fk} == 0 or {target}.find({recv}.{fk}) == nil ->",
            )
            .unwrap();
            writeln!(out, "        false").unwrap();
        }
        writeln!(out, "      true ->").unwrap();
        writeln!(out, "        do_save({recv})").unwrap();
        writeln!(out, "    end").unwrap();
        writeln!(out, "  end").unwrap();
    } else {
        writeln!(out, "    do_save({recv})").unwrap();
        writeln!(out, "  end").unwrap();
    }

    writeln!(out).unwrap();
    writeln!(out, "  defp do_save({recv}) do").unwrap();
    writeln!(out, "    if {recv}.id == nil or {recv}.id == 0 do").unwrap();
    writeln!(
        out,
        "      _id = Roundhouse.Db.execute({insert_sql:?}, [{}])",
        non_id_args.join(", "),
    )
    .unwrap();
    writeln!(out, "    else").unwrap();
    writeln!(
        out,
        "      _id = Roundhouse.Db.execute({update_sql:?}, [{}, {recv}.id])",
        non_id_args.join(", "),
    )
    .unwrap();
    writeln!(out, "    end").unwrap();
    writeln!(out, "    true").unwrap();
    writeln!(out, "  end").unwrap();

    // ----- destroy/1 -----
    writeln!(out).unwrap();
    writeln!(out, "  def destroy({recv}) do").unwrap();
    for dc in &lp.dependent_children {
        let child_class = dc.child_class.0.as_str();
        let child_select = positional_placeholders_ex(&dc.select_by_parent_sql);
        writeln!(
            out,
            "    rows = Roundhouse.Db.query_all({child_select:?}, [{recv}.id])",
        )
        .unwrap();
        writeln!(out, "    Enum.each(rows, fn row ->").unwrap();
        writeln!(out, "      [{}] = row", dc
            .child_columns
            .iter()
            .map(|c| c.as_str().to_string())
            .collect::<Vec<_>>()
            .join(", "))
            .unwrap();
        let fields: Vec<String> = dc
            .child_columns
            .iter()
            .map(|c| format!("{0}: {0}", c.as_str()))
            .collect();
        writeln!(
            out,
            "      child = %{child_class}{{{}}}",
            fields.join(", ")
        )
        .unwrap();
        writeln!(out, "      {child_class}.destroy(child)").unwrap();
        writeln!(out, "    end)").unwrap();
    }
    writeln!(
        out,
        "    _ = Roundhouse.Db.execute({delete_sql:?}, [{recv}.id])",
    )
    .unwrap();
    writeln!(out, "    :ok").unwrap();
    writeln!(out, "  end").unwrap();

    // ----- count/0 -----
    writeln!(out).unwrap();
    writeln!(out, "  def count do").unwrap();
    writeln!(
        out,
        "    Roundhouse.Db.scalar({:?}, [])",
        lp.count_sql,
    )
    .unwrap();
    writeln!(out, "  end").unwrap();

    // ----- find/1 -----
    writeln!(out).unwrap();
    writeln!(out, "  def find(id) do").unwrap();
    writeln!(
        out,
        "    case Roundhouse.Db.query_one({select_by_id_sql:?}, [id]) do",
    )
    .unwrap();
    let field_list: Vec<String> = lp
        .columns
        .iter()
        .map(|c| c.as_str().to_string())
        .collect();
    writeln!(out, "      [{}] ->", field_list.join(", ")).unwrap();
    let struct_fields: Vec<String> = lp
        .columns
        .iter()
        .map(|c| format!("{0}: {0}", c.as_str()))
        .collect();
    writeln!(
        out,
        "        %{module}{{{}}}",
        struct_fields.join(", ")
    )
    .unwrap();
    writeln!(out, "      nil ->").unwrap();
    writeln!(out, "        nil").unwrap();
    writeln!(out, "    end").unwrap();
    writeln!(out, "  end").unwrap();

    // ----- all/0 -----
    let select_all_sql = positional_placeholders_ex(&lp.select_all_sql);
    writeln!(out).unwrap();
    writeln!(out, "  def all do").unwrap();
    writeln!(
        out,
        "    rows = Roundhouse.Db.query_all({select_all_sql:?}, [])"
    )
    .unwrap();
    writeln!(out, "    Enum.map(rows, fn row ->").unwrap();
    writeln!(
        out,
        "      [{}] = row",
        lp.columns
            .iter()
            .map(|c| c.as_str().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
    .unwrap();
    writeln!(
        out,
        "      %{module}{{{}}}",
        struct_fields.join(", ")
    )
    .unwrap();
    writeln!(out, "    end)").unwrap();
    writeln!(out, "  end").unwrap();

    // ----- last/0 -----
    let select_last_sql = positional_placeholders_ex(&lp.select_last_sql);
    writeln!(out).unwrap();
    writeln!(out, "  def last do").unwrap();
    writeln!(
        out,
        "    case Roundhouse.Db.query_one({select_last_sql:?}, []) do"
    )
    .unwrap();
    writeln!(out, "      [{}] ->", field_list.join(", ")).unwrap();
    writeln!(
        out,
        "        %{module}{{{}}}",
        struct_fields.join(", ")
    )
    .unwrap();
    writeln!(out, "      nil ->").unwrap();
    writeln!(out, "        nil").unwrap();
    writeln!(out, "    end").unwrap();
    writeln!(out, "  end").unwrap();

    // ----- reload/1 — reads the row fresh, returns updated struct -----
    writeln!(out).unwrap();
    writeln!(out, "  def reload({recv}) do").unwrap();
    writeln!(out, "    case find({recv}.id) do").unwrap();
    writeln!(out, "      nil -> {recv}").unwrap();
    writeln!(out, "      fresh -> fresh").unwrap();
    writeln!(out, "    end").unwrap();
    writeln!(out, "  end").unwrap();

    // ----- has_many association accessors -----
    for dc in &lp.dependent_children {
        let child_class = dc.child_class.0.as_str();
        let assoc = crate::naming::pluralize_snake(child_class);
        let child_select = positional_placeholders_ex(&dc.select_by_parent_sql);
        let child_cols: Vec<String> = dc
            .child_columns
            .iter()
            .map(|c| c.as_str().to_string())
            .collect();
        let child_struct_fields: Vec<String> = dc
            .child_columns
            .iter()
            .map(|c| format!("{0}: {0}", c.as_str()))
            .collect();
        writeln!(out).unwrap();
        writeln!(out, "  def {assoc}({recv}) do").unwrap();
        writeln!(
            out,
            "    rows = Roundhouse.Db.query_all({child_select:?}, [{recv}.id])"
        )
        .unwrap();
        writeln!(out, "    Enum.map(rows, fn row ->").unwrap();
        writeln!(out, "      [{}] = row", child_cols.join(", ")).unwrap();
        writeln!(
            out,
            "      %{child_class}{{{}}}",
            child_struct_fields.join(", ")
        )
        .unwrap();
        writeln!(out, "    end)").unwrap();
        writeln!(out, "  end").unwrap();
    }
}

/// Elixir defstruct default for a field typed by the schema. Mirrors
/// TS/Crystal: `""` for strings/text/time, `0` for ints, `0.0` for
/// floats, `false` for bools, `nil` otherwise. The id column still
/// starts at nil — save distinguishes "unsaved" from "saved" by
/// checking nil-or-0 so both work.
fn ex_default_for(ty: &crate::ty::Ty) -> &'static str {
    use crate::ty::Ty;
    match ty {
        Ty::Int => "nil",
        Ty::Float => "0.0",
        Ty::Bool => "false",
        Ty::Str | Ty::Sym => "\"\"",
        Ty::Class { id, .. } if id.0.as_str() == "Time" => "\"\"",
        _ => "nil",
    }
}

/// SQLite `?N` → exqlite/sqlite positional `?`. Same workaround as
/// Crystal/Go/TS — driver-level quirk, absorbed at emit time.
fn positional_placeholders_ex(sql: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = sql.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '?' {
            out.push('?');
            i += 1;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn emit_validate_method_ex(
    out: &mut String,
    validations: &[crate::lower::LoweredValidation],
) {
    writeln!(out, "  def validate(record) do").unwrap();
    writeln!(out, "    errors = []").unwrap();
    for lv in validations {
        for check in &lv.checks {
            emit_check_inline_ex(out, lv.attribute.as_str(), check);
        }
    }
    writeln!(out, "    errors").unwrap();
    writeln!(out, "  end").unwrap();
}

fn emit_check_inline_ex(out: &mut String, attr: &str, check: &crate::lower::Check) {
    use crate::lower::Check;
    let msg = check.default_message();
    let push = |cond: &str| -> String {
        format!(
            "    errors =\n      if {cond} do\n        [Roundhouse.ValidationError.new({attr:?}, {msg:?}) | errors]\n      else\n        errors\n      end"
        )
    };
    let block = match check {
        Check::Presence => push(&format!("record.{attr} == \"\" or record.{attr} == nil")),
        Check::Absence => push(&format!("record.{attr} != \"\" and record.{attr} != nil")),
        Check::MinLength { n } => {
            push(&format!("record.{attr} == nil or String.length(record.{attr}) < {n}"))
        }
        Check::MaxLength { n } => {
            push(&format!("record.{attr} != nil and String.length(record.{attr}) > {n}"))
        }
        Check::GreaterThan { threshold } => {
            push(&format!("record.{attr} <= {threshold}"))
        }
        Check::LessThan { threshold } => push(&format!("record.{attr} >= {threshold}")),
        Check::OnlyInteger => {
            format!("    # OnlyInteger on {attr:?} — no-op, Elixir has no implicit coercion")
        }
        Check::Inclusion { values } => {
            let parts: Vec<String> = values.iter().map(inclusion_value_to_ex).collect();
            push(&format!("record.{attr} not in [{}]", parts.join(", ")))
        }
        Check::Format { pattern } => {
            format!("    # TODO: Format check on {attr:?} requires Regex ({pattern:?})")
        }
        Check::Uniqueness { .. } => {
            format!("    # TODO: Uniqueness on {attr:?} requires DB access at runtime")
        }
        Check::Custom { method } => {
            format!("    errors = {method}(record, errors)", method = method.as_str())
        }
    };
    writeln!(out, "{block}").unwrap();
}

fn inclusion_value_to_ex(v: &crate::lower::InclusionValue) -> String {
    use crate::lower::InclusionValue;
    match v {
        InclusionValue::Str { value } => format!("{value:?}"),
        InclusionValue::Int { value } => value.to_string(),
        InclusionValue::Float { value } => {
            let s = value.to_string();
            if s.contains('.') { s } else { format!("{s}.0") }
        }
        InclusionValue::Bool { value } => value.to_string(),
    }
}

fn emit_model_method_with_attrs(
    out: &mut String,
    module: &str,
    m: &MethodDef,
    attrs: &[Symbol],
) {
    let name = m.name.as_str();
    // Instance methods take the record as first arg (`post` for
    // module Post). Class methods take only their declared params.
    let is_instance = matches!(m.receiver, crate::dialect::MethodReceiver::Instance);
    let first_arg = if is_instance { Some(module_receiver_name(module)) } else { None };

    let mut params: Vec<String> = Vec::new();
    if let Some(arg) = &first_arg {
        params.push(arg.clone());
    }
    for p in &m.params {
        params.push(p.to_string());
    }
    let param_list = if params.is_empty() {
        String::new()
    } else {
        format!("({})", params.join(", "))
    };

    writeln!(out, "  def {name}{param_list} do").unwrap();
    // Pre-emit rewrite: bare-name Sends matching a model attribute
    // become Ivar reads, which `emit_expr` already renders as
    // `post.<field>` when receiver_arg is set. Same shape as the
    // TS/Go pre-emit passes.
    let body_expr = if first_arg.is_some() {
        rewrite_bare_attrs_to_ivars_ex(&m.body, attrs)
    } else {
        m.body.clone()
    };
    let body = emit_block(&body_expr, first_arg.as_deref());
    for line in body.lines() {
        writeln!(out, "    {line}").unwrap();
    }
    writeln!(out, "  end").unwrap();
}

fn rewrite_bare_attrs_to_ivars_ex(e: &Expr, attrs: &[Symbol]) -> Expr {
    use crate::expr::{Arm, InterpPart, Pattern};
    let rewrite = |child: &Expr| rewrite_bare_attrs_to_ivars_ex(child, attrs);
    let new_node = match &*e.node {
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if args.is_empty() && attrs.iter().any(|s| s == method) =>
        {
            ExprNode::Ivar { name: method.clone() }
        }
        ExprNode::Send { recv, method, args, block, parenthesized } => ExprNode::Send {
            recv: recv.as_ref().map(&rewrite),
            method: method.clone(),
            args: args.iter().map(&rewrite).collect(),
            block: block.as_ref().map(&rewrite),
            parenthesized: *parenthesized,
        },
        ExprNode::Seq { exprs } => ExprNode::Seq {
            exprs: exprs.iter().map(&rewrite).collect(),
        },
        ExprNode::Array { elements, style } => ExprNode::Array {
            elements: elements.iter().map(&rewrite).collect(),
            style: *style,
        },
        ExprNode::Hash { entries, braced } => ExprNode::Hash {
            entries: entries.iter().map(|(k, v)| (rewrite(k), rewrite(v))).collect(),
            braced: *braced,
        },
        ExprNode::If { cond, then_branch, else_branch } => ExprNode::If {
            cond: rewrite(cond),
            then_branch: rewrite(then_branch),
            else_branch: rewrite(else_branch),
        },
        ExprNode::Case { scrutinee, arms } => ExprNode::Case {
            scrutinee: rewrite(scrutinee),
            arms: arms
                .iter()
                .map(|arm| Arm {
                    pattern: arm.pattern.clone(),
                    guard: arm.guard.as_ref().map(&rewrite),
                    body: rewrite(&arm.body),
                })
                .collect(),
        },
        ExprNode::BoolOp { op, surface, left, right } => ExprNode::BoolOp {
            op: *op,
            surface: *surface,
            left: rewrite(left),
            right: rewrite(right),
        },
        ExprNode::StringInterp { parts } => ExprNode::StringInterp {
            parts: parts
                .iter()
                .map(|p| match p {
                    InterpPart::Text { value } => InterpPart::Text { value: value.clone() },
                    InterpPart::Expr { expr } => InterpPart::Expr { expr: rewrite(expr) },
                })
                .collect(),
        },
        ExprNode::Let { id, name, value, body } => ExprNode::Let {
            id: *id,
            name: name.clone(),
            value: rewrite(value),
            body: rewrite(body),
        },
        ExprNode::Lambda { params, block_param, body, block_style } => ExprNode::Lambda {
            params: params.clone(),
            block_param: block_param.clone(),
            body: rewrite(body),
            block_style: *block_style,
        },
        ExprNode::Apply { fun, args, block } => ExprNode::Apply {
            fun: rewrite(fun),
            args: args.iter().map(&rewrite).collect(),
            block: block.as_ref().map(&rewrite),
        },
        ExprNode::Assign { target, value } => {
            let new_target = match target {
                LValue::Var { id, name } => LValue::Var { id: *id, name: name.clone() },
                LValue::Ivar { name } => LValue::Ivar { name: name.clone() },
                LValue::Attr { recv, name } => LValue::Attr {
                    recv: rewrite(recv),
                    name: name.clone(),
                },
                LValue::Index { recv, index } => LValue::Index {
                    recv: rewrite(recv),
                    index: rewrite(index),
                },
            };
            ExprNode::Assign {
                target: new_target,
                value: rewrite(value),
            }
        }
        ExprNode::Yield { args } => ExprNode::Yield {
            args: args.iter().map(&rewrite).collect(),
        },
        ExprNode::Raise { value } => ExprNode::Raise { value: rewrite(value) },
        ExprNode::RescueModifier { expr, fallback } => ExprNode::RescueModifier {
            expr: rewrite(expr),
            fallback: rewrite(fallback),
        },
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. } => (*e.node).clone(),
    };
    let _ = Pattern::Wildcard;
    Expr {
        span: e.span,
        node: Box::new(new_node),
        ty: e.ty.clone(),
        leading_blank_line: e.leading_blank_line,
    }
}

/// Convention: the record arg is the snake_case form of the module name.
/// `Post` → `post`, `ApplicationRecord` → `application_record`.
fn module_receiver_name(module: &str) -> String {
    snake_case(module)
}

// Controllers ----------------------------------------------------------
//
// Phase 4c: Elixir is the first dynamic target on this pass, so the
// shape is simpler than the typed twins — no `Response` struct, no
// ivar-as-struct-field decl, no nullable-unwrap. Every action still
// returns *something* (the existing convention emits `:ok` or whatever
// the body's tail produces), and the external surface (`params`,
// `render`, `redirect_to`, `head`) is imported from the emitted
// `Roundhouse.Http` stub module.
//
// Send rewrites applied inside controller bodies (via
// `emit_controller_send_ex`):
//   * `respond_to do |fmt| body end` → emit `body` directly; Elixir
//     has no FormatRouter, so the HTML branch flattens inline.
//   * `format.html { body }` → just `body`.
//   * `format.json { body }` → `# TODO: JSON branch (Phase 4e)` comment.
//   * `Model.new` / `Model.new(anything)` → `%Model{}`.
//   * unsupported query chains (`.all`/`.order`/...) → `[]`.
//   * `<assoc>.find` / `<assoc>.build` / `<assoc>.create` → `%Target{}`
//     via singularization.
//   * `x.destroy!` / `save!` / `update!` → strip the bang. Elixir
//     parses trailing-`!` idents as function names (`fetch!/2`), but
//     on a local-variable receiver the shape is map-access-then-call
//     and the intent is clearer without the bang.
//   * bare `*_path` / `*_url` → `""` (placeholder).
//   * `x.update(...)` → `false`.
//
// Ivars from Rails `before_action` filters: Elixir has no implicit
// controller state, so we inject a local binding at the top of any
// action that reads an ivar without first assigning it. Same posture
// as the typed-target Phase-4c emitters, different rendering.

fn emit_controller_file(c: &Controller, known_models: &[Symbol]) -> EmittedFile {
    let module = c.name.0.as_str();
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "defmodule {module} do").unwrap();

    let (public_actions, private_actions) = crate::lower::split_public_private(c);
    // Only emit `import Roundhouse.Http` when there's at least one
    // action body that might call into its surface. An empty module
    // (e.g. ApplicationController) with an unused import would fail
    // under `--warnings-as-errors`.
    if !public_actions.is_empty() || !private_actions.is_empty() {
        writeln!(s, "  import Roundhouse.Http").unwrap();
    }

    let self_methods: Vec<Symbol> = public_actions
        .iter()
        .chain(private_actions.iter())
        .map(|a| a.name.clone())
        .collect();

    let ctx = ExCtrlCtx {
        known_models,
        self_methods: &self_methods,
    };

    for action in &public_actions {
        writeln!(s).unwrap();
        emit_action(&mut s, action, "def", ctx);
    }
    // Private helpers emit as `def` (not `defp`) so Elixir's strict
    // `--warnings-as-errors` mode doesn't flag them as unused —
    // Phase 4c doesn't wire before_action dispatch, so they're not
    // called from anywhere inside the module. A public fn on a
    // controller module isn't unused by definition (external
    // callers might invoke it), which dodges the warning without
    // affecting the real semantics (which are Phase 4e+).
    for action in &private_actions {
        writeln!(s).unwrap();
        emit_action(&mut s, action, "def", ctx);
    }
    writeln!(s, "end").unwrap();
    let fname = format!("lib/{}.ex", snake_case(module));
    EmittedFile { path: PathBuf::from(fname), content: s }
}

fn emit_action(out: &mut String, a: &Action, def_kw: &str, ctx: ExCtrlCtx) {
    let name = a.name.as_str();

    // Gather the action body + any before_action ivar priming into a
    // single in-memory string before writing out, so we can decide
    // the arg name (`params` vs. `_params`) based on whether the
    // emitted text actually references `params`. The pre-rewrite IR
    // can reference `params` in shapes (e.g. `<assoc>.find(params.
    // expect(:id))`) that collapse to a default at emit time, so the
    // IR check can't predict the final usage.
    let mut body_text = String::new();
    let walked = crate::lower::walk_controller_ivars(&a.body);
    for ivar in walked.ivars_read_without_assign() {
        let default = ivar_default_ex(ivar.as_str(), ctx.known_models);
        body_text.push_str(&format!("{} = {default}\n", ivar.as_str()));
    }
    let body = emit_block_ctrl_ex(&a.body, ctx);
    if body.is_empty() && body_text.is_empty() {
        body_text.push_str(":ok");
    } else {
        body_text.push_str(&body);
    }
    // Append a trailing read when the last statement is an
    // assignment whose LHS isn't referenced later (matches Rails'
    // "ivar is the return value" convention and silences Elixir's
    // unused-variable warning).
    if let Some(tail_binding) = tail_assignment_binding(&a.body) {
        body_text.push('\n');
        body_text.push_str(&tail_binding);
    }

    let uses_params = emitted_text_uses_params(&body_text);
    let param_arg = if uses_params { "params" } else { "_params" };
    writeln!(out, "  {def_kw} {name}({param_arg}) do").unwrap();
    for line in body_text.lines() {
        writeln!(out, "    {line}").unwrap();
    }
    writeln!(out, "  end").unwrap();
}

/// Scan emitted Elixir text for a bare `params` reference — either
/// `params` at a word boundary, `params[...]`, or `params.<foo>`.
/// Post-rewrite check since some controller-body shapes (`<assoc>.
/// find(params.expect(:id))`) collapse to defaults that drop the
/// `params` reference, and we only learn that at emit time.
fn emitted_text_uses_params(text: &str) -> bool {
    // Simple pass: find "params" as a whole word (not part of a
    // longer identifier). Good enough for the patterns the emitter
    // produces — _params, article_params, etc. don't match.
    let bytes = text.as_bytes();
    let needle = b"params";
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            let prev_is_word = i > 0
                && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
            let next_idx = i + needle.len();
            let next_is_word = next_idx < bytes.len()
                && (bytes[next_idx].is_ascii_alphanumeric()
                    || bytes[next_idx] == b'_');
            if !prev_is_word && !next_is_word {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// If a body ends in an assignment whose LHS isn't read later in the
/// same body, return the LHS name. Used to tack a trailing reference
/// onto the emitted action so Elixir doesn't flag the binding unused.
fn tail_assignment_binding(body: &Expr) -> Option<String> {
    let tail = match &*body.node {
        ExprNode::Assign { .. } => body,
        ExprNode::Seq { exprs } if !exprs.is_empty() => exprs.last().unwrap(),
        _ => return None,
    };
    let name = match &*tail.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, .. }
        | ExprNode::Assign { target: LValue::Ivar { name }, .. } => name.to_string(),
        _ => return None,
    };
    // Build a quick set of names read *before* the tail stmt; if the
    // tail's LHS appears there, no trailing-ref is needed. In
    // practice Phase-4c action bodies are small enough that over-
    // emitting the tail ref is harmless, but skipping when already
    // read keeps the output cleaner.
    if let ExprNode::Seq { exprs } = &*body.node {
        for (i, e) in exprs.iter().enumerate() {
            if i + 1 == exprs.len() {
                break;
            }
            if reads_var(e, &name) {
                return None;
            }
        }
    }
    Some(name)
}

fn reads_var(e: &Expr, target: &str) -> bool {
    match &*e.node {
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } => name.as_str() == target,
        ExprNode::Send { recv, args, block, .. } => {
            recv.as_ref().is_some_and(|r| reads_var(r, target))
                || args.iter().any(|a| reads_var(a, target))
                || block.as_ref().is_some_and(|b| reads_var(b, target))
        }
        ExprNode::Assign { value, .. } => reads_var(value, target),
        ExprNode::Seq { exprs } => exprs.iter().any(|e| reads_var(e, target)),
        ExprNode::If { cond, then_branch, else_branch } => {
            reads_var(cond, target)
                || reads_var(then_branch, target)
                || reads_var(else_branch, target)
        }
        ExprNode::BoolOp { left, right, .. } => {
            reads_var(left, target) || reads_var(right, target)
        }
        ExprNode::Hash { entries, .. } => entries
            .iter()
            .any(|(k, v)| reads_var(k, target) || reads_var(v, target)),
        ExprNode::Array { elements, .. } => elements.iter().any(|el| reads_var(el, target)),
        ExprNode::Lambda { body, .. } => reads_var(body, target),
        _ => false,
    }
}

/// ActiveRecord-style method names that should route through
/// `Module.func(record)` on Elixir rather than `record.method` field
/// access. The closed set keeps attribute reads (`article.title`) out
/// of the rewrite.
fn is_ar_verb_ex(method: &str) -> bool {
    matches!(
        method,
        "save" | "save!" | "destroy" | "destroy!" | "update" | "update!"
            | "reload" | "touch" | "delete" | "validate" | "errors"
    )
}

/// Elixir default-value expression for an ivar. `@article` →
/// `%Article{}`; `@articles` → `[]`; unresolved names fall back to
/// `nil`.
fn ivar_default_ex(name: &str, known_models: &[Symbol]) -> String {
    let singular_class = crate::naming::singularize_camelize(name);
    let is_plural = singular_class.to_lowercase() != name.to_lowercase();
    if known_models.iter().any(|m| m.as_str() == singular_class) {
        if is_plural {
            return "[]".to_string();
        }
        return format!("%{}{{}}", singular_class);
    }
    "nil".to_string()
}

#[derive(Clone, Copy)]
struct ExCtrlCtx<'a> {
    known_models: &'a [Symbol],
    self_methods: &'a [Symbol],
}

fn emit_block_ctrl_ex(body: &Expr, ctx: ExCtrlCtx) -> String {
    match &*body.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            let mut lines: Vec<String> = Vec::new();
            for (i, e) in exprs.iter().enumerate() {
                if i > 0 && e.leading_blank_line {
                    lines.push(String::new());
                }
                lines.push(emit_stmt_ctrl_ex(e, ctx));
            }
            lines.join("\n")
        }
        _ => emit_stmt_ctrl_ex(body, ctx),
    }
}

fn emit_stmt_ctrl_ex(e: &Expr, ctx: ExCtrlCtx) -> String {
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value }
        | ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            format!("{} = {}", name, emit_expr_ctrl_ex(value, ctx))
        }
        _ => emit_expr_ctrl_ex(e, ctx),
    }
}

fn emit_expr_ctrl_ex(e: &Expr, ctx: ExCtrlCtx) -> String {
    match &*e.node {
        ExprNode::Send { recv, method, args, block, .. } => {
            if let Some(s) = emit_controller_send_ex(
                recv.as_ref(),
                method.as_str(),
                args,
                block.as_ref(),
                ctx,
            ) {
                return s;
            }
            // Fall back to the plain emit_send — but re-render args
            // with controller ctx so nested rewrites apply.
            let args_rendered: Vec<Expr> = args.to_vec();
            let _ = args_rendered;
            // Simplest: use existing emit_send but rebuild with ctx-
            // aware args by piping through emit_expr_ctrl_ex inline.
            let args_s: Vec<String> =
                args.iter().map(|a| emit_expr_ctrl_ex(a, ctx)).collect();
            render_plain_send(recv.as_ref(), method.as_str(), &args_s, ctx)
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            let cond_s = emit_expr_ctrl_ex(cond, ctx);
            let then_s = emit_block_ctrl_ex(then_branch, ctx);
            let else_s = emit_block_ctrl_ex(else_branch, ctx);
            format!(
                "if {cond_s} do\n{}\nelse\n{}\nend",
                indent(&then_s, 1),
                indent(&else_s, 1),
            )
        }
        ExprNode::Ivar { name } => name.to_string(),
        // All other shapes fall through to the controller-unaware
        // emitter — literals, hashes, interpolated strings, etc.
        _ => emit_expr(e, None),
    }
}

/// Plain Send emit with controller-ctx args already rendered. Used as
/// the fall-through when no controller-specific rewrite applies.
fn render_plain_send(
    recv: Option<&Expr>,
    method: &str,
    args_s: &[String],
    ctx: ExCtrlCtx,
) -> String {
    if method == "[]" && recv.is_some() {
        return format!(
            "{}[{}]",
            emit_expr_ctrl_ex(recv.unwrap(), ctx),
            args_s.join(", "),
        );
    }
    match recv {
        None => {
            if args_s.is_empty() {
                method.to_string()
            } else {
                format!("{}({})", method, args_s.join(", "))
            }
        }
        Some(r) => {
            let recv_s = emit_expr_ctrl_ex(r, ctx);
            if args_s.is_empty() {
                format!("{recv_s}.{method}")
            } else {
                format!("{recv_s}.{method}({})", args_s.join(", "))
            }
        }
    }
}

fn emit_controller_send_ex(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    ctx: ExCtrlCtx,
) -> Option<String> {
    use crate::lower::SendKind;
    let args_s: Vec<String> =
        args.iter().map(|a| emit_expr_ctrl_ex(a, ctx)).collect();

    // Try the shared classifier first. Elixir's unique rewrite —
    // struct-method-to-Module-function for model-typed receivers —
    // runs after the classifier returns None. (Not in the shared
    // enum: only Elixir needs this rewrite; the other three targets
    // have real method dispatch.)
    if let Some(kind) =
        crate::lower::classify_controller_send(recv, method, args, block, ctx.known_models)
    {
        return Some(match kind {
            // respond_to flattens inline in Elixir — no FormatRouter.
            SendKind::RespondToBlock { body } => emit_block_ctrl_ex(body, ctx),

            // format.html { body } unwraps to just `body` inline.
            SendKind::FormatHtml { body } => emit_block_ctrl_ex(body, ctx),
            SendKind::FormatJson => "# TODO: JSON branch (Phase 4e)".to_string(),

            SendKind::ParamsAccess => "params".to_string(),
            // `params.expect(...)` and `params[:k]` pass through the
            // dynamic runtime unchanged — Elixir doesn't typecheck
            // these at compile, and the stub exists so hand-written
            // code compiles.
            SendKind::ParamsExpect { .. } => {
                format!("params.expect({})", args_s.join(", "))
            }
            SendKind::ParamsIndex { .. } => {
                let arg = args_s.first().cloned().unwrap_or_default();
                format!("params[{arg}]")
            }

            SendKind::ModelNew { class } => format!("%{}{{}}", class.as_str()),
            // Model.find(x) passes through — the generated Elixir
            // model module has a `find/1` function and returns the
            // struct-or-nil; callers assign directly.
            SendKind::ModelFind { class, .. } => {
                let arg = args_s.first().cloned().unwrap_or_default();
                format!("{}.find({arg})", class.as_str())
            }

            SendKind::AssocLookup { target, .. } => format!("%{}{{}}", target.as_str()),

            SendKind::QueryChain { .. } => "[]".to_string(),

            SendKind::PathOrUrlHelper => "\"\"".to_string(),

            // Bang strip: `article.save!` → `Article.save(article)`
            // (combined with the Module.fn rewrite below for model
            // receivers) or just `recv.stripped` otherwise.
            SendKind::BangStrip { recv, stripped_method, args: _ } => {
                elixir_render_module_or_field_call(
                    recv,
                    stripped_method,
                    &args_s,
                    ctx,
                )
            }

            SendKind::InstanceUpdate => "false".to_string(),

            SendKind::Render { .. } => format!("render({})", args_s.join(", ")),
            SendKind::RedirectTo { .. } => {
                format!("redirect_to({})", args_s.join(", "))
            }
            SendKind::Head { .. } => format!("head({})", args_s.join(", ")),
        });
    }

    // Elixir-specific rewrite: `article.save` / `article.destroy` on
    // a model-typed local variable → `Article.save(article)`. Elixir
    // doesn't have struct method dispatch; the idiomatic form is
    // module-function-call with the struct as first arg. Recognise
    // the shape by checking whether the receiver's Var/Ivar name
    // singularizes to a known model.
    if let Some(r) = recv {
        if let ExprNode::Var { name, .. } | ExprNode::Ivar { name } = &*r.node {
            if crate::lower::singularize_to_model(name.as_str(), ctx.known_models)
                .is_some()
                && is_ar_verb_ex(method)
            {
                return Some(elixir_render_module_or_field_call(
                    r, method, &args_s, ctx,
                ));
            }
        }
    }

    None
}

/// Given a model-typed receiver + verb, render `Module.verb(recv,
/// args)`. For non-model receivers, fall back to plain `recv.verb(
/// args)`. Used by both the `BangStrip` and the Elixir-specific
/// AR-verb-on-model rewrites.
fn elixir_render_module_or_field_call(
    recv: &Expr,
    verb: &str,
    args_s: &[String],
    ctx: ExCtrlCtx,
) -> String {
    if let ExprNode::Var { name, .. } | ExprNode::Ivar { name } = &*recv.node {
        if let Some(target) =
            crate::lower::singularize_to_model(name.as_str(), ctx.known_models)
        {
            let recv_s = name.to_string();
            return if args_s.is_empty() {
                format!("{}.{}({})", target.as_str(), verb, recv_s)
            } else {
                format!(
                    "{}.{}({}, {})",
                    target.as_str(),
                    verb,
                    recv_s,
                    args_s.join(", "),
                )
            };
        }
    }
    let recv_s = emit_expr_ctrl_ex(recv, ctx);
    if args_s.is_empty() {
        format!("{recv_s}.{verb}")
    } else {
        format!("{recv_s}.{verb}({})", args_s.join(", "))
    }
}

// Router ---------------------------------------------------------------

fn emit_router_file(app: &App) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "defmodule Router do").unwrap();
    writeln!(s, "  @routes [").unwrap();
    let mut flat: Vec<(String, String, String, String)> = Vec::new();
    for entry in &app.routes.entries {
        collect_flat_routes(entry, &mut flat, None);
    }
    for (i, (method, path, controller, action)) in flat.iter().enumerate() {
        let sep = if i + 1 == flat.len() { "" } else { "," };
        writeln!(
            s,
            "    %{{method: :{}, path: {:?}, controller: {}, action: :{}}}{sep}",
            method.to_lowercase(),
            path,
            controller,
            action,
        )
        .unwrap();
    }
    writeln!(s, "  ]").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "  def routes, do: @routes").unwrap();
    writeln!(s, "end").unwrap();
    EmittedFile { path: PathBuf::from("lib/router.ex"), content: s }
}

fn collect_flat_routes(
    spec: &RouteSpec,
    out: &mut Vec<(String, String, String, String)>,
    scope_prefix: Option<(&str, &str)>,
) {
    match spec {
        RouteSpec::Explicit { method, path, controller, action, .. } => {
            let verb = match method {
                crate::dialect::HttpMethod::Get => "GET",
                crate::dialect::HttpMethod::Post => "POST",
                crate::dialect::HttpMethod::Put => "PUT",
                crate::dialect::HttpMethod::Patch => "PATCH",
                crate::dialect::HttpMethod::Delete => "DELETE",
                crate::dialect::HttpMethod::Head => "HEAD",
                crate::dialect::HttpMethod::Options => "OPTIONS",
                crate::dialect::HttpMethod::Any => "ANY",
            };
            let full_path = match scope_prefix {
                Some((parent, _)) => format!("/{parent}/:{parent}_id{path}"),
                None => path.clone(),
            };
            out.push((
                verb.to_string(),
                full_path,
                controller.0.to_string(),
                action.to_string(),
            ));
        }
        RouteSpec::Root { target } => {
            if let Some((c, a)) = target.split_once('#') {
                out.push((
                    "GET".into(),
                    "/".into(),
                    controller_class_name(c),
                    a.to_string(),
                ));
            }
        }
        RouteSpec::Resources { name, only, except, nested } => {
            let resource_path = format!("/{name}");
            let controller = controller_class_name(name.as_str());
            for (action, verb, suffix) in standard_resource_actions() {
                let action = *action;
                let verb = *verb;
                let suffix = *suffix;
                if !only.is_empty() && !only.iter().any(|s| s.as_str() == action) {
                    continue;
                }
                if except.iter().any(|s| s.as_str() == action) {
                    continue;
                }
                let path = format!("{resource_path}{suffix}");
                let full_path = match scope_prefix {
                    Some((parent, _)) => format!("/{parent}/:{parent}_id{path}"),
                    None => path,
                };
                out.push((verb.into(), full_path, controller.clone(), action.into()));
            }
            let singular =
                crate::naming::singularize_camelize(name.as_str()).to_lowercase();
            for child in nested {
                collect_flat_routes(child, out, Some((&singular, name.as_str())));
            }
        }
    }
}

fn standard_resource_actions() -> &'static [(&'static str, &'static str, &'static str)] {
    &[
        ("index", "GET", ""),
        ("new", "GET", "/new"),
        ("create", "POST", ""),
        ("show", "GET", "/:id"),
        ("edit", "GET", "/:id/edit"),
        ("update", "PATCH", "/:id"),
        ("destroy", "DELETE", "/:id"),
    ]
}

fn controller_class_name(short: &str) -> String {
    let mut s = crate::naming::camelize(short);
    s.push_str("Controller");
    s
}

// Pass-2 controllers ---------------------------------------------------
//
// The pass-1 emitter (`emit_controller_file`) returns :ok from every
// action to make `mix compile` pass. Pass-2 replaces that with
// template actions — each action returns an ActionResponse struct
// that Router.match-driven TestClient dispatch can assert on. The
// action bodies are synthesized per Rails CRUD shape (index, show,
// new, edit, create, update, destroy); arbitrary custom actions
// fall back to a 501 stub. Mirrors the Python/TS/Crystal pass-2
// controllers in shape.

fn emit_controller_file_pass2(
    c: &Controller,
    known_models: &[Symbol],
    _app: &App,
) -> EmittedFile {
    let module = c.name.0.as_str();
    let resource = resource_from_controller_name_ex(module);
    let model_class = crate::naming::singularize_camelize(&resource);
    let has_model = known_models.iter().any(|m| m.as_str() == model_class);
    let parent = find_nested_parent_ex(module);
    let permitted = permitted_fields_for_ex(c, &resource)
        .unwrap_or_else(|| default_permitted_fields_ex(&model_class));

    let (public_actions, _private) = crate::lower::split_public_private(c);

    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "defmodule {module} do").unwrap();
    if !public_actions.is_empty() {
        writeln!(s, "  alias Roundhouse.Http.ActionResponse").unwrap();
        writeln!(s, "  import Roundhouse.RouteHelpers").unwrap();
        writeln!(s).unwrap();
    }

    for action in &public_actions {
        emit_ex_action_template(
            &mut s,
            action,
            &resource,
            &model_class,
            has_model,
            parent.as_ref(),
            &permitted,
        );
        writeln!(s).unwrap();
    }
    writeln!(s, "end").unwrap();

    let fname = format!("lib/{}.ex", snake_case(module));
    EmittedFile {
        path: PathBuf::from(fname),
        content: s,
    }
}

fn resource_from_controller_name_ex(name: &str) -> String {
    let trimmed = name.strip_suffix("Controller").unwrap_or(name);
    crate::naming::singularize(&crate::naming::snake_case(trimmed))
}

#[derive(Clone, Debug)]
struct ExNestedParent {
    singular: String,
    #[allow(dead_code)]
    plural: String,
}

fn find_nested_parent_ex(controller_name: &str) -> Option<ExNestedParent> {
    let resource = resource_from_controller_name_ex(controller_name);
    if resource == "comment" {
        Some(ExNestedParent {
            singular: "article".to_string(),
            plural: "articles".to_string(),
        })
    } else {
        None
    }
}

fn permitted_fields_for_ex(c: &Controller, resource: &str) -> Option<Vec<String>> {
    use crate::dialect::ControllerBodyItem;
    let helper_name = format!("{}_params", resource);
    let action = c.body.iter().find_map(|item| match item {
        ControllerBodyItem::Action { action, .. }
            if action.name.as_str() == helper_name =>
        {
            Some(action)
        }
        _ => None,
    })?;
    extract_permitted_from_expr_ex(&action.body)
}

fn extract_permitted_from_expr_ex(expr: &Expr) -> Option<Vec<String>> {
    if let ExprNode::Send { recv: Some(r), method, args, .. } = &*expr.node {
        if method.as_str() == "expect" && crate::lower::is_params_expr(r) {
            if let Some(arg) = args.first() {
                if let ExprNode::Hash { entries, .. } = &*arg.node {
                    if let Some((_, value)) = entries.first() {
                        if let ExprNode::Array { elements, .. } = &*value.node {
                            let fields: Vec<String> = elements
                                .iter()
                                .filter_map(|e| match &*e.node {
                                    ExprNode::Lit {
                                        value: Literal::Sym { value },
                                    } => Some(value.as_str().to_string()),
                                    _ => None,
                                })
                                .collect();
                            if !fields.is_empty() {
                                return Some(fields);
                            }
                        }
                    }
                }
            }
        }
    }
    if let ExprNode::Seq { exprs } = &*expr.node {
        for e in exprs {
            if let Some(v) = extract_permitted_from_expr_ex(e) {
                return Some(v);
            }
        }
    }
    None
}

fn default_permitted_fields_ex(model_class: &str) -> Vec<String> {
    match model_class {
        "Article" => vec!["title".to_string(), "body".to_string()],
        "Comment" => vec!["commenter".to_string(), "body".to_string()],
        _ => Vec::new(),
    }
}

fn ex_view_fn(model_class: &str, suffix: &str) -> String {
    let plural = crate::naming::pluralize_snake(model_class);
    format!("render_{plural}_{}", suffix.to_lowercase())
}

fn emit_ex_action_template(
    out: &mut String,
    action: &Action,
    resource: &str,
    model_class: &str,
    has_model: bool,
    parent: Option<&ExNestedParent>,
    permitted: &[String],
) {
    let raw = action.name.as_str();
    match raw {
        "index" => emit_ex_index(out, raw, model_class, has_model),
        "show" => emit_ex_show(out, raw, model_class, has_model),
        "new" => emit_ex_new(out, raw, model_class, has_model),
        "edit" => emit_ex_edit(out, raw, model_class, has_model),
        "create" => emit_ex_create(
            out, raw, resource, model_class, has_model, parent, permitted,
        ),
        "update" => emit_ex_update(
            out, raw, resource, model_class, has_model, permitted,
        ),
        "destroy" => emit_ex_destroy(out, raw, model_class, has_model, parent),
        _ => {
            writeln!(out, "  def {raw}(_context) do").unwrap();
            writeln!(out, "    %ActionResponse{{status: 501}}").unwrap();
            writeln!(out, "  end").unwrap();
        }
    }
}

fn emit_ex_index(out: &mut String, name: &str, model_class: &str, has_model: bool) {
    let view_fn = ex_view_fn(model_class, "index");
    writeln!(out, "  def {name}(_context) do").unwrap();
    if has_model {
        writeln!(out, "    records = {model_class}.all()").unwrap();
        writeln!(
            out,
            "    %ActionResponse{{body: App.Views.{view_fn}(records)}}"
        )
        .unwrap();
    } else {
        writeln!(out, "    %ActionResponse{{body: \"\"}}").unwrap();
    }
    writeln!(out, "  end").unwrap();
}

fn emit_ex_show(out: &mut String, name: &str, model_class: &str, has_model: bool) {
    let view_fn = ex_view_fn(model_class, "show");
    writeln!(out, "  def {name}(context) do").unwrap();
    if has_model {
        writeln!(out, "    record_id = String.to_integer(to_string(context.params[\"id\"]))").unwrap();
        writeln!(out, "    record = {model_class}.find(record_id) || %{model_class}{{}}").unwrap();
        writeln!(
            out,
            "    %ActionResponse{{body: App.Views.{view_fn}(record)}}"
        )
        .unwrap();
    } else {
        writeln!(out, "    %ActionResponse{{body: \"\"}}").unwrap();
    }
    writeln!(out, "  end").unwrap();
}

fn emit_ex_new(out: &mut String, name: &str, model_class: &str, has_model: bool) {
    let view_fn = ex_view_fn(model_class, "new");
    writeln!(out, "  def {name}(_context) do").unwrap();
    if has_model {
        writeln!(out, "    record = %{model_class}{{}}").unwrap();
        writeln!(
            out,
            "    %ActionResponse{{body: App.Views.{view_fn}(record)}}"
        )
        .unwrap();
    } else {
        writeln!(out, "    %ActionResponse{{body: \"\"}}").unwrap();
    }
    writeln!(out, "  end").unwrap();
}

fn emit_ex_edit(out: &mut String, name: &str, model_class: &str, has_model: bool) {
    let view_fn = ex_view_fn(model_class, "edit");
    writeln!(out, "  def {name}(context) do").unwrap();
    if has_model {
        writeln!(out, "    record_id = String.to_integer(to_string(context.params[\"id\"]))").unwrap();
        writeln!(out, "    record = {model_class}.find(record_id) || %{model_class}{{}}").unwrap();
        writeln!(
            out,
            "    %ActionResponse{{body: App.Views.{view_fn}(record)}}"
        )
        .unwrap();
    } else {
        writeln!(out, "    %ActionResponse{{body: \"\"}}").unwrap();
    }
    writeln!(out, "  end").unwrap();
}

fn emit_ex_create(
    out: &mut String,
    name: &str,
    resource: &str,
    model_class: &str,
    has_model: bool,
    parent: Option<&ExNestedParent>,
    permitted: &[String],
) {
    let uses_context = has_model && (parent.is_some() || !permitted.is_empty());
    let arg = if uses_context { "context" } else { "_context" };
    writeln!(out, "  def {name}({arg}) do").unwrap();
    if !has_model {
        writeln!(out, "    %ActionResponse{{body: \"\"}}").unwrap();
        writeln!(out, "  end").unwrap();
        return;
    }
    writeln!(out, "    record = %{model_class}{{}}").unwrap();
    if let Some(p) = parent {
        writeln!(
            out,
            "    record = %{{record | {0}_id: String.to_integer(to_string(context.params[\"{0}_id\"]))}}",
            p.singular
        )
        .unwrap();
    }
    let mut field_assigns: Vec<String> = Vec::new();
    for field in permitted {
        field_assigns.push(format!(
            "{field}: Map.get(context.params, \"{resource}[{field}]\", \"\")"
        ));
    }
    if !field_assigns.is_empty() {
        writeln!(
            out,
            "    record = %{{record | {}}}",
            field_assigns.join(", ")
        )
        .unwrap();
    }
    writeln!(out, "    if {model_class}.save(record) do").unwrap();
    if let Some(p) = parent {
        writeln!(
            out,
            "      %ActionResponse{{status: 303, location: {0}_path(String.to_integer(to_string(context.params[\"{0}_id\"])))}}",
            p.singular,
        )
        .unwrap();
    } else {
        writeln!(
            out,
            "      record = %{{record | id: Roundhouse.Db.last_insert_rowid()}}"
        )
        .unwrap();
        writeln!(
            out,
            "      %ActionResponse{{status: 303, location: {resource}_path(record.id)}}"
        )
        .unwrap();
    }
    writeln!(out, "    else").unwrap();
    if let Some(p) = parent {
        writeln!(
            out,
            "      %ActionResponse{{status: 303, location: {0}_path(String.to_integer(to_string(context.params[\"{0}_id\"])))}}",
            p.singular,
        )
        .unwrap();
    } else {
        let view_fn = ex_view_fn(model_class, "new");
        writeln!(
            out,
            "      %ActionResponse{{status: 422, body: App.Views.{view_fn}(record)}}"
        )
        .unwrap();
    }
    writeln!(out, "    end").unwrap();
    writeln!(out, "  end").unwrap();
}

fn emit_ex_update(
    out: &mut String,
    name: &str,
    resource: &str,
    model_class: &str,
    has_model: bool,
    permitted: &[String],
) {
    let arg = if has_model { "context" } else { "_context" };
    writeln!(out, "  def {name}({arg}) do").unwrap();
    if !has_model {
        writeln!(out, "    %ActionResponse{{body: \"\"}}").unwrap();
        writeln!(out, "  end").unwrap();
        return;
    }
    writeln!(out, "    record_id = String.to_integer(to_string(context.params[\"id\"]))").unwrap();
    writeln!(out, "    record = {model_class}.find(record_id) || %{model_class}{{}}").unwrap();
    for field in permitted {
        writeln!(
            out,
            "    record = if Map.has_key?(context.params, \"{resource}[{field}]\"), do: %{{record | {field}: context.params[\"{resource}[{field}]\"]}}, else: record"
        )
        .unwrap();
    }
    writeln!(out, "    if {model_class}.save(record) do").unwrap();
    writeln!(
        out,
        "      %ActionResponse{{status: 303, location: {resource}_path(record.id)}}"
    )
    .unwrap();
    writeln!(out, "    else").unwrap();
    let edit_view = ex_view_fn(model_class, "edit");
    writeln!(
        out,
        "      %ActionResponse{{status: 422, body: App.Views.{edit_view}(record)}}"
    )
    .unwrap();
    writeln!(out, "    end").unwrap();
    writeln!(out, "  end").unwrap();
}

fn emit_ex_destroy(
    out: &mut String,
    name: &str,
    model_class: &str,
    has_model: bool,
    parent: Option<&ExNestedParent>,
) {
    let arg = if has_model { "context" } else { "_context" };
    writeln!(out, "  def {name}({arg}) do").unwrap();
    if !has_model {
        writeln!(out, "    %ActionResponse{{body: \"\"}}").unwrap();
        writeln!(out, "  end").unwrap();
        return;
    }
    writeln!(out, "    record_id = String.to_integer(to_string(context.params[\"id\"]))").unwrap();
    writeln!(out, "    record = {model_class}.find(record_id)").unwrap();
    writeln!(
        out,
        "    if record != nil, do: {model_class}.destroy(record)"
    )
    .unwrap();
    if let Some(p) = parent {
        writeln!(
            out,
            "    %ActionResponse{{status: 303, location: {0}_path(String.to_integer(to_string(context.params[\"{0}_id\"])))}}",
            p.singular,
        )
        .unwrap();
    } else {
        let plural = crate::naming::pluralize_snake(model_class);
        writeln!(
            out,
            "    %ActionResponse{{status: 303, location: {plural}_path()}}"
        )
        .unwrap();
    }
    writeln!(out, "  end").unwrap();
}

// Pass-2 route helpers -------------------------------------------------

fn emit_ex_route_helpers(app: &App) -> EmittedFile {
    let flat = flatten_ex_routes(app);
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "defmodule Roundhouse.RouteHelpers do").unwrap();
    writeln!(s, "  @moduledoc false").unwrap();
    writeln!(s).unwrap();
    use std::collections::BTreeSet;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for route in &flat {
        if !seen.insert(route.as_name.clone()) {
            continue;
        }
        let fn_name = format!("{}_path", route.as_name);
        let params: Vec<String> = route
            .path_params
            .iter()
            .map(|p| format!("{p} \\\\ 0"))
            .collect();
        let sig = if params.is_empty() {
            String::new()
        } else {
            format!("({})", params.join(", "))
        };
        let body = if route.path_params.is_empty() {
            format!("\"{}\"", route.path)
        } else {
            let mut interp = String::new();
            let mut chars = route.path.chars().peekable();
            while let Some(c) = chars.next() {
                if c == ':' {
                    let mut ident = String::new();
                    while let Some(&nc) = chars.peek() {
                        if nc.is_alphanumeric() || nc == '_' {
                            ident.push(nc);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    interp.push_str("#{");
                    interp.push_str(&ident);
                    interp.push('}');
                } else {
                    interp.push(c);
                }
            }
            format!("\"{}\"", interp)
        };
        writeln!(s, "  def {fn_name}{sig}, do: {body}").unwrap();
    }
    writeln!(s, "end").unwrap();
    EmittedFile {
        path: PathBuf::from("lib/roundhouse/route_helpers.ex"),
        content: s,
    }
}

#[derive(Debug)]
struct ExFlatRoute {
    path: String,
    as_name: String,
    path_params: Vec<String>,
}

fn flatten_ex_routes(app: &App) -> Vec<ExFlatRoute> {
    let mut out = Vec::new();
    for entry in &app.routes.entries {
        collect_flat_ex_routes(entry, &mut out, None);
    }
    out
}

fn collect_flat_ex_routes(
    spec: &RouteSpec,
    out: &mut Vec<ExFlatRoute>,
    scope_prefix: Option<(&str, &str)>,
) {
    match spec {
        RouteSpec::Explicit { path, action, as_name, .. } => {
            let (full_path, mut params) = nest_ex_path(path, scope_prefix);
            extract_ex_path_params(&full_path, &mut params);
            out.push(ExFlatRoute {
                path: full_path,
                as_name: as_name
                    .as_ref()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| action.to_string()),
                path_params: params,
            });
        }
        RouteSpec::Root { .. } => {
            out.push(ExFlatRoute {
                path: "/".to_string(),
                as_name: "root".to_string(),
                path_params: vec![],
            });
        }
        RouteSpec::Resources { name, only, except, nested } => {
            let resource_path = format!("/{name}");
            let singular_low =
                crate::naming::singularize_camelize(name.as_str()).to_lowercase();
            for (action, _method, suffix) in standard_resource_actions() {
                let action: &str = action;
                let suffix: &str = suffix;
                if !only.is_empty()
                    && !only.iter().any(|s| s.as_str() == action)
                {
                    continue;
                }
                if except.iter().any(|s| s.as_str() == action) {
                    continue;
                }
                let path = format!("{resource_path}{suffix}");
                let (full_path, mut params) = nest_ex_path(&path, scope_prefix);
                if suffix.contains(":id") && !params.iter().any(|p| p == "id") {
                    params.push("id".to_string());
                }
                let as_name = ex_resource_as_name(
                    action,
                    &singular_low,
                    name.as_str(),
                    scope_prefix,
                );
                out.push(ExFlatRoute {
                    path: full_path,
                    as_name,
                    path_params: params,
                });
            }
            for child in nested {
                collect_flat_ex_routes(
                    child,
                    out,
                    Some((&singular_low, name.as_str())),
                );
            }
        }
    }
}

fn nest_ex_path(
    path: &str,
    scope_prefix: Option<(&str, &str)>,
) -> (String, Vec<String>) {
    match scope_prefix {
        Some((parent, parent_plural)) => {
            let full = format!("/{parent_plural}/:{parent}_id{path}");
            let params = vec![format!("{parent}_id")];
            (full, params)
        }
        None => (path.to_string(), vec![]),
    }
}

fn extract_ex_path_params(path: &str, params: &mut Vec<String>) {
    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ':' {
            let mut ident = String::new();
            while let Some(&nc) = chars.peek() {
                if nc.is_alphanumeric() || nc == '_' {
                    ident.push(nc);
                    chars.next();
                } else {
                    break;
                }
            }
            if !ident.is_empty() && !params.iter().any(|p| p == &ident) {
                params.push(ident);
            }
        }
    }
}

fn ex_resource_as_name(
    action: &str,
    singular_low: &str,
    plural: &str,
    scope_prefix: Option<(&str, &str)>,
) -> String {
    let parent_prefix = scope_prefix
        .map(|(p, _)| format!("{p}_"))
        .unwrap_or_default();
    match action {
        "index" => format!("{parent_prefix}{plural}"),
        "create" => format!("{parent_prefix}{plural}"),
        "new" => format!("new_{parent_prefix}{singular_low}"),
        "show" => format!("{parent_prefix}{singular_low}"),
        "edit" => format!("edit_{parent_prefix}{singular_low}"),
        "update" => format!("{parent_prefix}{singular_low}"),
        "destroy" => format!("{parent_prefix}{singular_low}"),
        _ => format!("{parent_prefix}{singular_low}"),
    }
}

// Pass-2 route registration --------------------------------------------

fn emit_ex_routes_register(app: &App) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "defmodule App.Routes do").unwrap();
    writeln!(s, "  @moduledoc false").unwrap();
    writeln!(s, "  alias Roundhouse.Http.Router").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "  def register do").unwrap();
    writeln!(s, "    Router.reset()").unwrap();
    for entry in &app.routes.entries {
        emit_ex_route_spec(&mut s, entry);
    }
    writeln!(s, "    :ok").unwrap();
    writeln!(s, "  end").unwrap();
    writeln!(s, "end").unwrap();
    EmittedFile {
        path: PathBuf::from("lib/app/routes.ex"),
        content: s,
    }
}

fn emit_ex_route_spec(out: &mut String, spec: &RouteSpec) {
    match spec {
        RouteSpec::Explicit { method, path, controller, action, .. } => {
            let verb = match method {
                crate::dialect::HttpMethod::Get => "get",
                crate::dialect::HttpMethod::Post => "post",
                crate::dialect::HttpMethod::Put => "put",
                crate::dialect::HttpMethod::Patch => "patch",
                crate::dialect::HttpMethod::Delete => "delete",
                _ => "get",
            };
            writeln!(
                out,
                "    Router.{verb}({:?}, {}, :{})",
                path,
                controller.0,
                action.as_str(),
            )
            .unwrap();
        }
        RouteSpec::Root { target } => {
            let (controller, action) = target
                .split_once('#')
                .map(|(c, a)| (controller_class_name(c), a.to_string()))
                .unwrap_or_else(|| (target.clone(), "index".to_string()));
            writeln!(out, "    Router.root({controller}, :{action})").unwrap();
        }
        RouteSpec::Resources { name, only, except: _, nested } => {
            let controller = controller_class_name(name.as_str());
            let mut opts: Vec<String> = Vec::new();
            if !only.is_empty() {
                let parts: Vec<String> =
                    only.iter().map(|s| format!(":{}", s.as_str())).collect();
                opts.push(format!("only: [{}]", parts.join(", ")));
            }
            if !nested.is_empty() {
                let mut nested_parts: Vec<String> = Vec::new();
                for child in nested {
                    if let Some(part) = ex_nested_spec_entry(child) {
                        nested_parts.push(part);
                    }
                }
                if !nested_parts.is_empty() {
                    opts.push(format!("nested: [{}]", nested_parts.join(", ")));
                }
            }
            if opts.is_empty() {
                writeln!(
                    out,
                    "    Router.resources({:?}, {})",
                    name.as_str(),
                    controller
                )
                .unwrap();
            } else {
                writeln!(
                    out,
                    "    Router.resources({:?}, {}, [{}])",
                    name.as_str(),
                    controller,
                    opts.join(", ")
                )
                .unwrap();
            }
        }
    }
}

fn ex_nested_spec_entry(spec: &RouteSpec) -> Option<String> {
    let RouteSpec::Resources { name, only, except: _, nested: _ } = spec else {
        return None;
    };
    let controller = controller_class_name(name.as_str());
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!("name: {:?}", name.as_str()));
    parts.push(format!("controller: {}", controller));
    if !only.is_empty() {
        let items: Vec<String> =
            only.iter().map(|s| format!(":{}", s.as_str())).collect();
        parts.push(format!("only: [{}]", items.join(", ")));
    }
    Some(format!("[{}]", parts.join(", ")))
}

// Pass-2 views ---------------------------------------------------------

fn emit_ex_views(app: &App) -> EmittedFile {
    let known_models: Vec<Symbol> =
        app.models.iter().map(|m| m.name.0.clone()).collect();
    let attrs_by_class: std::collections::BTreeMap<String, Vec<String>> = app
        .models
        .iter()
        .map(|m| {
            (
                m.name.0.as_str().to_string(),
                m.attributes
                    .fields
                    .keys()
                    .map(|k| k.as_str().to_string())
                    .collect(),
            )
        })
        .collect();

    let mut body = String::new();

    let mut emitted_names: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();

    for v in &app.views {
        let fn_name = ex_view_function_name(v.name.as_str());
        if !emitted_names.insert(fn_name.clone()) {
            continue;
        }
        emit_view_file_pass2_ex(&mut body, v, &known_models, &attrs_by_class);
        writeln!(body).unwrap();
    }

    // Stub missing standard CRUD views so controllers always link.
    for model in &app.models {
        if model.attributes.fields.is_empty() {
            continue;
        }
        let class = model.name.0.as_str();
        let plural = crate::naming::pluralize_snake(class);
        for (_, suffix) in [
            ("Index", "index"),
            ("Show", "show"),
            ("New", "new"),
            ("Edit", "edit"),
        ] {
            let view_name = format!("{plural}/{suffix}");
            let fn_name = ex_view_function_name(&view_name);
            if emitted_names.insert(fn_name.clone()) {
                writeln!(body, "  def {fn_name}(_record), do: \"\"").unwrap();
                writeln!(body).unwrap();
            }
        }
    }

    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "defmodule App.Views do").unwrap();
    writeln!(s, "  @moduledoc false").unwrap();
    if ex_text_references(&body, "ViewHelpers") {
        writeln!(s, "  alias Roundhouse.ViewHelpers").unwrap();
    }
    if ex_text_references(&body, "FormBuilder") {
        writeln!(s, "  alias Roundhouse.FormBuilder").unwrap();
    }
    writeln!(s).unwrap();
    s.push_str(&body);
    writeln!(s, "end").unwrap();
    EmittedFile {
        path: PathBuf::from("lib/app/views.ex"),
        content: s,
    }
}

fn ex_view_function_name(name: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for seg in name.split('/') {
        let trimmed = seg.strip_prefix('_').unwrap_or(seg);
        parts.push(trimmed.to_string());
    }
    format!("render_{}", parts.join("_"))
}

fn emit_view_file_pass2_ex(
    out: &mut String,
    view: &crate::dialect::View,
    known_models: &[Symbol],
    attrs_by_class: &std::collections::BTreeMap<String, Vec<String>>,
) {
    let fn_name = ex_view_function_name(view.name.as_str());
    let (arg_name, arg_model) = ex_view_signature(view.name.as_str(), known_models);
    let attrs = arg_model
        .as_ref()
        .and_then(|c| attrs_by_class.get(c).cloned())
        .unwrap_or_default();

    writeln!(out, "  def {fn_name}({arg_name}) do").unwrap();
    writeln!(out, "    _ = {arg_name}").unwrap();
    writeln!(out, "    buf = \"\"").unwrap();

    let mut locals: Vec<String> = vec!["buf".to_string(), arg_name.clone()];
    let ivar_names = ex_collect_ivar_names(&view.body);
    for n in &ivar_names {
        if !locals.iter().any(|x| x == n) {
            locals.push(n.clone());
        }
    }
    let resource_dir = view
        .name
        .as_str()
        .rsplit_once('/')
        .map(|(d, _): (&str, &str)| d.to_string())
        .unwrap_or_default();
    let ctx = ExViewCtx {
        locals,
        arg_name: arg_name.clone(),
        arg_attrs: attrs,
        resource_dir,
    };

    let body_lines = emit_ex_view_body(&view.body, &ctx);
    for line in body_lines {
        writeln!(out, "    {line}").unwrap();
    }
    writeln!(out, "    buf").unwrap();
    writeln!(out, "  end").unwrap();
}

struct ExViewCtx {
    locals: Vec<String>,
    arg_name: String,
    arg_attrs: Vec<String>,
    resource_dir: String,
}

impl ExViewCtx {
    fn is_local(&self, n: &str) -> bool {
        self.locals.iter().any(|x| x == n)
    }
    fn arg_has_attr(&self, name: &str, attr: &str) -> bool {
        name == self.arg_name && self.arg_attrs.iter().any(|a| a == attr)
    }
}

fn ex_view_signature(
    view_name: &str,
    known_models: &[Symbol],
) -> (String, Option<String>) {
    let (dir, base) = view_name.rsplit_once('/').unwrap_or(("", view_name));
    let is_partial = base.starts_with('_');
    let stem = base.trim_start_matches('_');
    let model_class = crate::naming::singularize_camelize(dir);
    let model_exists = known_models.iter().any(|m| m.as_str() == model_class);
    let singular = crate::naming::singularize(dir);

    if is_partial {
        let arg_name = if model_exists { singular } else { stem.to_string() };
        return (arg_name, if model_exists { Some(model_class) } else { None });
    }
    match stem {
        "index" => (dir.to_string(), if model_exists { Some(model_class) } else { None }),
        _ => (singular, if model_exists { Some(model_class) } else { None }),
    }
}

fn emit_ex_view_body(body: &Expr, ctx: &ExViewCtx) -> Vec<String> {
    let stmts: Vec<&Expr> = match &*body.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![body],
    };
    let mut out = Vec::new();
    for stmt in &stmts {
        out.extend(emit_ex_view_stmt_pass2(stmt, ctx));
    }
    out
}

fn emit_ex_view_stmt_pass2(stmt: &Expr, ctx: &ExViewCtx) -> Vec<String> {
    match &*stmt.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value }
            if name.as_str() == "_buf" =>
        {
            if let ExprNode::Lit { value: Literal::Str { value: s } } = &*value.node {
                if s.is_empty() {
                    return Vec::new();
                }
            }
            if let ExprNode::Send { recv: Some(recv), method, args, .. } = &*value.node {
                if method.as_str() == "+" && args.len() == 1 {
                    if let ExprNode::Var { name: rn, .. } = &*recv.node {
                        if rn.as_str() == "_buf" {
                            let chunk = emit_ex_view_append_pass2(&args[0], ctx);
                            return chunk
                                .lines()
                                .map(|l| l.to_string())
                                .collect();
                        }
                    }
                }
            }
            vec!["# TODO ERB: buf shape".to_string()]
        }
        ExprNode::Var { name, .. } if name.as_str() == "_buf" => Vec::new(),
        ExprNode::Ivar { .. } => Vec::new(),
        ExprNode::If { cond, then_branch, else_branch } => {
            let cond_s = if is_ex_simple_expr(cond, ctx) {
                emit_ex_view_expr_raw(cond, ctx)
            } else {
                "false".to_string()
            };
            let mut out = vec![format!("buf = if {cond_s} do")];
            let then_lines = emit_ex_view_body(then_branch, ctx);
            out.push("  buf = buf".to_string());
            for line in then_lines {
                out.push(format!("  {line}"));
            }
            out.push("  buf".to_string());
            let has_else = !matches!(
                &*else_branch.node,
                ExprNode::Lit { value: Literal::Nil }
            );
            out.push("else".to_string());
            if has_else {
                out.push("  buf = buf".to_string());
                for line in emit_ex_view_body(else_branch, ctx) {
                    out.push(format!("  {line}"));
                }
                out.push("  buf".to_string());
            } else {
                out.push("  buf".to_string());
            }
            out.push("end".to_string());
            out
        }
        ExprNode::Send { recv: Some(recv), method, args, block: Some(block), .. }
            if method.as_str() == "each" && args.is_empty() =>
        {
            if !is_ex_simple_expr(recv, ctx) {
                return vec!["# TODO ERB: each over complex coll".to_string()];
            }
            let ExprNode::Lambda { params, body, .. } = &*block.node else {
                return vec!["# unexpected each block".to_string()];
            };
            let coll_s = emit_ex_view_expr_raw(recv, ctx);
            let var = params
                .first()
                .map(|p| p.as_str().to_string())
                .unwrap_or_else(|| "item".into());
            let inner_ctx = ExViewCtx {
                locals: {
                    let mut l = ctx.locals.clone();
                    if !l.iter().any(|x| x == &var) {
                        l.push(var.clone());
                    }
                    l
                },
                arg_name: ctx.arg_name.clone(),
                arg_attrs: ctx.arg_attrs.clone(),
                resource_dir: ctx.resource_dir.clone(),
            };
            let inner_lines = emit_ex_view_body(body, &inner_ctx);
            let inner_text = inner_lines.join("\n");
            let emitted_var = if ex_text_references(&inner_text, &var) {
                var.clone()
            } else {
                format!("_{var}")
            };
            let mut out = vec![format!(
                "buf = Enum.reduce({coll_s}, buf, fn {emitted_var}, buf ->"
            )];
            for line in inner_lines {
                out.push(format!("  {line}"));
            }
            out.push("  buf".to_string());
            out.push("end)".to_string());
            out
        }
        _ => vec!["# TODO ERB: unknown stmt".to_string()],
    }
}

fn emit_ex_view_append_pass2(arg: &Expr, ctx: &ExViewCtx) -> String {
    if let ExprNode::Lit { value: Literal::Str { value: s } } = &*arg.node {
        return format!("buf = buf <> {}", ex_string_literal(s));
    }
    let inner = unwrap_to_s_ex(arg);

    if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*inner.node {
        if method.as_str() == "render" {
            if args.len() == 1 {
                return emit_ex_render_call(&args[0], ctx);
            }
            if args.len() == 2 {
                if let (
                    ExprNode::Lit { value: Literal::Str { value: partial } },
                    ExprNode::Hash { entries, .. },
                ) = (&*args[0].node, &*args[1].node)
                {
                    let partial_fn = format!(
                        "render_{}_{}",
                        ctx.resource_dir,
                        partial.trim_start_matches('_'),
                    );
                    if let Some((_, v)) = entries.first() {
                        if is_ex_simple_expr(v, ctx) {
                            let arg_expr = emit_ex_view_expr_raw(v, ctx);
                            return format!("buf = buf <> {partial_fn}({arg_expr})");
                        }
                    }
                    return format!("buf = buf <> {partial_fn}(nil)");
                }
            }
        }
    }

    if let ExprNode::Send {
        recv: None,
        method,
        args,
        block: Some(block),
        ..
    } = &*inner.node
    {
        if is_ex_capturing_helper(method.as_str()) {
            return emit_ex_captured_helper(method.as_str(), args, block, ctx);
        }
    }

    if is_ex_simple_expr(inner, ctx) {
        return format!(
            "buf = buf <> to_string({})",
            emit_ex_view_expr_raw(inner, ctx),
        );
    }

    "buf = buf <> \"\" # TODO ERB: complex interpolation".to_string()
}

fn is_ex_capturing_helper(method: &str) -> bool {
    matches!(method, "form_with" | "content_for")
}

fn emit_ex_captured_helper(
    method: &str,
    args: &[Expr],
    block: &Expr,
    ctx: &ExViewCtx,
) -> String {
    let ExprNode::Lambda { params, body, .. } = &*block.node else {
        return format!("buf = buf <> \"\" # TODO ERB: {method}");
    };
    let cls_expr = args
        .iter()
        .find_map(|a| ex_extract_kwarg(a, "class"))
        .filter(|e| is_ex_simple_expr(e, ctx))
        .map(|e| emit_ex_view_expr_raw(e, ctx))
        .unwrap_or_else(|| "\"\"".to_string());
    match method {
        "form_with" => {
            let pname = params.first().map(|p| p.as_str()).unwrap_or("form");
            let inner_ctx = ExViewCtx {
                locals: {
                    let mut l = ctx.locals.clone();
                    l.push(pname.to_string());
                    l.push("form_begin".to_string());
                    l
                },
                arg_name: ctx.arg_name.clone(),
                arg_attrs: ctx.arg_attrs.clone(),
                resource_dir: ctx.resource_dir.clone(),
            };
            let body_lines = emit_ex_view_body(body, &inner_ctx);
            let body_text = body_lines.join("\n");
            let form_binding = if ex_text_references(&body_text, pname) {
                format!("{pname} = %FormBuilder{{record: nil}}")
            } else {
                format!("_{pname} = %FormBuilder{{record: nil}}")
            };
            let mut lines = vec![
                "form_begin = byte_size(buf)".to_string(),
                form_binding,
            ];
            for line in body_lines {
                lines.push(line);
            }
            lines.push(format!(
                "buf = binary_part(buf, 0, form_begin) <> ViewHelpers.form_wrap(nil, {cls_expr}, binary_part(buf, form_begin, byte_size(buf) - form_begin))"
            ));
            lines.join("\n")
        }
        _ => {
            let _ = cls_expr;
            "buf = buf <> \"\"".to_string()
        }
    }
}

fn emit_ex_render_call(arg: &Expr, ctx: &ExViewCtx) -> String {
    match &*arg.node {
        ExprNode::Var { name, .. } | ExprNode::Ivar { name }
            if ctx.is_local(name.as_str()) =>
        {
            let singular = crate::naming::singularize(name.as_str());
            let partial_fn = format!("render_{}_{singular}", name.as_str());
            let coll = name.to_string();
            format!(
                "buf = buf <> Enum.map_join({coll}, \"\", fn r -> {partial_fn}(r) end)"
            )
        }
        ExprNode::Send { recv: Some(r), method, args, .. }
            if args.is_empty()
                && matches!(&*r.node, ExprNode::Var { .. } | ExprNode::Ivar { .. }) =>
        {
            let assoc_plural = method.as_str();
            let singular = crate::naming::singularize(assoc_plural);
            let partial_fn = format!("render_{assoc_plural}_{singular}");
            let parent_name = match &*r.node {
                ExprNode::Var { name, .. } | ExprNode::Ivar { name } => name.to_string(),
                _ => unreachable!(),
            };
            let parent_class = crate::naming::singularize_camelize(&parent_name);
            format!(
                "buf = buf <> Enum.map_join({parent_class}.{assoc_plural}({parent_name}), \"\", fn c -> {partial_fn}(c) end)"
            )
        }
        _ => "buf = buf <> \"\" # TODO ERB: render".to_string(),
    }
}

fn ex_extract_kwarg<'a>(arg: &'a Expr, key: &str) -> Option<&'a Expr> {
    if let ExprNode::Hash { entries, .. } = &*arg.node {
        for (k, v) in entries {
            if let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node {
                if value.as_str() == key {
                    return Some(v);
                }
            }
        }
    }
    None
}

fn ex_text_references(text: &str, ident: &str) -> bool {
    let bytes = text.as_bytes();
    let needle = ident.as_bytes();
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            let prev_is_word = i > 0
                && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
            let next_idx = i + needle.len();
            let next_is_word = next_idx < bytes.len()
                && (bytes[next_idx].is_ascii_alphanumeric()
                    || bytes[next_idx] == b'_');
            if !prev_is_word && !next_is_word {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn unwrap_to_s_ex(expr: &Expr) -> &Expr {
    if let ExprNode::Send { recv: Some(inner), method, args, .. } = &*expr.node {
        if method.as_str() == "to_s" && args.is_empty() {
            return inner;
        }
    }
    expr
}

fn ex_string_literal(s: &str) -> String {
    format!("{s:?}")
}

fn is_ex_simple_expr(expr: &Expr, ctx: &ExViewCtx) -> bool {
    match &*expr.node {
        ExprNode::Lit { .. } => true,
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } => {
            ctx.is_local(name.as_str())
        }
        ExprNode::Send { recv: Some(r), method, args, block, .. } => {
            if !args.is_empty() || block.is_some() {
                return false;
            }
            let clean = method.as_str().trim_end_matches('?').trim_end_matches('!');
            if clean.is_empty() {
                return false;
            }
            if let ExprNode::Var { name, .. } | ExprNode::Ivar { name } = &*r.node {
                if ctx.arg_has_attr(name.as_str(), clean) {
                    return true;
                }
                if ctx.is_local(name.as_str())
                    && matches!(
                        method.as_str(),
                        "any?" | "none?" | "present?" | "empty?"
                    )
                {
                    return true;
                }
            }
            false
        }
        ExprNode::StringInterp { parts } => parts.iter().all(|p| match p {
            crate::expr::InterpPart::Text { .. } => true,
            crate::expr::InterpPart::Expr { expr } => is_ex_simple_expr(expr, ctx),
        }),
        _ => false,
    }
}

fn emit_ex_view_expr_raw(expr: &Expr, ctx: &ExViewCtx) -> String {
    match &*expr.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } => name.to_string(),
        ExprNode::Send { recv: Some(r), method, args, .. } => {
            let method_s = method.as_str();
            if args.is_empty() {
                if let ExprNode::Var { name, .. } | ExprNode::Ivar { name } = &*r.node {
                    if ctx.is_local(name.as_str()) {
                        match method_s {
                            "any?" | "present?" => return format!("(length({name}) > 0)"),
                            "none?" | "empty?" => return format!("(length({name}) == 0)"),
                            _ => {}
                        }
                    }
                }
            }
            let recv_s = emit_ex_view_expr_raw(r, ctx);
            let clean = method_s.trim_end_matches('?').trim_end_matches('!');
            if args.is_empty() {
                format!("{recv_s}.{clean}")
            } else {
                let args_s: Vec<String> = args
                    .iter()
                    .map(|a| emit_ex_view_expr_raw(a, ctx))
                    .collect();
                format!("{recv_s}.{clean}({})", args_s.join(", "))
            }
        }
        ExprNode::StringInterp { parts } => {
            use crate::expr::InterpPart;
            let mut out = String::from("\"");
            for p in parts {
                match p {
                    InterpPart::Text { value } => {
                        for c in value.chars() {
                            match c {
                                '"' => out.push_str("\\\""),
                                '\\' => out.push_str("\\\\"),
                                '\n' => out.push_str("\\n"),
                                other => out.push(other),
                            }
                        }
                    }
                    InterpPart::Expr { expr } => {
                        out.push_str("#{");
                        out.push_str(&emit_ex_view_expr_raw(expr, ctx));
                        out.push('}');
                    }
                }
            }
            out.push('"');
            out
        }
        _ => "\"\"".to_string(),
    }
}

fn ex_collect_ivar_names(expr: &Expr) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    ex_collect_ivars_into(expr, &mut out);
    out
}

fn ex_collect_ivars_into(expr: &Expr, out: &mut Vec<String>) {
    match &*expr.node {
        ExprNode::Ivar { name } => {
            let n = name.to_string();
            if !out.iter().any(|existing| existing == &n) {
                out.push(n);
            }
        }
        ExprNode::Assign { target, value } => {
            if let LValue::Ivar { name } = target {
                let n = name.to_string();
                if !out.iter().any(|existing| existing == &n) {
                    out.push(n);
                }
            }
            ex_collect_ivars_into(value, out);
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                ex_collect_ivars_into(r, out);
            }
            for a in args {
                ex_collect_ivars_into(a, out);
            }
            if let Some(b) = block {
                ex_collect_ivars_into(b, out);
            }
        }
        ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
            for e in exprs {
                ex_collect_ivars_into(e, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                ex_collect_ivars_into(k, out);
                ex_collect_ivars_into(v, out);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            ex_collect_ivars_into(cond, out);
            ex_collect_ivars_into(then_branch, out);
            ex_collect_ivars_into(else_branch, out);
        }
        ExprNode::BoolOp { left, right, .. } => {
            ex_collect_ivars_into(left, out);
            ex_collect_ivars_into(right, out);
        }
        ExprNode::Lambda { body, .. } => ex_collect_ivars_into(body, out),
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let crate::expr::InterpPart::Expr { expr } = p {
                    ex_collect_ivars_into(expr, out);
                }
            }
        }
        _ => {}
    }
}

// Bodies ---------------------------------------------------------------

/// Emit a method / action body as Elixir statements. Ruby ivar writes
/// become local rebinds (`@post = …` → `post = …`); ivar reads become
/// struct field access through the receiver arg. If `receiver_arg` is
/// `None` (e.g. a controller action), ivar reads become bare locals.
fn emit_block(body: &Expr, receiver_arg: Option<&str>) -> String {
    match &*body.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            let mut lines: Vec<String> = Vec::new();
            for (i, e) in exprs.iter().enumerate() {
                if i > 0 && e.leading_blank_line {
                    lines.push(String::new());
                }
                lines.push(emit_stmt(e, receiver_arg));
            }
            lines.join("\n")
        }
        _ => emit_stmt(body, receiver_arg),
    }
}

fn emit_stmt(e: &Expr, receiver_arg: Option<&str>) -> String {
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            format!("{} = {}", name, emit_expr(value, receiver_arg))
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            // Elixir has no instance state. At scaffold depth, treat
            // `@foo = expr` as a local rebind `foo = expr`; real
            // controller code that mutates @post across multiple
            // statements needs a `with` pipeline, which Phase 3 adds.
            format!("{} = {}", name, emit_expr(value, receiver_arg))
        }
        _ => emit_expr(e, receiver_arg),
    }
}

fn emit_expr(e: &Expr, receiver_arg: Option<&str>) -> String {
    match &*e.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".")
        }
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Ivar { name } => {
            // Inside an instance method, `@foo` is a field on the
            // record arg: `post.foo`. Outside (e.g., in a controller
            // action), we've rebound to a local, so emit the bare name.
            match receiver_arg {
                Some(recv) => format!("{recv}.{name}"),
                None => name.to_string(),
            }
        }
        ExprNode::Send { recv, method, args, .. } => {
            emit_send(recv.as_ref(), method.as_str(), args, receiver_arg)
        }
        ExprNode::Assign { target: _, value } => emit_expr(value, receiver_arg),
        ExprNode::Seq { exprs } => exprs
            .iter()
            .map(|e| emit_expr(e, receiver_arg))
            .collect::<Vec<_>>()
            .join("; "),
        ExprNode::If { cond, then_branch, else_branch } => {
            let cond_s = emit_expr(cond, receiver_arg);
            let then_s = emit_block(then_branch, receiver_arg);
            let else_s = emit_block(else_branch, receiver_arg);
            // Elixir's `if / else / end` form. `case` would be more
            // idiomatic for `{:ok, _} / {:error, _}` shapes but that's
            // a Phase-3 semantic transform.
            format!(
                "if {cond_s} do\n{}\nelse\n{}\nend",
                indent(&then_s, 1),
                indent(&else_s, 1),
            )
        }
        ExprNode::BoolOp { op, left, right, .. } => {
            use crate::expr::BoolOpKind;
            let op_s = match op {
                BoolOpKind::Or => "or",
                BoolOpKind::And => "and",
            };
            format!(
                "{} {op_s} {}",
                emit_expr(left, receiver_arg),
                emit_expr(right, receiver_arg),
            )
        }
        ExprNode::Array { elements, .. } => {
            let parts: Vec<String> = elements.iter().map(|e| emit_expr(e, receiver_arg)).collect();
            format!("[{}]", parts.join(", "))
        }
        ExprNode::Hash { entries, .. } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| {
                    // A symbol key in Ruby (`foo: 1`) becomes an atom key
                    // in an Elixir map: `%{foo: 1}` (shorthand) or
                    // `%{:foo => 1}`. Emit the shorthand when the key is
                    // a bareword-safe symbol; rocket form otherwise.
                    if let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node {
                        format!("{value}: {}", emit_expr(v, receiver_arg))
                    } else {
                        format!(
                            "{} => {}",
                            emit_expr(k, receiver_arg),
                            emit_expr(v, receiver_arg),
                        )
                    }
                })
                .collect();
            format!("%{{{}}}", parts.join(", "))
        }
        ExprNode::StringInterp { parts } => {
            // Elixir interpolation syntax matches Ruby exactly:
            // `"text #{expr} more"`. Emit verbatim.
            use crate::expr::InterpPart;
            let mut out = String::from("\"");
            for p in parts {
                match p {
                    InterpPart::Text { value } => {
                        for c in value.chars() {
                            match c {
                                '"' => out.push_str("\\\""),
                                '\\' => out.push_str("\\\\"),
                                '\n' => out.push_str("\\n"),
                                other => out.push(other),
                            }
                        }
                    }
                    InterpPart::Expr { expr } => {
                        out.push_str("#{");
                        out.push_str(&emit_expr(expr, receiver_arg));
                        out.push('}');
                    }
                }
            }
            out.push('"');
            out
        }
        ExprNode::Yield { args } => {
            let parts: Vec<String> = args.iter().map(|e| emit_expr(e, receiver_arg)).collect();
            // Elixir doesn't have `yield`; use `send(self, …)` as a
            // placeholder that parses. Real runtime work would pattern
            // this into a block-passing convention.
            format!("send(self(), {{:yield, {}}})", parts.join(", "))
        }
        other => format!("# TODO: emit {:?}", std::mem::discriminant(other)),
    }
}

fn emit_send(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    receiver_arg: Option<&str>,
) -> String {
    let args_s: Vec<String> = args.iter().map(|e| emit_expr(e, receiver_arg)).collect();

    if method == "[]" && recv.is_some() {
        // `params[:id]` → `params[:id]` (Elixir maps with atom keys
        // support the same access syntax). For string keys or integer
        // indexing this would lower to `Map.fetch!` / `Enum.at`.
        return format!("{}[{}]", emit_expr(recv.unwrap(), receiver_arg), args_s.join(", "));
    }

    match recv {
        None => {
            // Bareword call. In Elixir this is a function in the
            // enclosing module or an imported function — the scaffold
            // emits as-is.
            if args_s.is_empty() {
                method.to_string()
            } else {
                format!("{}({})", method, args_s.join(", "))
            }
        }
        Some(r) => {
            let recv_s = emit_expr(r, receiver_arg);
            // Ruby String methods map onto Elixir's `String` module
            // functions (module-function-call form, not method). `.strip`
            // → `String.trim(recv)`, upcase/downcase similar.
            if args.is_empty() && matches!(r.ty, Some(crate::ty::Ty::Str)) {
                if let Some(wrapped) = map_ex_str_method(method, &recv_s) {
                    return wrapped;
                }
            }
            // `recv.method(args)` reads fine for both module function
            // calls (e.g. `Post.find(id)`) and struct-field-style
            // getters (`post.title` with no args).
            if args_s.is_empty() {
                format!("{recv_s}.{method}")
            } else {
                format!("{recv_s}.{method}({})", args_s.join(", "))
            }
        }
    }
}

/// Map Ruby String methods onto Elixir's `String` module functions
/// (module-function-call form — Elixir strings don't have `.method`
/// dispatch). Returns `Some(emit_text)` for a handled method; unhandled
/// methods fall through to the default `recv.method` emit.
fn map_ex_str_method(method: &str, recv_text: &str) -> Option<String> {
    match method {
        "strip" => Some(format!("String.trim({recv_text})")),
        "upcase" => Some(format!("String.upcase({recv_text})")),
        "downcase" => Some(format!("String.downcase({recv_text})")),
        "length" | "size" => Some(format!("String.length({recv_text})")),
        "empty?" => Some(format!("{recv_text} == \"\"")),
        _ => None,
    }
}

fn emit_literal(lit: &Literal) -> String {
    match lit {
        Literal::Nil => "nil".to_string(),
        Literal::Bool { value } => value.to_string(),
        Literal::Int { value } => value.to_string(),
        Literal::Float { value } => {
            let s = value.to_string();
            if s.contains('.') { s } else { format!("{s}.0") }
        }
        Literal::Str { value } => format!("{value:?}"),
        // Ruby symbols map cleanly to Elixir atoms.
        Literal::Sym { value } => format!(":{}", value.as_str()),
    }
}

fn indent(text: &str, depth: usize) -> String {
    let pad = "  ".repeat(depth);
    text.lines()
        .map(|l| if l.is_empty() { String::new() } else { format!("{pad}{l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

// Fixtures + tests ---------------------------------------------------

fn emit_ex_fixture(lowered: &crate::lower::LoweredFixture) -> EmittedFile {
    let fixture_name = lowered.name.as_str();
    let class_name = lowered.class.0.as_str();
    let ns = crate::naming::camelize(fixture_name);

    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "defmodule Fixtures.{ns} do").unwrap();

    // _load_all — invoked from Fixtures.setup in the test setup
    // callback. Each record runs through Class.save/1 (validations +
    // INSERT) then Roundhouse.Db.last_insert_rowid captures the
    // autoincrement id — exposed via the shared fixture-id map.
    writeln!(s).unwrap();
    writeln!(s, "  def _load_all do").unwrap();
    for record in &lowered.records {
        let label = record.label.as_str();
        let mut field_lines: Vec<String> = Vec::new();
        for field in &record.fields {
            let col = field.column.as_str();
            let val = match &field.value {
                crate::lower::LoweredFixtureValue::Literal { ty, raw } => {
                    ex_literal_for(raw, ty)
                }
                crate::lower::LoweredFixtureValue::FkLookup {
                    target_fixture,
                    target_label,
                } => format!(
                    "Fixtures.fixture_id({:?}, {:?})",
                    target_fixture.as_str(),
                    target_label.as_str(),
                ),
            };
            field_lines.push(format!("      {col}: {val}"));
        }
        writeln!(s, "    record = %{class_name}{{").unwrap();
        for (idx, line) in field_lines.iter().enumerate() {
            if idx < field_lines.len() - 1 {
                writeln!(s, "{line},").unwrap();
            } else {
                writeln!(s, "{line}").unwrap();
            }
        }
        writeln!(s, "    }}").unwrap();
        writeln!(
            s,
            "    unless {class_name}.save(record), do: raise \"fixture {fixture_name}/{label} failed to save\""
        )
        .unwrap();
        writeln!(
            s,
            "    Fixtures.register({fixture_name:?}, {label:?}, Roundhouse.Db.last_insert_rowid())"
        )
        .unwrap();
    }
    writeln!(s, "    :ok").unwrap();
    writeln!(s, "  end").unwrap();

    // Named getters — look up id and Class.find.
    for record in &lowered.records {
        let label = record.label.as_str();
        writeln!(s).unwrap();
        writeln!(s, "  def {label}() do").unwrap();
        writeln!(
            s,
            "    id = Fixtures.fixture_id({fixture_name:?}, {label:?})"
        )
        .unwrap();
        writeln!(s, "    {class_name}.find(id)").unwrap();
        writeln!(s, "  end").unwrap();
    }

    writeln!(s, "end").unwrap();

    EmittedFile {
        path: PathBuf::from(format!("test/support/fixtures/{fixture_name}.ex")),
        content: s,
    }
}

/// `test/support/fixtures.ex` — shared Fixtures module with setup,
/// fixture_id lookup, and the register helper the per-class
/// _load_all functions call.
fn emit_ex_fixtures_helper(lowered: &crate::lower::LoweredFixtureSet) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "defmodule Fixtures do").unwrap();
    writeln!(s, "  @moduledoc false").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "  @doc \"Per-test setup. Called from each test module's ExUnit setup.\"").unwrap();
    writeln!(s, "  def setup do").unwrap();
    writeln!(
        s,
        "    Roundhouse.Db.setup_test_db(Roundhouse.SchemaSQL.create_tables())"
    )
    .unwrap();
    writeln!(s, "    Process.put(:roundhouse_fixture_ids, %{{}})").unwrap();
    for f in &lowered.fixtures {
        let ns = crate::naming::camelize(f.name.as_str());
        writeln!(s, "    Fixtures.{ns}._load_all()").unwrap();
    }
    writeln!(s, "    :ok").unwrap();
    writeln!(s, "  end").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "  def register(fixture, label, id) do").unwrap();
    writeln!(s, "    ids = Process.get(:roundhouse_fixture_ids, %{{}})").unwrap();
    writeln!(
        s,
        "    Process.put(:roundhouse_fixture_ids, Map.put(ids, {{fixture, label}}, id))"
    )
    .unwrap();
    writeln!(s, "  end").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "  def fixture_id(fixture, label) do").unwrap();
    writeln!(
        s,
        "    case Map.get(Process.get(:roundhouse_fixture_ids, %{{}}), {{fixture, label}}) do"
    )
    .unwrap();
    writeln!(s, "      nil -> raise \"fixture #{{fixture}}/#{{label}} not loaded\"").unwrap();
    writeln!(s, "      id -> id").unwrap();
    writeln!(s, "    end").unwrap();
    writeln!(s, "  end").unwrap();
    writeln!(s, "end").unwrap();
    EmittedFile {
        path: PathBuf::from("test/support/fixtures.ex"),
        content: s,
    }
}

fn ex_literal_for(value: &str, ty: &crate::ty::Ty) -> String {
    use crate::ty::Ty;
    match ty {
        Ty::Str | Ty::Sym => format!("{value:?}"),
        Ty::Int => {
            if value.parse::<i64>().is_ok() {
                value.to_string()
            } else {
                format!("0 # TODO: coerce {value:?}")
            }
        }
        Ty::Float => {
            if value.parse::<f64>().is_ok() {
                value.to_string()
            } else {
                format!("0.0 # TODO: coerce {value:?}")
            }
        }
        Ty::Bool => match value {
            "true" | "1" => "true".into(),
            "false" | "0" => "false".into(),
            _ => format!("false # TODO: coerce {value:?}"),
        },
        Ty::Class { id, .. } if id.0.as_str() == "Time" => format!("{value:?}"),
        _ => format!("{value:?}"),
    }
}

fn emit_ex_test(tm: &TestModule, app: &App) -> EmittedFile {
    let fixture_names: Vec<Symbol> =
        app.fixtures.iter().map(|f| f.name.clone()).collect();
    let known_models: Vec<Symbol> =
        app.models.iter().map(|m| m.name.0.clone()).collect();
    let mut attrs_set: std::collections::BTreeSet<Symbol> =
        std::collections::BTreeSet::new();
    for m in &app.models {
        for attr in m.attributes.fields.keys() {
            attrs_set.insert(attr.clone());
        }
    }
    let model_attrs: Vec<Symbol> = attrs_set.into_iter().collect();

    let ctx = ExTestCtx {
        app,
        fixture_names: &fixture_names,
        known_models: &known_models,
        model_attrs: &model_attrs,
    };

    let is_controller_test = tm.name.0.as_str().ends_with("ControllerTest");

    let mut s = String::new();
    writeln!(s, "# Generated by Roundhouse.").unwrap();
    writeln!(s, "defmodule {} do", tm.name.0).unwrap();
    writeln!(s, "  use ExUnit.Case").unwrap();
    if is_controller_test {
        writeln!(s, "  alias Roundhouse.TestClient").unwrap();
        writeln!(s, "  alias Roundhouse.TestResponse").unwrap();
        writeln!(s, "  import Roundhouse.RouteHelpers").unwrap();
    }
    // Each test starts on a fresh :memory: SQLite DB with all
    // fixtures loaded — Rails' transactional-fixture isolation
    // adapted to Elixir's per-process test semantics.
    if !app.fixtures.is_empty() {
        writeln!(s).unwrap();
        writeln!(s, "  setup do").unwrap();
        if is_controller_test {
            writeln!(s, "    App.Routes.register()").unwrap();
        }
        writeln!(s, "    Fixtures.setup()").unwrap();
        writeln!(s, "    :ok").unwrap();
        writeln!(s, "  end").unwrap();
    }

    for test in &tm.tests {
        writeln!(s).unwrap();
        if is_controller_test {
            writeln!(s, "  test {:?} do", test.name).unwrap();
            let body = emit_ex_controller_test_body(test, app, ctx);
            if body.is_empty() {
                writeln!(s, "    :ok").unwrap();
            } else {
                for line in body.lines() {
                    writeln!(s, "    {line}").unwrap();
                }
            }
            writeln!(s, "  end").unwrap();
        } else if test_needs_runtime_unsupported_ex(test) {
            writeln!(s, "  @tag :skip").unwrap();
            writeln!(s, "  test {:?} do", test.name).unwrap();
            writeln!(s, "    # Phase 3: needs persistence runtime").unwrap();
            writeln!(s, "  end").unwrap();
        } else {
            writeln!(s, "  test {:?} do", test.name).unwrap();
            let body = emit_ex_test_body(&test.body, ctx);
            for line in body.lines() {
                writeln!(s, "    {line}").unwrap();
            }
            writeln!(s, "  end").unwrap();
        }
    }

    writeln!(s, "end").unwrap();

    let filename = snake_case(tm.name.0.as_str());
    EmittedFile {
        path: PathBuf::from(format!("test/{filename}.exs")),
        content: s,
    }
}

#[derive(Clone, Copy)]
struct ExTestCtx<'a> {
    app: &'a App,
    fixture_names: &'a [Symbol],
    known_models: &'a [Symbol],
    model_attrs: &'a [Symbol],
}

fn emit_ex_test_body(body: &Expr, ctx: ExTestCtx) -> String {
    match &*body.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => exprs
            .iter()
            .map(|e| emit_ex_test_stmt(e, ctx))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => emit_ex_test_stmt(body, ctx),
    }
}

fn emit_ex_test_stmt(e: &Expr, ctx: ExTestCtx) -> String {
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            format!("{} = {}", name, emit_ex_test_expr(value, ctx))
        }
        _ => emit_ex_test_expr(e, ctx),
    }
}

fn emit_ex_test_expr(e: &Expr, ctx: ExTestCtx) -> String {
    match &*e.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".")
        }
        ExprNode::Send { recv, method, args, block, .. } => {
            emit_ex_test_send(recv.as_ref(), method.as_str(), args, block.as_ref(), ctx)
        }
        ExprNode::BoolOp { op, left, right, .. } => {
            use crate::expr::BoolOpKind;
            let op_s = match op {
                BoolOpKind::Or => "or",
                BoolOpKind::And => "and",
            };
            format!(
                "{} {op_s} {}",
                emit_ex_test_expr(left, ctx),
                emit_ex_test_expr(right, ctx)
            )
        }
        ExprNode::Hash { entries, .. } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| {
                    if let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node {
                        format!("{value}: {}", emit_ex_test_expr(v, ctx))
                    } else {
                        format!(
                            "{} => {}",
                            emit_ex_test_expr(k, ctx),
                            emit_ex_test_expr(v, ctx),
                        )
                    }
                })
                .collect();
            format!("%{{{}}}", parts.join(", "))
        }
        _ => format!("# TODO: Elixir test emit for {:?}", std::mem::discriminant(&*e.node)),
    }
}

fn emit_ex_test_send(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    ctx: ExTestCtx,
) -> String {
    let args_s: Vec<String> = args.iter().map(|a| emit_ex_test_expr(a, ctx)).collect();

    // Fixture accessor: articles(:one) → Fixtures.Articles.one()
    if recv.is_none()
        && args.len() == 1
        && ctx.fixture_names.iter().any(|s| s.as_str() == method)
    {
        if let ExprNode::Lit { value: Literal::Sym { value: sym } } = &*args[0].node {
            let ns = crate::naming::camelize(method);
            return format!("Fixtures.{ns}.{}()", sym.as_str());
        }
    }

    // assert_difference("Class.count", delta) do ... end →
    // `_before = ...; <body>; _after = ...; assert _after - _before == delta`.
    if recv.is_none() && method == "assert_difference" {
        if let Some(body) = block {
            if let Some(count_expr) = args
                .first()
                .and_then(|a| match &*a.node {
                    ExprNode::Lit { value: Literal::Str { value } } => {
                        rewrite_ruby_dot_call_ex(value)
                    }
                    _ => None,
                })
            {
                let delta = args_s.get(1).cloned().unwrap_or_else(|| "1".into());
                let body_s = emit_block_body_ex(body, ctx);
                return format!(
                    "count_before = {count_expr}\n{body_s}\ncount_after = {count_expr}\nassert count_after - count_before == {delta}"
                );
            }
        }
    }

    // owner.<assoc>.create(hash) / .build(hash) — HasMany rewrite
    // wrapped in an anonymous function so the record (with its
    // assigned id) becomes available as the expression's value.
    if (method == "create" || method == "build") && args.len() == 1 {
        if let Some(outer_recv) = recv {
            if let ExprNode::Send {
                recv: Some(assoc_recv),
                method: assoc_method,
                args: inner_args,
                ..
            } = &*outer_recv.node
            {
                if inner_args.is_empty() {
                    if let Some(s) = try_emit_assoc_create_ex(
                        assoc_recv,
                        assoc_method.as_str(),
                        args,
                        method,
                        ctx,
                    ) {
                        return s;
                    }
                }
            }
        }
    }

    // Assertion macros → ExUnit's assert/refute.
    if recv.is_none() {
        match (method, args_s.len()) {
            ("assert_equal", 2) => {
                return format!(
                    "assert {actual} == {expected}",
                    expected = args_s[0],
                    actual = args_s[1]
                );
            }
            ("assert_not_equal", 2) => {
                return format!(
                    "refute {actual} == {expected}",
                    expected = args_s[0],
                    actual = args_s[1]
                );
            }
            ("assert_not", 1) => return format!("refute {}", args_s[0]),
            ("assert", 1) => return format!("assert {}", args_s[0]),
            ("assert_nil", 1) => return format!("assert is_nil({})", args_s[0]),
            ("assert_not_nil", 1) => return format!("refute is_nil({})", args_s[0]),
            _ => {}
        }
    }

    // `Class.new(hash)` → `%Class{ k: v, ... }` struct literal.
    if let Some(r) = recv {
        if method == "new" && args.len() == 1 {
            if let ExprNode::Const { path } = &*r.node {
                if let Some(class_name) = path.last() {
                    if ctx.known_models.iter().any(|s| s == class_name) {
                        if let ExprNode::Hash { entries, .. } = &*args[0].node {
                            let pairs: Vec<String> = entries
                                .iter()
                                .filter_map(|(k, v)| {
                                    if let ExprNode::Lit {
                                        value: Literal::Sym { value: f },
                                    } = &*k.node
                                    {
                                        Some(format!(
                                            "{}: {}",
                                            f.as_str(),
                                            emit_ex_test_expr(v, ctx)
                                        ))
                                    } else {
                                        None
                                    }
                                })
                                .collect();
                            return format!("%{} {{{}}}", class_name, pairs.join(", "));
                        }
                    }
                }
            }
        }
    }

    match recv {
        None => {
            if args_s.is_empty() {
                method.to_string()
            } else {
                format!("{}({})", method, args_s.join(", "))
            }
        }
        Some(r) => {
            let recv_s = emit_ex_test_expr(r, ctx);
            let is_class_call = matches!(&*r.node, ExprNode::Const { .. });
            if is_class_call {
                // Module.function(args) — Elixir's module-function-call form.
                if args_s.is_empty() {
                    format!("{recv_s}.{method}()")
                } else {
                    format!("{recv_s}.{method}({})", args_s.join(", "))
                }
            } else {
                let is_attr = args_s.is_empty()
                    && ctx.model_attrs.iter().any(|s| s.as_str() == method);
                if is_attr {
                    format!("{recv_s}.{method}")
                } else if matches!(method, "save" | "destroy") && args_s.is_empty() {
                    // Instance-method-like call on a model record:
                    // `article.save` / `article.destroy` in Ruby →
                    // `Module.save(article)` / `Module.destroy(article)`.
                    format!("{}(\n      {recv_s}\n    )", ex_module_fn_for(r, method))
                } else if args_s.is_empty() {
                    format!("{recv_s}.{method}")
                } else {
                    format!("{recv_s}.{method}({})", args_s.join(", "))
                }
            }
        }
    }
}

/// Parse a Ruby-style `"Class.method"` expression into Elixir
/// `Class.method()`. Capitalized LHS → module function call;
/// lowercase LHS → instance field access (same as in the other
/// targets).
fn rewrite_ruby_dot_call_ex(expr: &str) -> Option<String> {
    let trimmed = expr.trim();
    let (lhs, rhs) = trimmed.split_once('.')?;
    let is_ident = |s: &str| {
        !s.is_empty()
            && s.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
            && s.chars().all(|c| c.is_alphanumeric() || c == '_')
    };
    if !is_ident(lhs) || !is_ident(rhs) {
        return None;
    }
    let is_class = lhs.chars().next().is_some_and(|c| c.is_uppercase());
    if is_class {
        Some(format!("{lhs}.{rhs}()"))
    } else {
        Some(format!("{lhs}.{rhs}"))
    }
}

/// Render a Ruby block body as Elixir statements, peeling one
/// Lambda layer. Ruby `do ... end` lowers to `ExprNode::Lambda`.
fn emit_block_body_ex(e: &Expr, ctx: ExTestCtx) -> String {
    let inner = match &*e.node {
        ExprNode::Lambda { body, .. } => body,
        _ => e,
    };
    match &*inner.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => exprs
            .iter()
            .map(|s| emit_ex_test_stmt(s, ctx))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => emit_ex_test_stmt(inner, ctx),
    }
}

fn try_emit_assoc_create_ex(
    owner: &Expr,
    assoc_name: &str,
    args: &[Expr],
    outer_method: &str,
    ctx: ExTestCtx,
) -> Option<String> {
    let resolved = crate::lower::resolve_has_many(
        &Symbol::from(assoc_name),
        owner.ty.as_ref(),
        ctx.app,
    )?;
    let target_class = resolved.target_class.0.as_str();
    let foreign_key = resolved.foreign_key.as_str();

    let owner_s = emit_ex_test_expr(owner, ctx);
    let hash_entries = match &args.first()?.node.as_ref() {
        ExprNode::Hash { entries, .. } => entries.clone(),
        _ => return None,
    };

    let mut pairs: Vec<String> = vec![format!("{foreign_key}: {owner_s}.id")];
    for (k, v) in &hash_entries {
        if let ExprNode::Lit { value: Literal::Sym { value: field_name } } = &*k.node {
            pairs.push(format!(
                "{}: {}",
                field_name.as_str(),
                emit_ex_test_expr(v, ctx),
            ));
        }
    }
    let struct_lit = format!("%{target_class}{{{}}}", pairs.join(", "));
    // Elixir has no direct "block expression" — an anonymous function
    // call is the standard way to evaluate a sequence and yield the
    // last value. `.create` saves and returns the record with its id
    // assigned; `.build` just constructs.
    if outer_method == "create" {
        Some(format!(
            "(fn ->\n      record = {struct_lit}\n      {target_class}.save(record)\n      %{{record | id: Roundhouse.Db.last_insert_rowid()}}\n    end).()"
        ))
    } else {
        Some(struct_lit)
    }
}

/// Guess the module-function path for `recv.method`. If recv is a
/// struct, the struct's module holds the function — so `article.save`
/// emits as `Article.save(article)`. Without type info we scrape the
/// variable name and infer.
fn ex_module_fn_for(recv: &Expr, method: &str) -> String {
    if let ExprNode::Var { name, .. } = &*recv.node {
        let n = name.as_str();
        // Variable names for fixtures follow the snake-cased class name.
        let camelized = crate::naming::camelize(n);
        return format!("{camelized}.{method}");
    }
    format!("??.{method}") // fallback; should be rare
}

fn test_needs_runtime_unsupported_ex(_test: &Test) -> bool {
    // Phase 3 rounded out Elixir's real-blog coverage. Keep as a
    // future-guard; no current pattern forces a skip.
    false
}

// Pass-2 controller-test emit ------------------------------------------
//
// Walks the test AST via the shared classifier + renders each
// statement through an Elixir-specific assertion render table.

fn emit_ex_controller_test_body(test: &Test, app: &App, ctx: ExTestCtx) -> String {
    let mut out = String::new();
    // Prime ivars read without assignment: `@article` → `article = Fixtures.Articles.one()`.
    let walked = crate::lower::walk_controller_ivars(&test.body);
    for ivar in walked.ivars_read_without_assign() {
        let plural = crate::naming::pluralize_snake(ivar.as_str());
        if ctx.fixture_names.iter().any(|s| s.as_str() == plural) {
            let ns = crate::naming::camelize(&plural);
            out.push_str(&format!("{} = Fixtures.{}.one()\n", ivar.as_str(), ns));
        }
    }

    for stmt in crate::lower::test_body_stmts(&test.body) {
        let rendered = emit_ex_ctrl_test_stmt(stmt, app, ctx);
        out.push_str(&rendered);
        out.push('\n');
    }
    out
}

fn emit_ex_ctrl_test_stmt(stmt: &Expr, app: &App, ctx: ExTestCtx) -> String {
    match &*stmt.node {
        ExprNode::Send { recv: None, method, args, block, .. } => {
            emit_ex_ctrl_test_send(method.as_str(), args, block.as_ref(), app, ctx)
        }
        ExprNode::Send { recv: Some(r), method, args, .. } => {
            if method.as_str() == "reload" {
                let recv_s = match &*r.node {
                    ExprNode::Ivar { name } | ExprNode::Var { name, .. } => name.to_string(),
                    _ => emit_ex_ctrl_test_expr(r, app, ctx),
                };
                let module = match &*r.node {
                    ExprNode::Ivar { name } | ExprNode::Var { name, .. } => {
                        crate::naming::camelize(name.as_str())
                    }
                    _ => "Unknown".to_string(),
                };
                return format!("{recv_s} = {module}.reload({recv_s})");
            }
            let recv_s = emit_ex_ctrl_test_expr(r, app, ctx);
            let args_s: Vec<String> = args
                .iter()
                .map(|a| emit_ex_ctrl_test_expr(a, app, ctx))
                .collect();
            if args_s.is_empty() {
                format!("{recv_s}.{}", method.as_str())
            } else {
                format!("{recv_s}.{}({})", method.as_str(), args_s.join(", "))
            }
        }
        ExprNode::Assign { target: LValue::Var { name, .. }, value }
        | ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            format!("{name} = {}", emit_ex_ctrl_test_expr(value, app, ctx))
        }
        _ => emit_ex_ctrl_test_expr(stmt, app, ctx),
    }
}

fn emit_ex_ctrl_test_send(
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    app: &App,
    ctx: ExTestCtx,
) -> String {
    use crate::lower::{AssertSelectKind, ControllerTestSend};
    match crate::lower::classify_controller_test_send(method, args, block) {
        Some(ControllerTestSend::HttpGet { url }) => {
            let u = emit_ex_url_expr(url, app, ctx);
            format!("resp = TestClient.get({u})")
        }
        Some(ControllerTestSend::HttpWrite { method: m, url, params }) => {
            let u = emit_ex_url_expr(url, app, ctx);
            let body = params
                .map(|h| flatten_ex_params_to_form(h, None, app, ctx))
                .unwrap_or_else(|| "%{}".to_string());
            format!("resp = TestClient.{m}({u}, {body})")
        }
        Some(ControllerTestSend::HttpDelete { url }) => {
            let u = emit_ex_url_expr(url, app, ctx);
            format!("resp = TestClient.delete({u})")
        }
        Some(ControllerTestSend::AssertResponse { sym }) => match sym.as_str() {
            "success" => "TestResponse.assert_ok(resp)".to_string(),
            "unprocessable_entity" => "TestResponse.assert_unprocessable(resp)".to_string(),
            other => format!("TestResponse.assert_status(resp, 200) # TODO: {other:?}"),
        },
        Some(ControllerTestSend::AssertRedirectedTo { url }) => {
            let u = emit_ex_url_expr(url, app, ctx);
            format!("TestResponse.assert_redirected_to(resp, {u})")
        }
        Some(ControllerTestSend::AssertSelect { selector, kind }) => {
            emit_ex_assert_select_classified(selector, kind, app, ctx)
        }
        Some(ControllerTestSend::AssertDifference {
            method: _,
            count_expr,
            delta,
            block,
        }) => emit_ex_assert_difference_classified(count_expr, delta, block, app, ctx),
        Some(ControllerTestSend::AssertEqual { expected, actual }) => {
            let e = emit_ex_ctrl_test_expr(expected, app, ctx);
            let a = emit_ex_ctrl_test_expr(actual, app, ctx);
            format!("assert {a} == {e}")
        }
        None => {
            let args_s: Vec<String> = args
                .iter()
                .map(|a| emit_ex_ctrl_test_expr(a, app, ctx))
                .collect();
            if args_s.is_empty() {
                method.to_string()
            } else {
                format!("{method}({})", args_s.join(", "))
            }
        }
    }
}

fn emit_ex_url_expr(expr: &Expr, app: &App, ctx: ExTestCtx) -> String {
    use crate::lower::UrlArg;
    let Some(helper) = crate::lower::classify_url_expr(expr) else {
        return emit_ex_ctrl_test_expr(expr, app, ctx);
    };
    let helper_name = format!("{}_path", helper.helper_base);
    let args_s: Vec<String> = helper
        .args
        .iter()
        .map(|a| match a {
            UrlArg::IvarOrVarId(name) => format!("{name}.id"),
            UrlArg::ModelLast(class) => format!("{}.last().id", class.as_str()),
            UrlArg::Raw(e) => emit_ex_ctrl_test_expr(e, app, ctx),
        })
        .collect();
    if args_s.is_empty() {
        format!("{helper_name}()")
    } else {
        format!("{helper_name}({})", args_s.join(", "))
    }
}

fn emit_ex_assert_select_classified(
    selector_expr: &Expr,
    kind: crate::lower::AssertSelectKind<'_>,
    app: &App,
    ctx: ExTestCtx,
) -> String {
    use crate::lower::AssertSelectKind;
    let ExprNode::Lit { value: Literal::Str { value: selector } } =
        &*selector_expr.node
    else {
        return format!(
            "TestResponse.assert_select(resp, {}) # TODO: dynamic selector",
            emit_ex_ctrl_test_expr(selector_expr, app, ctx),
        );
    };
    match kind {
        AssertSelectKind::Text(expr) => {
            let text = emit_ex_ctrl_test_expr(expr, app, ctx);
            format!("TestResponse.assert_select_text(resp, {selector:?}, {text})")
        }
        AssertSelectKind::Minimum(expr) => {
            let n = emit_ex_ctrl_test_expr(expr, app, ctx);
            format!("TestResponse.assert_select_min(resp, {selector:?}, {n})")
        }
        AssertSelectKind::SelectorBlock(b) => {
            let mut out = String::new();
            out.push_str(&format!("TestResponse.assert_select(resp, {selector:?})\n"));
            let inner_body = match &*b.node {
                ExprNode::Lambda { body, .. } => body,
                _ => b,
            };
            for stmt in crate::lower::test_body_stmts(inner_body) {
                out.push_str(&emit_ex_ctrl_test_stmt(stmt, app, ctx));
                out.push('\n');
            }
            out.trim_end().to_string()
        }
        AssertSelectKind::SelectorOnly => {
            format!("TestResponse.assert_select(resp, {selector:?})")
        }
    }
}

fn emit_ex_assert_difference_classified(
    count_expr_str: String,
    expected_delta: i64,
    block: Option<&Expr>,
    app: &App,
    ctx: ExTestCtx,
) -> String {
    // `Article.count` → `Article.count()` in Elixir.
    let count_expr = count_expr_str
        .split_once('.')
        .map(|(cls, m)| format!("{cls}.{m}()"))
        .unwrap_or_else(|| count_expr_str.clone());

    let mut out = String::new();
    out.push_str(&format!("count_before = {count_expr}\n"));
    if let Some(b) = block {
        let inner_body = match &*b.node {
            ExprNode::Lambda { body, .. } => body,
            _ => b,
        };
        for stmt in crate::lower::test_body_stmts(inner_body) {
            out.push_str(&emit_ex_ctrl_test_stmt(stmt, app, ctx));
            out.push('\n');
        }
    }
    out.push_str(&format!("count_after = {count_expr}\n"));
    out.push_str(&format!("assert count_after - count_before == {expected_delta}"));
    out
}

fn emit_ex_ctrl_test_expr(expr: &Expr, app: &App, ctx: ExTestCtx) -> String {
    match &*expr.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Ivar { name } => name.to_string(),
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".")
        }
        ExprNode::Send { recv: Some(r), method, args, .. } => {
            let m = method.as_str();
            if m == "last" && args.is_empty() {
                if let ExprNode::Const { path } = &*r.node {
                    let class = path.last().map(|s| s.as_str().to_string()).unwrap_or_default();
                    return format!("{class}.last()");
                }
            }
            if m == "count" && args.is_empty() {
                if let ExprNode::Const { path } = &*r.node {
                    let class = path.last().map(|s| s.as_str().to_string()).unwrap_or_default();
                    return format!("{class}.count()");
                }
            }
            if args.is_empty() {
                let recv_s = match &*r.node {
                    ExprNode::Ivar { name } | ExprNode::Var { name, .. } => name.to_string(),
                    _ => emit_ex_ctrl_test_expr(r, app, ctx),
                };
                return format!("{recv_s}.{m}");
            }
            let recv_s = emit_ex_ctrl_test_expr(r, app, ctx);
            let args_s: Vec<String> = args
                .iter()
                .map(|a| emit_ex_ctrl_test_expr(a, app, ctx))
                .collect();
            format!("{recv_s}.{m}({})", args_s.join(", "))
        }
        ExprNode::Send { recv: None, method, args, .. } => {
            if method.as_str().ends_with("_url") || method.as_str().ends_with("_path") {
                return emit_ex_url_expr(expr, app, ctx);
            }
            let args_s: Vec<String> = args
                .iter()
                .map(|a| emit_ex_ctrl_test_expr(a, app, ctx))
                .collect();
            if args_s.is_empty() {
                method.to_string()
            } else {
                format!("{method}({})", args_s.join(", "))
            }
        }
        ExprNode::Hash { entries, .. } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| {
                    if let ExprNode::Lit {
                        value: Literal::Sym { value },
                    } = &*k.node
                    {
                        format!("{value}: {}", emit_ex_ctrl_test_expr(v, app, ctx))
                    } else {
                        format!(
                            "{} => {}",
                            emit_ex_ctrl_test_expr(k, app, ctx),
                            emit_ex_ctrl_test_expr(v, app, ctx),
                        )
                    }
                })
                .collect();
            format!("%{{{}}}", parts.join(", "))
        }
        _ => "nil # TODO expr".to_string(),
    }
}

fn flatten_ex_params_to_form(
    expr: &Expr,
    scope: Option<&str>,
    app: &App,
    ctx: ExTestCtx,
) -> String {
    let pairs: Vec<String> = crate::lower::flatten_params_pairs(expr, scope)
        .into_iter()
        .map(|(key, value)| {
            let val = emit_ex_ctrl_test_expr(value, app, ctx);
            format!("{key:?} => to_string({val})")
        })
        .collect();
    format!("%{{{}}}", pairs.join(", "))
}

#[allow(dead_code)]

fn test_body_uses_unsupported_ex(e: &Expr) -> bool {
    use crate::expr::InterpPart;
    let self_hit = match &*e.node {
        ExprNode::Send { recv, method, .. } => {
            let m = method.as_str();
            matches!(
                m,
                "assert_difference"
                    | "destroy"
                    | "destroy!"
                    | "build"
                    | "create"
                    | "create!"
            ) || (m == "count"
                && recv.as_ref().is_some_and(|r| matches!(&*r.node, ExprNode::Const { .. })))
        }
        _ => false,
    };
    if self_hit {
        return true;
    }
    match &*e.node {
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                if test_body_uses_unsupported_ex(r) {
                    return true;
                }
            }
            for a in args {
                if test_body_uses_unsupported_ex(a) {
                    return true;
                }
            }
            if let Some(b) = block {
                if test_body_uses_unsupported_ex(b) {
                    return true;
                }
            }
        }
        ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
            for e in exprs {
                if test_body_uses_unsupported_ex(e) {
                    return true;
                }
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                if test_body_uses_unsupported_ex(k) || test_body_uses_unsupported_ex(v) {
                    return true;
                }
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    if test_body_uses_unsupported_ex(expr) {
                        return true;
                    }
                }
            }
        }
        ExprNode::BoolOp { left, right, .. }
        | ExprNode::RescueModifier { expr: left, fallback: right } => {
            if test_body_uses_unsupported_ex(left) || test_body_uses_unsupported_ex(right) {
                return true;
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            if test_body_uses_unsupported_ex(cond)
                || test_body_uses_unsupported_ex(then_branch)
                || test_body_uses_unsupported_ex(else_branch)
            {
                return true;
            }
        }
        ExprNode::Let { value, body, .. } => {
            if test_body_uses_unsupported_ex(value) || test_body_uses_unsupported_ex(body) {
                return true;
            }
        }
        ExprNode::Lambda { body, .. } => {
            if test_body_uses_unsupported_ex(body) {
                return true;
            }
        }
        ExprNode::Assign { value, .. } => {
            if test_body_uses_unsupported_ex(value) {
                return true;
            }
        }
        _ => {}
    }
    false
}
